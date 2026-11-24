//! Input: an `Authorization` header value, then NUL, then `Cookie` header values split at NUL.
//! The bearer/DPoP split and the cookie readers end in a value or an error; an auth cookie never
//! yields more than AUTH_COOKIE_HANDLES handles, each 43 base64url characters.
#![no_main]

use http::{HeaderMap, HeaderValue, header};
use kaisatsu_oidc::{AUTH_COOKIE_HANDLES, add_auth_cookie, auth_cookie_holds, parse_authz_header, read_cookie};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut parts = data.split(|b| *b == 0);
    let mut headers = HeaderMap::new();
    if let Some(Ok(v)) = parts.next().map(HeaderValue::from_bytes) {
        headers.insert(header::AUTHORIZATION, v);
    }
    for raw in parts {
        if let Ok(v) = HeaderValue::from_bytes(raw) {
            headers.append(header::COOKIE, v);
        }
    }
    if let Ok((_, token)) = parse_authz_header(&headers) {
        assert!(!token.is_empty(), "empty token accepted");
    }
    let _ = read_cookie(&headers, "__Host-wk_sso");
    let handles = kaisatsu_oidc::handlers::auth_cookie_handles(&headers);
    assert!(handles.len() <= AUTH_COOKIE_HANDLES);
    assert!(handles.iter().all(|h| h.len() == 43));
    let _ = auth_cookie_holds(&headers, "0000");
    let set = add_auth_cookie(&headers, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", 60);
    assert!(set.contains("HttpOnly"), "auth cookie without HttpOnly: {set}");
});
