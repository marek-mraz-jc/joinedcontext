#![forbid(unsafe_code)]
//! The `DaiyaStore` behaviour contract (R8) on the memory store; daiya-store-pg runs the same
//! module on Postgres.

mod contract;

#[tokio::test]
async fn store_contract_holds_on_the_memory_store() {
    contract::run_contract(&daiya_store_memory::MemoryStore::new()).await;
}
