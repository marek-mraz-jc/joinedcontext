//! Input: the client_id URL the metadata was fetched from, then NUL, then the fetched CIMD
//! document (JSON). A built client is always a public client that asks consent, never one with
//! the full scope, and its client_id is the fetched URL.
#![no_main]

use kaisatsu_core::RealmConfig;
use kaisatsu_oidc::{validate_and_build_cimd_client, CimdMetadata};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let (client_id, doc) = text
        .split_once('\0')
        .unwrap_or(("https://app.example/cimd", &text));
    let Ok(doc) = serde_json::from_str::<CimdMetadata>(doc) else {
        return;
    };
    let realm = RealmConfig::default();
    if let Ok(client) = validate_and_build_cimd_client(&realm, client_id, doc) {
        assert_eq!(client.client_id, client_id);
        assert!(client.consent_required && !client.full_scope_allowed);
    }
});
