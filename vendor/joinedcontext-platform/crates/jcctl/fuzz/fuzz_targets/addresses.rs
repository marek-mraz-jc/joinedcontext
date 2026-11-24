//! Input: an address as `--resource`, `--idm` or a token's destination would take it.
//! A slug that comes back names exactly one scope; an issuer never carries credentials.
#![no_main]

use jcctl::login::{endpoint_scope, endpoint_slug, issuer, token_may_travel};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(slug) = endpoint_slug(&text) {
        assert!(!slug.is_empty() && slug.len() <= 64, "slug {slug:?}");
        assert!(
            slug.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
            "slug {slug:?}"
        );
        assert_eq!(endpoint_scope(&slug).split(' ').count(), 2);
    }
    if let Ok(url) = issuer(&text) {
        assert!(url.username().is_empty() && url.password().is_none());
        assert!(url.host_str().is_some());
    }
    if let Ok(url) = url::Url::parse(&text) {
        if token_may_travel(&url) {
            assert!(matches!(url.scheme(), "https" | "http"), "{url}");
        }
    }
});
