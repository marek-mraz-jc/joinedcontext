//! The API's `Authorization` and `X-Jc-Org` headers (split at the first NUL byte): a caller is the
//! admin only with the admin token, an organization token acts only for its own organization.
#![no_main]

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use ekiben_core::OrgId;
use ekiben_server::ServerAuthConfig;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut parts = data.splitn(2, |b| *b == 0);
    let auth = parts.next().unwrap_or_default();
    let org = parts.next();
    let mut headers = HeaderMap::new();
    let Ok(value) = HeaderValue::from_bytes(auth) else {
        return;
    };
    headers.insert("authorization", value);
    if let Some(org) = org {
        let Ok(value) = HeaderValue::from_bytes(org) else {
            return;
        };
        headers.insert("x-jc-org", value);
    }
    let config = ServerAuthConfig {
        admin_bearer_token: Some("admin-fuzz-token".to_string()),
        org_tokens: vec![("org-fuzz-token".to_string(), OrgId::new("org-a"))],
        ..Default::default()
    };
    let presented = auth
        .strip_prefix(b"Bearer ")
        .or_else(|| auth.strip_prefix(b"bearer "));
    match config.identify(&headers) {
        Ok(caller) if caller.admin => assert_eq!(presented, Some(&b"admin-fuzz-token"[..])),
        Ok(caller) => {
            assert_eq!(presented, Some(&b"org-fuzz-token"[..]));
            assert_eq!(caller.org, OrgId::new("org-a"));
        }
        Err(code) => assert!(matches!(
            code,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::BAD_REQUEST
        )),
    }
});
