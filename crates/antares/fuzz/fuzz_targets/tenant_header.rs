//! `NGSILD-Tenant`: never panics, and a tenant it accepts is a tenant name
//! a storage key can carry (T-3463).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // the API caps a query string and a header far below this, so no
    // allocation the target sees scales past what a request can carry
    if data.len() > 4096 {
        return;
    }
    let _ = antares_api::fuzzing::tenant_header(data);
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(tenant) = antares_model::TenantId::new(text) {
            assert!(
                tenant.as_str().bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                "{text:?} accepted as a tenant"
            );
        }
    }
});
