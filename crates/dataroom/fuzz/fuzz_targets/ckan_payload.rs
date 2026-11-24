//! Input: a CKAN payload as `package_create`, `resource_create` and `resource_view_create` take
//! it. The readers keep what they can and drop the rest: never a panic, and what they build must
//! go back out through the CKAN writer.
#![no_main]

use std::collections::BTreeMap;

use dataroom_core::ckan_compat::{
    dataset_from_ckan, dataset_to_ckan, distribution_from_ckan, distribution_to_ckan,
    resource_view_from_ckan, resource_view_to_ckan, validate_view_payload,
};
use dataroom_core::{CatalogueId, OrgId};
use libfuzzer_sys::fuzz_target;
use uuid::Uuid;

fuzz_target!(|data: &[u8]| {
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    let org = OrgId::from_uuid(Uuid::from_u128(1));
    let cat = CatalogueId::from_uuid(Uuid::from_u128(2));
    let ds = dataset_from_ckan(org, cat, &payload, None, "sk");
    let again = dataset_from_ckan(org, cat, &payload, Some(&ds), "en");
    let out = dataset_to_ckan(&again, None, &BTreeMap::new(), "sk");
    assert!(out.is_object());
    let dist = distribution_from_ckan(&payload, "ds-1", "fallback", "sk");
    let _ = distribution_to_ckan(&dist, "ds-1", 0, &[], "sk");
    if validate_view_payload(&payload).is_ok() {
        let view = resource_view_from_ckan(&payload, "view-1", "res-1", "image_view");
        let _ = resource_view_to_ckan(&view);
    }
});
