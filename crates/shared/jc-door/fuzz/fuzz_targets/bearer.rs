//! Any bytes as an Authorization header: `bearer` answers a trimmed, non-empty value no longer
//! than MAX_TOKEN_BYTES, or nothing, and never panics (T-7766).
#![no_main]

use http::{header, HeaderMap, HeaderValue};
use jc_door::{bearer, MAX_TOKEN_BYTES};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(value) = HeaderValue::from_bytes(data) else {
        return;
    };
    let mut headers = HeaderMap::new();
    headers.insert(header::AUTHORIZATION, value);
    if let Some(token) = bearer(&headers) {
        assert!(!token.is_empty() && token.len() <= MAX_TOKEN_BYTES);
        assert_eq!(token, token.trim());
    }
});
