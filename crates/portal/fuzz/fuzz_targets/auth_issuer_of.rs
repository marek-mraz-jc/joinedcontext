//! The unverified `iss` of a presented bearer token, read to pick the realm that verifies it
//! (`auth::realms::issuer_of`): never panics, and an issuer it names never ends in `/`.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // An `Authorization` header is capped far below this by the edge and hyper.
    if data.len() > 16 * 1024 {
        return;
    }
    let Ok(token) = std::str::from_utf8(data) else {
        return;
    };
    if let Some(issuer) = joinedcontext_portal::auth::realms::issuer_of(token) {
        assert!(!issuer.ends_with('/'), "{issuer:?}");
    }
});
