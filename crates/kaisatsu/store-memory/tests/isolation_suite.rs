#![forbid(unsafe_code)]
//! Isolation suite (tests/isolation_cases, W2, W3, W4) against `MemoryStore`; kaisatsu-store-pg
//! runs the same suite against `PgStore`.

use kaisatsu_store_memory::MemoryStore;
use std::sync::Arc;

mod isolation_cases;

#[tokio::test]
async fn memory_store_passes_isolation_and_odata_secret_field_refusal() {
    let store = Arc::new(MemoryStore::default());
    isolation_cases::run_shared_isolation_suite(store).await;
}
