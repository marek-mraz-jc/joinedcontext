// SPDX-License-Identifier: EUPL-1.2
//! `POST $batch` (odata-linkml-mapping.md section 6, W24 to W26): a
//! `multipart/mixed` or JSON batch read by `odata-batch`, each member then
//! served by the same router as a request of its own, so it is parsed,
//! refused and gated alone (O12) and runs as one NGSI-LD operation.
//!
//! The whole batch is refused before any member runs when a member URL
//! leaves this view's service root (O8), a member header is not an OData
//! one (O9: the tenant, the subject and the credentials are the outer
//! request's) or a bound is passed (O11). Members run in order. In a
//! multipart batch the first failure ends the batch unless `Prefer:
//! odata.continue-on-error` (Protocol 4.01 section 11.7.7.4); a JSON batch,
//! a 4.01 format, goes on unless `continue-on-error=false` (JSON Format 4.01
//! section 19.4). A member whose `dependsOn` failed answers `424` (section
//! 19.1).

use crate::query_in_body;
use antares_ops::bounds::{MAX_BATCH_ITEMS, MAX_BODY_BYTES};
use antares_ops::Ops;
use axum::extract::Request;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE, HOST};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::Router;
use odata_axum::ODataErrorResponse;
use odata_batch::{BatchError, Limits};
use odata_json::ODataVersion;
use std::sync::atomic::{AtomicU64, Ordering};
use tower::ServiceExt as _;

/// The member headers a batch passes on (O9); the parser already took the
/// part's own `Content-Type`, `Content-ID` and `Content-Transfer-Encoding`.
const MEMBER_HEADERS: [&str; 8] = [
    "accept",
    "content-type",
    "content-length",
    "if-match",
    "if-none-match",
    "prefer",
    "odata-version",
    "odata-maxversion",
];

/// The outer headers a member does not inherit: they describe the batch,
/// not the member.
const BATCH_ONLY: [&str; 6] = [
    "content-type",
    "content-length",
    "accept",
    "prefer",
    "if-match",
    "if-none-match",
];

/// `POST {root}$batch`. `routed` serves a member at `/{view}/odata/v4/…`
/// (`inner_root`); `service_root` is the root a client writes.
pub(crate) async fn serve(
    routed: &Router,
    ops: &Ops,
    (service_root, inner_root): (&str, &str),
    req: Request,
    max_query_len: usize,
) -> Response {
    let (parts, body) = req.into_parts();
    let header = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok());
    let Ok(version) =
        odata_json::negotiate_version(header("OData-Version"), header("OData-MaxVersion"))
    else {
        return bad("the OData-Version headers name no version this service answers in")
            .into_response();
    };
    let content_type = header("Content-Type").unwrap_or_default().to_owned();
    let json = is_json(&content_type);
    // Only a client that caps the version refuses 4.01; without the
    // headers the version is 4.0 and a JSON batch still reads.
    if json && version == ODataVersion::V4_0 && header("OData-MaxVersion").is_some() {
        return bad("a JSON batch is an OData 4.01 format; OData-MaxVersion 4.0 asks for 4.0")
            .with_version(version)
            .into_response();
    }
    let preference = parts
        .headers
        .get_all("Prefer")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .find_map(continues);
    let continue_on_error = preference.unwrap_or(json);
    let host = parts
        .uri
        .authority()
        .map(|a| a.as_str().to_owned())
        .or_else(|| header(HOST.as_str()).map(str::to_owned));
    let limits = Limits {
        max_body_bytes: *MAX_BODY_BYTES,
        max_requests: Limits::strict().max_requests.min(*MAX_BATCH_ITEMS),
        ..Limits::strict()
    };
    // The bounds wall answered a longer body `413` already; a body that
    // breaks off is read as the empty batch it then is, `400`.
    let bytes = axum::body::to_bytes(body, limits.max_body_bytes)
        .await
        .unwrap_or_default();
    let parsed = if json {
        odata_batch::parse_json(&bytes, &limits)
    } else {
        odata_batch::boundary(&content_type)
            .and_then(|b| odata_batch::parse_multipart(&bytes, &b, &limits))
    };
    let batch = match parsed {
        Ok(batch) => batch,
        Err(e) => return refused(&e, version),
    };
    let mut checked = Vec::with_capacity(batch.requests.len());
    for member in &batch.requests {
        let Some(rest) = below(&member.url, service_root, host.as_deref()) else {
            return bad("a batch member names a resource outside this service").into_response();
        };
        let Some(headers) = member
            .headers
            .iter()
            .map(|(name, value)| allowed(name, value))
            .collect::<Option<Vec<_>>>()
        else {
            return bad("a batch member carries a header a member may not set").into_response();
        };
        checked.push((format!("{inner_root}{rest}"), headers));
    }
    let mut answers: Vec<odata_batch::Response> = Vec::with_capacity(checked.len());
    // Whether each member that ran failed; `dependsOn` names earlier ones.
    let mut failed: Vec<bool> = Vec::with_capacity(checked.len());
    // the member answers buffered so far share `max_answer_bytes` (F-ANT4-004)
    let mut left = ops.max_answer_bytes();
    for (member, (target, headers)) in batch.requests.into_iter().zip(checked) {
        let id = member.id.clone();
        let skip = skipped(&member.depends_on, &failed, left).await;
        let answer = if let Some(answer) = skip {
            answer
        } else {
            let budget = (max_query_len, &mut left);
            run(routed, ops, &parts, member, (&target, headers), budget).await
        };
        let failure = answer.status >= 400;
        answers.push(odata_batch::Response { id, ..answer });
        failed.push(failure);
        if failure && !continue_on_error {
            break;
        }
    }
    written(&answers, json, preference == Some(true), version)
}

/// The answer of a member that is not run: 413 once the batch's answer bound
/// is spent (F-ANT4-004), 424 when a member it depends on failed. Either is
/// reached only under `continue-on-error`: a failure stops the batch otherwise.
async fn skipped(
    depends_on: &[usize],
    failed: &[bool],
    left: usize,
) -> Option<odata_batch::Response> {
    if left == 0 {
        return Some(member_too_large().await);
    }
    depends_on
        .iter()
        .any(|&d| failed.get(d) == Some(&true))
        .then(|| odata_batch::Response {
            id: None,
            status: 424,
            headers: Vec::new(),
            body: Vec::new(),
        })
}

/// One member as a request of its own: the outer request's headers (the
/// tenant, the subject, the credentials) with the member's OData headers
/// over them.
async fn run(
    routed: &Router,
    ops: &Ops,
    outer: &axum::http::request::Parts,
    member: odata_batch::Request,
    (target, own): (&str, Vec<(HeaderName, HeaderValue)>),
    (max_query_len, left): (usize, &mut usize),
) -> odata_batch::Response {
    let mut headers = outer.headers.clone();
    for name in BATCH_ONLY {
        headers.remove(name);
    }
    for (name, value) in own {
        headers.insert(name, value);
    }
    if !member.body.is_empty() {
        headers.insert(CONTENT_LENGTH, HeaderValue::from(member.body.len()));
        headers
            .entry(CONTENT_TYPE)
            .or_insert(HeaderValue::from_static("application/json"));
    }
    let mut req = Request::new(axum::body::Body::from(member.body));
    *req.method_mut() = match member.method {
        odata_batch::Method::Get => Method::GET,
        odata_batch::Method::Post => Method::POST,
        odata_batch::Method::Put => Method::PUT,
        odata_batch::Method::Patch => Method::PATCH,
        odata_batch::Method::Delete => Method::DELETE,
    };
    *req.headers_mut() = headers;
    *req.extensions_mut() = outer.extensions.clone();
    let answer = match target.parse() {
        Ok(uri) => {
            *req.uri_mut() = uri;
            match query_in_body(req, max_query_len).await {
                Ok(req) => {
                    let Ok(resp) = routed.clone().oneshot(req).await;
                    resp
                }
                Err(refused) => refused.into_response(),
            }
        }
        Err(_) => bad("a batch member URL is not a URL").into_response(),
    };
    // A member's answer is buffered to be framed; past the broker's bound
    // the member answers 413 instead (T-3773) and buffers nothing. One that
    // fits alone but passes what the batch's bound has left answers 413 and
    // spends the bound, so every later member answers 413 too.
    let (parts, body) = answer.into_parts();
    let body = match ops.read_answer(body, "odata").await {
        Ok(body) => body,
        // the broker's buffered-answer pool is full: the member is refused
        // whole with a 503 that says when to ask again (T-5895)
        Err(antares_ops::Refusal::Overloaded { .. }) => {
            return member_refused(
                StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailable",
                antares_ops::bounds::MEMORY_OVERLOADED,
            )
            .await
        }
        Err(_) => return member_too_large().await,
    };
    let Some(rest) = left.checked_sub(body.len()) else {
        *left = 0;
        return member_too_large().await;
    };
    *left = rest;
    framed(&parts, &body)
}

/// The 413 a member answers past the buffer bound.
async fn member_too_large() -> odata_batch::Response {
    member_refused(
        StatusCode::PAYLOAD_TOO_LARGE,
        "PayloadTooLarge",
        antares_ops::bounds::ANSWER_TOO_LARGE,
    )
    .await
}

/// A member refused with `status` and its fixed text; a 503 says when to
/// ask again. The error document is the broker's own few hundred bytes, read
/// outside the pool it may have found full.
async fn member_refused(status: StatusCode, code: &str, message: &str) -> odata_batch::Response {
    let mut answer = ODataErrorResponse::new(status, code, message).into_response();
    if status == StatusCode::SERVICE_UNAVAILABLE {
        answer.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            antares_ops::bounds::ANSWER_POOL_RETRY_SECONDS.into(),
        );
    }
    let (parts, body) = answer.into_parts();
    let body = axum::body::to_bytes(body, 64 * 1024)
        .await
        .unwrap_or_default();
    framed(&parts, &body)
}

/// A member's answer as the batch frames it.
fn framed(parts: &axum::http::response::Parts, body: &[u8]) -> odata_batch::Response {
    let status = parts.status.as_u16();
    let headers = parts
        .headers
        .iter()
        .filter(|(name, _)| *name != CONTENT_LENGTH)
        .map(|(name, value)| {
            let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
            (name.as_str().to_owned(), value)
        })
        .collect();
    let body = body.to_vec();
    odata_batch::Response {
        id: None,
        status,
        headers,
        body,
    }
}

/// The path below the service root a member URL names (Protocol 11.7.7:
/// relative, an absolute path or an absolute URI of this host), or `None`
/// for any other: another host, another view, another root, a `.` or
/// `..` segment, an encoded `/` or `\`.
fn below<'u>(url: &'u str, service_root: &str, host: Option<&str>) -> Option<&'u str> {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    let scheme = ["https://", "http://"]
        .into_iter()
        .find(|s| lower.starts_with(s));
    let path = match scheme {
        Some(scheme) => {
            let rest = url.get(scheme.len()..).unwrap_or_default();
            let (authority, path) = rest.split_at(rest.find('/')?);
            if !host.is_some_and(|h| h.eq_ignore_ascii_case(authority)) {
                return None;
            }
            path
        }
        None => url,
    };
    let rest = match path.strip_prefix('/') {
        Some(_) => path.strip_prefix(service_root)?,
        // A scheme this service does not answer (`urn:`, `file:`) is no
        // relative URL; a key's `:` sits after its `(`.
        None if path
            .split(['/', '(', '?'])
            .next()
            .unwrap_or_default()
            .contains(':') =>
        {
            return None
        }
        None => path,
    };
    let route = rest.split('?').next().unwrap_or_default();
    let lowered = route.to_ascii_lowercase();
    let traverses = route.split('/').any(|s| {
        matches!(
            s.to_ascii_lowercase().replace("%2e", ".").as_str(),
            "." | ".."
        )
    });
    if traverses || lowered.contains("%2f") || lowered.contains("%5c") || route.contains('\\') {
        return None;
    }
    Some(rest)
}

/// The header a member may set (O9): an OData one, a `Prefer` only with
/// OData preferences; `None` for any other.
fn allowed(name: &str, value: &str) -> Option<(HeaderName, HeaderValue)> {
    let name = HeaderName::try_from(name).ok()?;
    let odata_only = value.split(',').all(|p| {
        let token = p.split(['=', ';']).next().unwrap_or_default().trim();
        token.eq_ignore_ascii_case("return") || token.to_ascii_lowercase().starts_with("odata.")
    });
    let kept = MEMBER_HEADERS.contains(&name.as_str()) && (name != "prefer" || odata_only);
    kept.then(|| HeaderValue::try_from(value).ok())
        .flatten()
        .map(|value| (name, value))
}

/// The value of one preference that is `odata.continue-on-error` (4.0) or
/// `continue-on-error` (4.01): `false` only when set to `false`.
fn continues(preference: &str) -> Option<bool> {
    let (name, value) = preference.split_once('=').unwrap_or((preference, "true"));
    let name = name.trim().to_ascii_lowercase();
    matches!(
        name.as_str(),
        "odata.continue-on-error" | "continue-on-error"
    )
    .then(|| !value.trim().eq_ignore_ascii_case("false"))
}

fn is_json(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
}

/// The batch response, in the format of the request.
fn written(
    answers: &[odata_batch::Response],
    json: bool,
    continue_on_error: bool,
    version: ODataVersion,
) -> Response {
    let mut body = Vec::new();
    let content_type = if json {
        odata_batch::write_json(answers, &mut body)
            .ok()
            .map(|()| "application/json".to_owned())
    } else {
        boundaries().find_map(|b| {
            body.clear();
            odata_batch::write_multipart(answers, &b, &mut body)
                .ok()
                .map(|()| format!("multipart/mixed; boundary={b}"))
        })
    };
    let Some(content_type) = content_type else {
        return ODataErrorResponse::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            "the batch response cannot be written",
        )
        .with_version(version)
        .into_response();
    };
    let mut response = (StatusCode::OK, [(CONTENT_TYPE, content_type)], body).into_response();
    let headers = response.headers_mut();
    headers.insert("OData-Version", HeaderValue::from_static(version.as_str()));
    if continue_on_error {
        headers.insert(
            "Preference-Applied",
            HeaderValue::from_static("odata.continue-on-error"),
        );
    }
    response
}

/// Fresh response boundaries: one a member's body does not hold is taken.
fn boundaries() -> impl Iterator<Item = String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    (0..3).map(move |_| {
        format!(
            "batchresponse_{nanos:x}_{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    })
}

fn refused(error: &BatchError, version: ODataVersion) -> Response {
    if let BatchError::NotImplemented(feature) = error {
        return crate::read::not_implemented(feature.as_str(), version).into_response();
    }
    let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::BAD_REQUEST);
    // `BadRequest`, `NotImplemented`, `PayloadTooLarge`.
    let code = status
        .canonical_reason()
        .unwrap_or_default()
        .replace(' ', "");
    ODataErrorResponse::new(status, &code, &error.to_string())
        .with_version(version)
        .into_response()
}

fn bad(message: &str) -> ODataErrorResponse {
    ODataErrorResponse::new(StatusCode::BAD_REQUEST, "BadRequest", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/x/views/cars/odata/v4/";

    #[test]
    fn a_member_url_stays_below_this_service_root() {
        let host = Some("broker:8080");
        for (url, want) in [
            ("Car('urn:ngsi-ld:Car:1')", Some("Car('urn:ngsi-ld:Car:1')")),
            ("/x/views/cars/odata/v4/Car?$top=1", Some("Car?$top=1")),
            ("http://broker:8080/x/views/cars/odata/v4/Car", Some("Car")),
            ("HTTPS://BROKER:8080/x/views/cars/odata/v4/", Some("")),
            ("http://evil/x/views/cars/odata/v4/Car", None),
            ("http://broker:8080", None),
            ("/x/views/other/odata/v4/Car", None),
            ("/q/health", None),
            ("/ngsi-ld/v1/entities", None),
            ("//evil/Car", None),
            ("../other/odata/v4/Car", None),
            ("Car/%2E%2e/x", None),
            ("Car('a%2Fb')", None),
            ("Car('a%5cb')", None),
            ("Car('a\\b')", None),
            ("urn:x", None),
            ("Car?$filter=x eq '..'", Some("Car?$filter=x eq '..'")),
        ] {
            assert_eq!(below(url, ROOT, host), want, "{url}");
        }
        assert_eq!(
            below("http://broker:8080/x/views/cars/odata/v4/Car", ROOT, None),
            None
        );
    }

    #[test]
    fn a_member_sets_only_odata_headers() {
        for (name, value, want) in [
            ("Accept", "application/json", true),
            ("OData-MaxVersion", "4.01", true),
            ("Prefer", "return=minimal, odata.maxpagesize=5", true),
            ("Prefer", "ngsi-ld=x", false),
            ("Prefer", "respond-async", false),
            ("NGSILD-Tenant", "other", false),
            ("Authorization", "Bearer x", false),
            ("Link", "<x>", false),
            ("Accept", "a\u{7f}", false),
            ("Bad Name", "x", false),
        ] {
            assert_eq!(allowed(name, value).is_some(), want, "{name}: {value}");
        }
    }

    #[test]
    fn an_answer_no_writer_can_frame_is_500() {
        let answer = |headers: Vec<(String, String)>, body: &[u8]| odata_batch::Response {
            id: None,
            status: 200,
            headers,
            body: body.to_vec(),
        };
        let binary = answer(vec![("Content-Type".into(), "image/png".into())], &[0xff]);
        let broken = answer(vec![("X".into(), "a\r\nb".into())], b"");
        for (answers, json) in [(vec![binary], true), (vec![broken], false)] {
            let response = written(&answers, json, false, ODataVersion::V4_01);
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    #[test]
    fn continue_on_error_is_read_in_both_spellings() {
        assert_eq!(continues("odata.continue-on-error"), Some(true));
        assert_eq!(continues(" continue-on-error=true"), Some(true));
        assert_eq!(continues("continue-on-error=false"), Some(false));
        assert_eq!(continues("return=minimal"), None);
    }
}
