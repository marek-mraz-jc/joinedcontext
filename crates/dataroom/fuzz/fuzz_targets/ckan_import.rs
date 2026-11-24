//! Input: a whole CKAN JSON dump, as `dataroom import-ckan` reads it from a file, imported into a
//! memory store by an admin. The import answers counts or a validation error, never a panic, and
//! never more datasets than the dump holds.
#![no_main]

use dataroom_core::{CatalogueId, OrgId, RequestCtx, Subject};
use dataroom_server::import_ckan_json;
use dataroom_store_memory::MemoryStore;
use libfuzzer_sys::fuzz_target;
use uuid::Uuid;

fuzz_target!(|data: &[u8]| {
    let Ok(raw) = std::str::from_utf8(data) else {
        return;
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let org = OrgId::from_uuid(Uuid::from_u128(1));
    let ctx = RequestCtx::new(
        org,
        CatalogueId::from_uuid(Uuid::from_u128(10)),
        Subject::authenticated("admin", vec![org], Vec::new()),
    );
    let store = MemoryStore::new();
    if let Ok((_, datasets)) = rt.block_on(import_ckan_json(&store, &ctx, raw, "sk")) {
        assert!(
            datasets <= raw.len(),
            "{datasets} datasets from {} bytes",
            raw.len()
        );
    }
});
