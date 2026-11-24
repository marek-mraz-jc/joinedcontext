//! The anonymous double-submit check over an arbitrary `Cookie` line and `X-CSRF-Token`
//! (`auth::csrf::double_submitted`, `is_allowed`): never panics, and a pass means the header is
//! exactly the non-empty `__Host-jc_csrf` value.
#![no_main]
use axum::http::{header, HeaderMap, HeaderValue, Method};
use joinedcontext_portal::auth::csrf::{double_submitted, is_allowed, CSRF_COOKIE, CSRF_HEADER};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 8 * 1024 {
        return;
    }
    // The first 0xff byte (never valid in a header) splits the cookie line from the token.
    let (cookie, token) = match data.iter().position(|b| *b == 0xff) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &data[..0]),
    };
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_bytes(cookie) {
        headers.insert(header::COOKIE, value);
    }
    if let Ok(value) = HeaderValue::from_bytes(token) {
        headers.insert(CSRF_HEADER, value);
    }
    let passed = double_submitted(&headers, None);
    assert_eq!(passed, is_allowed(&Method::POST, &headers, None));
    if passed {
        let presented = headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok());
        let jar = last_csrf_cookie(&headers);
        assert!(presented.is_some_and(|p| !p.is_empty() && Some(p) == jar.as_deref()));
    }
});

/// The last `__Host-jc_csrf` value of the `Cookie` line that parses, each pair read by the cookie
/// crate as the jar reads it (RFC 6265 trimming, percent-decoded) and not by the jar itself.
fn last_csrf_cookie(headers: &HeaderMap) -> Option<String> {
    let line = headers.get(header::COOKIE)?.to_str().ok()?;
    line.split(';')
        .filter_map(|pair| cookie::Cookie::parse_encoded(pair).ok())
        .filter(|c| c.name() == CSRF_COOKIE)
        .map(|c| c.value().to_owned())
        .last()
}
