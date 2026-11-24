#![forbid(unsafe_code)]
//! The `Store` contract (tests/contract) against `MemoryStore`; kaisatsu-store-pg runs the same
//! contract against `PgStore`.

use kaisatsu_store_memory::MemoryStore;
use std::sync::Arc;

mod contract;

#[tokio::test]
async fn memory_store_meets_the_store_contract() {
    contract::run_contract(Arc::new(MemoryStore::default())).await;
}
