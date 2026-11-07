// SPDX-License-Identifier: EUPL-1.2
//! T-5150: the write reserve (`ANTARES_PG_WRITE_RESERVE`). With every
//! connection of the main pool held by reads, a batch upsert still runs on
//! the reserve; without one it waits the pool's 5 s acquire timeout and
//! fails, which is the starvation the reserve removes.
//! Skips loudly without ANTARES_TEST_DATABASE_URL (see tests/pg.rs recipe).

#![cfg(feature = "postgres")]

use std::time::{Duration, Instant};

use antares_model::TenantId;
use antares_sql::store::pg;
use antares_sql::store::pg::entity::PgEntityStore;
use serde_json::json;

macro_rules! require_db {
    () => {
        match std::env::var("ANTARES_TEST_DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
                return;
            }
        }
    };
}

fn item(id: &str) -> (String, serde_json::Value) {
    let doc = json!({
        "id": id, "type": "Vehicle",
        "createdAt": "2026-10-23T09:00:00Z", "modifiedAt": "2026-10-23T09:00:00Z",
        "speed": {"type": "Property", "value": 42}
    });
    (id.to_owned(), doc)
}

const TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::test]
async fn batch_upsert_runs_on_the_reserve_while_reads_hold_the_pool() {
    let url = require_db!();
    let reads = pg::connect_with(&url, 2, TIMEOUT).await.expect("connect");
    let reserve = pg::connect_reserve(&url, 1, TIMEOUT)
        .await
        .expect("reserve");
    let t = TenantId::new("pgreserve").expect("tenant");
    pg::ensure_tenant(&reads, &t).await.expect("tenant row");
    let store = PgEntityStore::new(reads.clone()).with_batch_pool(reserve);

    // two slow reads hold the whole main pool
    let held = (
        reads.acquire().await.expect("read 1"),
        reads.acquire().await.expect("read 2"),
    );
    let started = Instant::now();
    let out = store
        .batch_upsert_replace(&t, &[item("urn:ngsi-ld:Vehicle:reserve-1")])
        .await
        .expect("the batch must not wait for the reads");
    assert_eq!(out.len(), 1);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the batch waited {:?} behind the reads",
        started.elapsed()
    );
    drop(held);
}

/// The defect without the reserve: the same batch behind the same two reads
/// fails with the pool's acquire timeout.
#[tokio::test]
async fn without_a_reserve_the_batch_waits_out_the_reads() {
    let url = require_db!();
    let reads = pg::connect_with(&url, 2, TIMEOUT).await.expect("connect");
    let t = TenantId::new("pgreserve").expect("tenant");
    pg::ensure_tenant(&reads, &t).await.expect("tenant row");
    let store = PgEntityStore::new(reads.clone());

    let held = (
        reads.acquire().await.expect("read 1"),
        reads.acquire().await.expect("read 2"),
    );
    let err = store
        .batch_upsert_replace(&t, &[item("urn:ngsi-ld:Vehicle:reserve-2")])
        .await
        .expect_err("no connection is free for the batch");
    assert!(
        matches!(err, sqlx::Error::PoolTimedOut),
        "expected the pool's acquire timeout, got {err}"
    );
    drop(held);
}
