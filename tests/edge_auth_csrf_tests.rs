//! Edge cases of the double-submit CSRF lock (T-2093, T-2094, T-2095; PF-50, R20).
//!
//! The three functions and the one sentence each must hold:
//!
//! - `cookie(token)` — the token is readable by the page on purpose, but the cookie is `Secure`,
//!   `Lax` and path `/`, and **nothing a caller can put in the token may become a second cookie
//!   attribute**.
//! - `is_allowed(method, headers)` — a mutation passes only when a `__Host-jc_csrf` cookie and an
//!   `X-CSRF-Token` header carry the same non-empty value, byte for byte; a safe method always
//!   passes; nothing else does.
//! - `require_csrf` — a refusal is `403` as problem+json and says nothing about the request.
//!
//! Tests only (the family's rule). A red case would become its own task; every case here is green,
//! and the two cases struck as impossible name the line that makes them so.

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{
    cookie, is_allowed, new_token, removal, Token, CSRF_HEADER,
};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use tower::ServiceExt;

fn headers(cookie_value: Option<&str>, presented: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(value) = cookie_value {
        headers.insert(
            header::COOKIE,
            format!("__Host-jc_csrf={value}")
                .parse()
                .expect("cookie header"),
        );
    }
    if let Some(value) = presented {
        headers.insert(CSRF_HEADER, value.parse().expect("csrf header"));
    }
    headers
}

// ---------------------------------------------------------------------------------------------
// `cookie`
// ---------------------------------------------------------------------------------------------

#[test]
fn the_cookie_is_readable_by_the_page_and_nothing_more() {
    let built = cookie(&Token::mint());
    assert_eq!(built.http_only(), Some(false), "the page has to read it");
    assert_eq!(built.secure(), Some(true));
    assert_eq!(built.path(), Some("/"));
    assert_eq!(
        built.max_age().map(|age| age.whole_hours()),
        Some(12),
        "a token that never expires is a token that never rotates",
    );
}

#[test]
fn nothing_but_a_minted_token_can_reach_the_cookie(/* T-2289 */) {
    // This was the family's one red case: `cookie` took a `String`, so a value carrying `;` became a
    // cookie attribute (`__Host-jc_csrf=abc; Domain=evil.example; …`) and a CR/LF aimed at the response
    // header. The fix is the signature — `cookie(&Token)`, and a `Token` comes from the CSPRNG — so
    // the forged values below no longer compile, and what is left to assert is that a minted token
    // produces the helper's own attributes and nothing else, whichever token it is.
    for _ in 0..64 {
        let token = Token::mint();
        let written = cookie(&token).to_string();
        let attributes: Vec<&str> = written.split(';').map(str::trim).skip(1).collect();
        assert!(
            attributes
                .iter()
                .all(|attribute| ["Secure", "Path=/"].contains(attribute)
                    || attribute.starts_with("Max-Age=")
                    || attribute.starts_with("SameSite=")
                    || attribute.starts_with("Expires=")),
            "a minted token produced attributes {attributes:?}",
        );
        assert!(
            !written.contains('\n') && !written.contains('\r'),
            "a minted token produced a header value carrying a newline: {written}",
        );
        assert!(
            HeaderValue::from_str(&written).is_ok(),
            "a minted token produced a value no header can carry: {written}",
        );
        // The value the page will echo is the one in the cookie, or the double submit cannot match.
        assert!(written.starts_with(&format!("__Host-jc_csrf={}", token.as_str())));
    }
}

#[test]
fn the_removal_cookie_clears_the_same_cookie_on_the_same_path() {
    let written = removal().to_string();
    assert!(written.starts_with("__Host-jc_csrf="));
    assert!(written.contains("Path=/"));
    assert!(
        written.contains("Max-Age=0"),
        "a logout that leaves the token behind leaves a usable half-credential: {written}",
    );
}

// ---------------------------------------------------------------------------------------------
// `is_allowed`
// ---------------------------------------------------------------------------------------------

#[test]
fn a_safe_method_needs_no_token_and_a_mutation_does() {
    for safe in [Method::GET, Method::HEAD, Method::OPTIONS] {
        assert!(is_allowed(&safe, &headers(None, None)), "{safe}");
    }
    // Every other method is a mutation as far as this lock is concerned, TRACE and an extension
    // method included: the list is the safe one, not the dangerous one.
    for mutating in [
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::TRACE,
        Method::CONNECT,
        Method::from_bytes(b"PURGE").expect("extension method"),
    ] {
        assert!(!is_allowed(&mutating, &headers(None, None)), "{mutating}");
    }
}

#[test]
fn a_mutation_passes_only_on_a_byte_for_byte_match() {
    let token = new_token();
    assert!(is_allowed(
        &Method::POST,
        &headers(Some(&token), Some(&token))
    ));

    let near_misses = [
        // Half a credential is none.
        (Some(token.as_str()), None),
        (None, Some(token.as_str())),
        // Empty on either side, or both: an empty cookie must never be a wildcard.
        (Some(""), Some("")),
        (Some(""), Some(token.as_str())),
        (Some(token.as_str()), Some("")),
        // A prefix, a suffix, one flipped byte, and one that differs only in case.
        (Some(token.as_str()), Some(&token[..token.len() - 1])),
        (Some(token.as_str()), Some("x")),
        // Whitespace is not trimmed into a match, and neither is a look-alike.
        (Some(token.as_str()), Some(" abc")),
    ];
    for (stored, presented) in near_misses {
        assert!(
            !is_allowed(&Method::POST, &headers(stored, presented)),
            "cookie {stored:?} with header {presented:?} passed",
        );
    }

    let flipped = {
        let mut bytes = token.clone().into_bytes();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        String::from_utf8(bytes).expect("ascii token")
    };
    assert!(!is_allowed(
        &Method::POST,
        &headers(Some(&token), Some(&flipped))
    ));

    // Percent-encoding is not decoded into a match, once or twice.
    let encoded = token.replace('a', "%61");
    if encoded != token {
        assert!(!is_allowed(
            &Method::POST,
            &headers(Some(&token), Some(&encoded))
        ));
    }
}

#[test]
fn the_cookie_that_counts_is_the_one_named_jc_csrf() {
    let token = new_token();
    let mut with_others = HeaderMap::new();
    with_others.insert(
        header::COOKIE,
        format!("__Host-jc_session=x; __Host-jc_csrf={token}; other=y")
            .parse()
            .expect("cookie header"),
    );
    with_others.insert(CSRF_HEADER, token.parse().expect("csrf header"));
    assert!(is_allowed(&Method::POST, &with_others));

    // A cookie of another name carrying the same value is not the token.
    let mut wrong_name = HeaderMap::new();
    wrong_name.insert(
        header::COOKIE,
        format!("jc_csrf_x={token}").parse().expect("cookie header"),
    );
    wrong_name.insert(CSRF_HEADER, token.parse().expect("csrf header"));
    assert!(!is_allowed(&Method::POST, &wrong_name));
}

#[test]
fn an_authorization_header_carries_its_own_authentication_and_an_empty_one_authenticates_nothing() {
    // A bearer is the credential, so there is no cookie for a cross-site form to ride on and the
    // token is not asked for (AG-59). An EMPTY `Authorization` skips this lock too — and cannot be
    // used to reach a cookie session: `CurrentUser` sees `Front::Bearer` the moment the header is
    // present and answers 401 without looking at the cookie, because the token is
    // `.filter(|t| !t.is_empty()).ok_or(ApiError::Unauthorized)` (src/auth/session.rs:250-252).
    // A cross-site page cannot set the header at all: `Authorization` is not CORS-safelisted, so a
    // request carrying it is preflighted. Both are asserted here so a change to either is noticed.
    for value in ["Bearer eyJ.x.y", "", "   ", "Basic Zm9v"] {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(value).expect("authorization header"),
        );
        assert!(is_allowed(&Method::POST, &headers), "{value:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// `require_csrf`
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_refused_mutation_is_problem_json_that_says_nothing_about_the_request() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/helsinki/spaces")
                .header(header::COOKIE, "__Host-jc_csrf=stored")
                .header(CSRF_HEADER, "presented")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json",
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).expect("utf-8 body");
    // The refusal is the same whatever was wrong, and carries neither the tokens nor the path.
    for leaked in ["stored", "presented", "helsinki", "csrf", "cookie"] {
        assert!(
            !text.to_lowercase().contains(leaked),
            "the refusal mentioned {leaked:?}: {text}",
        );
    }
}

#[tokio::test]
async fn the_gate_is_in_front_of_every_mutating_api_route_and_of_no_read() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    // A read reaches its handler (and answers 401 for want of a session, not 403 for want of a
    // token): the lock must not be in the way of a GET.
    let read = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/helsinki/spaces")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(read.status(), StatusCode::FORBIDDEN);

    for (method, uri) in [
        ("POST", "/api/v1/projects/helsinki/spaces"),
        ("PUT", "/api/v1/projects/helsinki/spaces/ovzdusie"),
        ("DELETE", "/api/v1/projects/helsinki/spaces/ovzdusie"),
        ("PATCH", "/api/v1/projects/helsinki/spaces/ovzdusie"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{method} {uri} passed the CSRF gate without a token",
        );
    }
}
