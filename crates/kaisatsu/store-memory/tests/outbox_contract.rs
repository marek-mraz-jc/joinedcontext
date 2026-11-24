#![forbid(unsafe_code)]
//! K9 outbox and K11/K12 WebAuthn contract (tests/outbox_cases) against `MemoryStore`;
//! kaisatsu-store-pg runs it against `PgStore` in odata_pushdown_and_outbox.rs.

use kaisatsu_core::Store;
use kaisatsu_store_memory::MemoryStore;
use std::sync::Arc;

mod outbox_cases;

#[tokio::test]
async fn memory_store_outbox_and_webauthn_contract() {
    let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
    outbox_cases::assert_outbox_and_webauthn_contract(store).await;
}
