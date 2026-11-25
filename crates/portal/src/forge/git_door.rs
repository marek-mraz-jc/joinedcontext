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

/// The door, mounted on the Portal's public router: another method on its two paths is `404`
/// too. jc and the edge send it nothing else under `/git`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/git/{owner}/{repo}/info/refs",
            get(info_refs).fallback(not_found),
        )
        .route(
            "/git/{owner}/{repo}/git-upload-pack",
            post(upload_pack)
                .fallback(not_found)
                .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES)),
        )
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
        Err(problem) => return problem.into_response(),
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
/// Organization), a project repository whoever reads all of it: a read grant on every kind it
/// holds, bound to no space and under no constraint; no other repository.
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
        .is_some_and(|(project, _)| {
            let kinds = state.mirror.kinds_in(&project);
            for_request(state, identity, &project)
                .reads_whole_project(kinds.iter().map(String::as_str))
        })
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
fn inflated(headers: &HeaderMap, body: &Bytes) -> Result<Vec<u8>, (StatusCode, String)> {
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
                )),
                Err(_) => Err((
                    StatusCode::BAD_REQUEST,
                    "the request body is not valid gzip".to_owned(),
                )),
            }
        }
        Some(_) => Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "the request body is sent plain or gzip".to_owned(),
        )),
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
        assert_eq!(refused.0, StatusCode::PAYLOAD_TOO_LARGE);
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("br"));
        assert_eq!(
            inflated(&headers, &Bytes::new()).unwrap_err().0,
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }

    mod door {
        use super::super::*;
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
        use p256::pkcs8::EncodePrivateKey;
        use serde_json::json;
        use std::sync::Arc;
        use tower::ServiceExt;
        use wiremock::matchers::{header as has_header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        const ISSUER: &str = "https://idm.example.test/realms/dev";

        /// A Portal whose forge is `forge` and whose bearer verifier trusts the returned key.
        fn portal(forge: &str) -> (AppState, EncodingKey) {
            let mut state = AppState::new(crate::config::Config::for_tests(), None);
            state.shako = Some(Arc::new(
                crate::forge::ShakoClient::new(
                    url::Url::parse(forge).expect("url"),
                    "joinedcontext",
                    "configuration",
                    "forge-token",
                )
                .expect("client"),
            ));
            let verifier = crate::auth::bearer::BearerVerifier::with_jwks_url(
                &url::Url::parse(ISSUER).expect("url"),
                "portal-api",
                url::Url::parse("http://127.0.0.1:9/certs").expect("url"),
            );
            let secret = p256::SecretKey::random(&mut rand_core::OsRng);
            let signer = EncodingKey::from_ec_der(secret.to_pkcs8_der().expect("der").as_bytes());
            let mut jwk: serde_json::Value =
                serde_json::from_str(&secret.public_key().to_jwk_string()).expect("jwk");
            jwk["kid"] = json!("k1");
            jwk["alg"] = json!("ES256");
            jwk["use"] = json!("sig");
            assert_eq!(
                verifier.install(&serde_json::from_value(json!({ "keys": [jwk] })).expect("set")),
                1
            );
            state.bearer = Some(Arc::new(verifier));
            state
                .mirror
                .set_repositories([("helsinki".to_owned(), "helsinki_repo".to_owned())].into());
            (state, signer)
        }

        /// A person's bearer: `sub`, with `roles` as realm roles.
        fn token(signer: &EncodingKey, sub: &str, roles: &[&str]) -> String {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_secs();
            let mut header = Header::new(Algorithm::ES256);
            header.kid = Some("k1".into());
            let claims = json!({
                "iss": ISSUER, "typ": "Bearer", "aud": "portal-api", "sub": sub, "azp": "jcctl",
                "exp": now + 300, "iat": now, "realm_access": { "roles": roles },
            });
            encode(&header, &claims, signer).expect("token")
        }

        async fn send(
            state: &AppState,
            request: axum::http::Request<Body>,
        ) -> (StatusCode, HeaderMap, String) {
            let response = router()
                .with_state(state.clone())
                .oneshot(request)
                .await
                .expect("a response");
            let (status, headers) = (response.status(), response.headers().clone());
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            (status, headers, String::from_utf8_lossy(&body).into_owned())
        }

        fn refs(uri: &str, bearer: Option<&str>) -> axum::http::Request<Body> {
            let mut request = axum::http::Request::get(uri).header("git-protocol", "version=2");
            if let Some(bearer) = bearer {
                request = request.header(header::AUTHORIZATION, bearer);
            }
            request.body(Body::empty()).expect("request")
        }

        #[tokio::test]
        async fn an_administrator_clones_the_installation_and_a_project_repository() {
            let forge = MockServer::start().await;
            for repo in ["configuration", "helsinki_repo"] {
                Mock::given(method("GET"))
                    .and(path(format!("/joinedcontext/{repo}.git/info/refs")))
                    .and(query_param("service", "git-upload-pack"))
                    .and(has_header("git-protocol", "version=2"))
                    .and(has_header("authorization", "Bearer forge-token"))
                    .respond_with(
                        ResponseTemplate::new(200).set_body_string(format!("refs of {repo}")),
                    )
                    .mount(&forge)
                    .await;
            }
            Mock::given(method("POST"))
                .and(path("/joinedcontext/configuration.git/git-upload-pack"))
                .respond_with(ResponseTemplate::new(200).set_body_string("PACK"))
                .mount(&forge)
                .await;
            let (state, signer) = portal(&forge.uri());
            let admin = format!(
                "Bearer {}",
                token(&signer, "f:1:admin-door", &["portal-approver"])
            );
            for repo in ["configuration", "helsinki_repo"] {
                let (status, headers, body) = send(
                    &state,
                    refs(
                        &format!("/git/joinedcontext/{repo}.git/info/refs?service=git-upload-pack"),
                        Some(&admin),
                    ),
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{repo}: {body}");
                assert_eq!(body, format!("refs of {repo}"));
                assert_eq!(
                    headers[header::CONTENT_TYPE],
                    "application/x-git-upload-pack-advertisement"
                );
                assert_eq!(
                    headers[header::CACHE_CONTROL],
                    "no-cache, max-age=0, must-revalidate"
                );
            }
            let pack =
                axum::http::Request::post("/git/joinedcontext/configuration.git/git-upload-pack")
                    .header(header::AUTHORIZATION, &admin)
                    .body(Body::from("0009done\n"))
                    .expect("request");
            let (status, headers, body) = send(&state, pack).await;
            assert_eq!((status, body.as_str()), (StatusCode::OK, "PACK"));
            assert_eq!(
                headers[header::CONTENT_TYPE],
                "application/x-git-upload-pack-result"
            );
            // A body the door cannot read is refused before the forge is asked.
            let garbled =
                axum::http::Request::post("/git/joinedcontext/configuration.git/git-upload-pack")
                    .header(header::AUTHORIZATION, &admin)
                    .header(header::CONTENT_ENCODING, "gzip")
                    .body(Body::from("not gzip"))
                    .expect("request");
            assert_eq!(send(&state, garbled).await.0, StatusCode::BAD_REQUEST);
        }

        #[tokio::test]
        async fn every_refusal_is_the_same_404_and_never_reaches_the_forge() {
            let forge = MockServer::start().await;
            let (state, signer) = portal(&forge.uri());
            let admin = format!(
                "Bearer {}",
                token(&signer, "f:1:admin-refused", &["portal-approver"])
            );
            let viewer = format!("Bearer {}", token(&signer, "f:1:viewer", &["viewer"]));
            let at = "/git/joinedcontext/configuration.git/info/refs";
            for (uri, bearer) in [
                (format!("{at}?service=git-upload-pack"), None),
                (format!("{at}?service=git-upload-pack"), Some("Basic abc")),
                (format!("{at}?service=git-upload-pack"), Some("Bearer ")),
                (
                    format!("{at}?service=git-upload-pack"),
                    Some("Bearer not-a-token"),
                ),
                (
                    format!("{at}?service=git-upload-pack"),
                    Some(viewer.as_str()),
                ),
                (
                    format!("{at}?service=git-receive-pack"),
                    Some(admin.as_str()),
                ),
                (
                    format!("{at}?service=git-upload-pack&access_token=x"),
                    Some(admin.as_str()),
                ),
                (at.to_owned(), Some(admin.as_str())),
                (
                    "/git/someone/configuration.git/info/refs?service=git-upload-pack".to_owned(),
                    Some(admin.as_str()),
                ),
                (
                    "/git/joinedcontext/configuration/info/refs?service=git-upload-pack".to_owned(),
                    Some(admin.as_str()),
                ),
                (
                    "/git/joinedcontext/unknown_repo.git/info/refs?service=git-upload-pack"
                        .to_owned(),
                    Some(admin.as_str()),
                ),
            ] {
                assert_eq!(
                    send(&state, refs(&uri, bearer)).await.0,
                    StatusCode::NOT_FOUND,
                    "{uri} {bearer:?}"
                );
            }
            // A push, a query on the pack path and another method are refused alike.
            for request in [
                axum::http::Request::post("/git/joinedcontext/configuration.git/git-receive-pack"),
                axum::http::Request::post(
                    "/git/joinedcontext/configuration.git/git-upload-pack?x=1",
                ),
                axum::http::Request::put("/git/joinedcontext/configuration.git/info/refs"),
            ] {
                let request = request
                    .header(header::AUTHORIZATION, &admin)
                    .body(Body::empty())
                    .expect("request");
                assert_eq!(send(&state, request).await.0, StatusCode::NOT_FOUND);
            }
            assert!(forge
                .received_requests()
                .await
                .expect("recorded")
                .is_empty());
            // A Portal without a forge reads no repository.
            let mut bare = state.clone();
            bare.shako = None;
            assert_eq!(
                send(
                    &bare,
                    refs(&format!("{at}?service=git-upload-pack"), Some(&admin))
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
            let anyone: crate::auth::session::Identity =
                serde_json::from_value(json!({ "subject": "f:1:x", "username": "x" }))
                    .expect("identity");
            assert!(!readable(&bare, &anyone, "configuration"));
        }

        /// A pack request without a bearer is the same 404, an Authorization header that is no
        /// text reads as none, and a person past the minute's rate is refused before the forge
        /// is asked.
        #[tokio::test]
        async fn a_bare_pack_an_opaque_header_and_a_busy_person_are_refused() {
            let forge = MockServer::start().await;
            let (state, signer) = portal(&forge.uri());
            let pack =
                axum::http::Request::post("/git/joinedcontext/configuration.git/git-upload-pack")
                    .body(Body::empty())
                    .expect("request");
            assert_eq!(send(&state, pack).await.0, StatusCode::NOT_FOUND);
            let at = "/git/joinedcontext/configuration.git/info/refs?service=git-upload-pack";
            let opaque = axum::http::Request::get(at)
                .header(
                    header::AUTHORIZATION,
                    axum::http::HeaderValue::from_bytes(b"Bearer \xff").expect("opaque"),
                )
                .body(Body::empty())
                .expect("request");
            assert_eq!(send(&state, opaque).await.0, StatusCode::NOT_FOUND);
            let busy = "f:1:admin-busy";
            let now = Instant::now();
            while within_rate(busy, now) {}
            let admin = format!("Bearer {}", token(&signer, busy, &["portal-approver"]));
            assert_eq!(
                send(&state, refs(at, Some(&admin))).await.0,
                StatusCode::NOT_FOUND
            );
            assert!(forge
                .received_requests()
                .await
                .expect("recorded")
                .is_empty());
        }

        #[tokio::test]
        async fn a_forge_that_fails_or_refuses_is_502_or_404() {
            let forge = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/joinedcontext/configuration.git/info/refs"))
                .respond_with(ResponseTemplate::new(503))
                .mount(&forge)
                .await;
            Mock::given(method("GET"))
                .and(path("/joinedcontext/helsinki_repo.git/info/refs"))
                .respond_with(ResponseTemplate::new(403))
                .mount(&forge)
                .await;
            let (state, signer) = portal(&forge.uri());
            let admin = format!(
                "Bearer {}",
                token(&signer, "f:1:admin-failing", &["portal-approver"])
            );
            let at = |repo: &str| {
                format!("/git/joinedcontext/{repo}.git/info/refs?service=git-upload-pack")
            };
            let (status, _, body) = send(&state, refs(&at("configuration"), Some(&admin))).await;
            assert_eq!(
                (status, body.as_str()),
                (StatusCode::BAD_GATEWAY, "the forge failed; try again")
            );
            assert_eq!(
                send(&state, refs(&at("helsinki_repo"), Some(&admin)))
                    .await
                    .0,
                StatusCode::NOT_FOUND
            );
            // A forge that does not answer at all: a bound socket that never listens.
            let socket = tokio::net::TcpSocket::new_v4().expect("socket");
            socket
                .bind("127.0.0.1:0".parse().expect("addr"))
                .expect("bind");
            let (closed, signer) =
                portal(&format!("http://{}", socket.local_addr().expect("addr")));
            let admin = format!(
                "Bearer {}",
                token(&signer, "f:1:admin-closed", &["portal-approver"])
            );
            let (status, _, body) = send(&closed, refs(&at("configuration"), Some(&admin))).await;
            assert_eq!(
                (status, body.as_str()),
                (
                    StatusCode::BAD_GATEWAY,
                    "the forge did not answer; try again"
                )
            );
            drop(socket);
        }
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
