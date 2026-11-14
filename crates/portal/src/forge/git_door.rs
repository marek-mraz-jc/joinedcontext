//! The configuration repositories over git's smart HTTP, read-only (T-6896, T-6893 part 2).
//!
//! Two paths and no third: `GET /git/{owner}/{repo}.git/info/refs?service=git-upload-pack` and
//! `POST /git/{owner}/{repo}.git/git-upload-pack`, so `git clone` and `git fetch` work and a push
//! has nothing to talk to. The caller is a person with a Keycloak access token in the
//! `Authorization` header (`git -c http.extraHeader="Authorization: Bearer $TOKEN" clone ...`);
//! a token in the URL is refused, since a URL ends up in logs and shell history.
//!
//! A project repository is read by the members of that project, the installation repository,
//! which holds every Organization, by platform administrators alone ([`readable`]). Everything
//! refused answers the same `404`: no token, a bad one, no right, no such repository, a push, so
//! the door never says which repositories exist. The Portal reads the forge with its own machine
//! user and streams the answer back.

use std::collections::HashMap;
use std::io::Read as _;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, RawQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;

use crate::auth::session::Identity;
use crate::permissions::{for_request, ORG_NAMESPACE};
use crate::state::AppState;

/// Requests one person makes on the door in a minute: a clone or a fetch is two.
pub const REQUESTS_PER_MINUTE: usize = 60;
/// The largest upload-pack request read, inflated or not: its wants and haves, never a pack.
pub const MAX_REQUEST_BYTES: usize = 4 << 20;

/// The door, mounted on the Portal's public router; every other `/git` path is `404`.
pub fn router() -> Router<AppState> {
    let door = Router::new()
        .route("/{owner}/{repo}/info/refs", get(info_refs))
        .route(
            "/{owner}/{repo}/git-upload-pack",
            post(upload_pack).layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES)),
        )
        .method_not_allowed_fallback(not_found)
        .fallback(not_found);
    Router::new().nest("/git", door)
}

async fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

async fn info_refs(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    // The one query git sends here; anything else, a token included, is refused.
    if query.as_deref() != Some("service=git-upload-pack") {
        return not_found().await;
    }
    let Some(forge) = admitted(&state, &headers, &owner, &repo).await else {
        return not_found().await;
    };
    let answer = forge
        .upload_pack(
            "info/refs?service=git-upload-pack",
            protocol(&headers),
            None,
        )
        .await;
    relay(answer, "application/x-git-upload-pack-advertisement")
}

async fn upload_pack(
    State(state): State<AppState>,
    Path((owner, repo)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if query.is_some() {
        return not_found().await;
    }
    let Some(forge) = admitted(&state, &headers, &owner, &repo).await else {
        return not_found().await;
    };
    let body = match inflated(&headers, &body) {
        Ok(body) => body,
        Err(problem) => return problem,
    };
    let answer = forge
        .upload_pack("git-upload-pack", protocol(&headers), Some(body))
        .await;
    relay(answer, "application/x-git-upload-pack-result")
}

/// The forge client of `owner`/`repo` when the bearer's person may read it, `None` for every
/// refusal. Past the token, a person over [`REQUESTS_PER_MINUTE`] is refused too.
async fn admitted(
    state: &AppState,
    headers: &HeaderMap,
    owner: &str,
    repo: &str,
) -> Option<crate::forge::ShakoClient> {
    let name = repo.strip_suffix(".git").filter(|name| is_name(name))?;
    let forge = state.shako.as_ref()?;
    if owner != forge.owner {
        return None;
    }
    let token = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|token| !token.is_empty())?;
    let session = state.bearer.as_ref()?.verify(token).await.ok()?;
    if state.is_revoked(&session) {
        return None;
    }
    let identity = session.identity;
    if !within_rate(&identity.subject, Instant::now()) {
        tracing::info!(subject = %identity.subject, "git read door: over {REQUESTS_PER_MINUTE} requests a minute");
        return None;
    }
    readable(state, &identity, name).then(|| forge.for_repository(name))
}

/// Who reads repository `name` of the configuration owner: the installation repository its
/// platform administrators (an administrator of the whole installation, never one of an
/// Organization), a project repository the members of its project; no other repository.
pub fn readable(state: &AppState, identity: &Identity, name: &str) -> bool {
    let Some(forge) = state.shako.as_ref() else {
        return false;
    };
    if name == forge.repo {
        return for_request(state, identity, ORG_NAMESPACE).administers_organization();
    }
    state
        .mirror
        .repositories()
        .into_iter()
        .find(|(_, repository)| repository == name)
        .is_some_and(|(project, _)| for_request(state, identity, &project).may_read_project())
}

/// A forge repository name as the Portal creates them; anything else names no repository.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn within_rate(subject: &str, now: Instant) -> bool {
    static REQUESTS: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();
    crate::api::knowledge::counted(&REQUESTS, subject, now, Duration::from_secs(60))
        <= REQUESTS_PER_MINUTE
}

/// git protocol v2 asks for itself in this header; it is passed on when it is short and plain.
fn protocol(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("git-protocol")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 256)
}

/// The request body as the forge reads it: git gzips a request over 1 KiB, and the forge takes
/// plain bytes, so it is inflated here, never past [`MAX_REQUEST_BYTES`].
fn inflated(headers: &HeaderMap, body: &Bytes) -> Result<Vec<u8>, Response> {
    match headers
        .get(header::CONTENT_ENCODING)
        .map(HeaderValue::as_bytes)
    {
        None | Some(b"identity") => Ok(body.to_vec()),
        Some(b"gzip") | Some(b"x-gzip") => {
            let mut out = Vec::new();
            let read = flate2::read::GzDecoder::new(body.as_ref())
                .take(MAX_REQUEST_BYTES as u64 + 1)
                .read_to_end(&mut out);
            match read {
                Ok(_) if out.len() <= MAX_REQUEST_BYTES => Ok(out),
                Ok(_) => Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!("the request inflates past {MAX_REQUEST_BYTES} bytes"),
                )
                    .into_response()),
                Err(_) => Err((
                    StatusCode::BAD_REQUEST,
                    "the request body is not valid gzip",
                )
                    .into_response()),
            }
        }
        Some(_) => Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "the request body is sent plain or gzip",
        )
            .into_response()),
    }
}

/// The forge's answer streamed back: its `200` with `content_type`, a `404` for what it refuses
/// (the repository may be gone since the mirror read it), a `502` when it could not answer.
fn relay(
    answer: Result<reqwest::Response, crate::forge::GitError>,
    content_type: &'static str,
) -> Response {
    let response = match answer {
        Ok(response) => response,
        Err(e) => {
            tracing::warn!(error = %e, "git read door: the forge did not answer");
            return (
                StatusCode::BAD_GATEWAY,
                "the forge did not answer; try again",
            )
                .into_response();
        }
    };
    match response.status().as_u16() {
        200 => (
            [
                (header::CONTENT_TYPE, content_type),
                (
                    header::CACHE_CONTROL,
                    "no-cache, max-age=0, must-revalidate",
                ),
            ],
            Body::from_stream(response.bytes_stream()),
        )
            .into_response(),
        status if status >= 500 => {
            tracing::warn!(status, "git read door: the forge failed");
            (StatusCode::BAD_GATEWAY, "the forge failed; try again").into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_what_the_portal_creates() {
        for good in ["configuration", "ovzdusie-config", "a_b.c"] {
            assert!(is_name(good), "{good}");
        }
        for bad in ["", ".git", "..", "a/b", "a%2fb", "a b", &"x".repeat(101)] {
            assert!(!is_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_gzip_request_is_inflated_and_a_bomb_refused() {
        use std::io::Write as _;
        let gzip = |bytes: &[u8]| {
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(bytes).unwrap();
            Bytes::from(e.finish().unwrap())
        };
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        assert_eq!(
            inflated(&headers, &gzip(b"0009done\n")).unwrap(),
            b"0009done\n"
        );
        let bomb = gzip(&vec![0; MAX_REQUEST_BYTES + 1]);
        let refused = inflated(&headers, &bomb).unwrap_err();
        assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("br"));
        assert_eq!(
            inflated(&headers, &Bytes::new()).unwrap_err().status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }

    #[test]
    fn a_person_past_the_rate_is_refused_for_a_minute() {
        let now = Instant::now();
        for _ in 0..REQUESTS_PER_MINUTE {
            assert!(within_rate("f:1:rate-test", now));
        }
        assert!(!within_rate("f:1:rate-test", now));
        assert!(within_rate("f:1:someone-else", now));
        assert!(within_rate("f:1:rate-test", now + Duration::from_secs(61)));
    }
}
