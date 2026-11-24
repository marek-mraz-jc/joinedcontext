#![forbid(unsafe_code)]
//! Error paths and less travelled methods of both stores: WebAuthn credential update, list and
//! organization-scoped delete (K11), identity-provider links (K12), outbox backoff without a
//! given retry time (K9), sessions removed with their person, the K entity sets over OData, and
//! the PostgreSQL boot and migration refusals. Runs against `MemoryStore` always and `PgStore`
//! with `KAISATSU_TEST_DATABASE_URL`.
//! The shared half lives in tests/error_cases; kaisatsu-store-pg runs it against `PgStore`.

use kaisatsu_store_memory::MemoryStore;
use std::sync::Arc;

mod error_cases;

#[tokio::test]
async fn memory_store_error_paths() {
    error_cases::contract(Arc::new(MemoryStore::default())).await;
}
