#![forbid(unsafe_code)]
//! The `Store` contract shared with kaisatsu-store-memory (its tests/contract) against `PgStore`,
//! with `KAISATSU_TEST_DATABASE_URL`.

use kaisatsu_crypto::Kek;
use std::sync::Arc;

mod common;
#[path = "../../store-memory/tests/contract/mod.rs"]
mod contract;

#[tokio::test]
async fn pg_store_meets_the_store_contract() {
    let Some(db) = common::test_db("pg_store_meets_the_store_contract").await else {
        return;
    };
    contract::run_contract(Arc::new(db.store(Kek::generate_for_tests()).await)).await;
    db.drop_db().await;
}
