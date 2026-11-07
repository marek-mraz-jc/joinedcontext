// SPDX-License-Identifier: EUPL-1.2
//! The antares-model entry checks (T-3912): an Entity id, a tenant name, an Attribute name and a
//! percent-decoded path segment answer Ok or a typed 400, never a panic, whatever the bytes.
#![no_main]
use antares_model::{check_attr_name, EntityId, TenantId};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let decoded = antares_ql::percent_decode(data);
    for s in [decoded.as_str(), std::str::from_utf8(data).unwrap_or("")] {
        if let Ok(id) = EntityId::new(s) {
            assert_eq!(id.as_str(), s);
        }
        if let Ok(t) = TenantId::new(s) {
            assert!(t.as_str().len() <= TenantId::MAX_LEN);
        }
        if let Err(e) = check_attr_name(s) {
            assert_eq!(e.status(), 400);
        }
    }
});
