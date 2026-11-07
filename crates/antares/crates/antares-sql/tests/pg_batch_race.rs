// SPDX-License-Identifier: EUPL-1.2
//! Concurrent batch writes over the same Entities (T-5636). Its own test
//! binary: pg_batch_statements counts sqlx's statement events through a
//! thread-local subscriber, and a test running beside it on threads with no
//! subscriber turns those events off process-wide. Skips loudly without
//! ANTARES_TEST_DATABASE_URL.

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::pg;
use antares_sql::store::pg::entity::PgEntityStore;
use serde_json::json;
use std::sync::Arc;

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

fn docs(n: usize) -> Vec<(String, serde_json::Value)> {
    (0..n)
        .map(|i| {
            let id = format!("urn:ngsi-ld:Vehicle:race-{i}");
            let doc = json!({
                "id": id, "type": ["Vehicle"],
                "createdAt": "2026-10-25T04:00:00Z", "modifiedAt": "2026-10-25T04:00:00Z",
                "speed": [{"type": "Property", "value": 0}]
            });
            (id, doc)
        })
        .collect()
}

/// T-5636: two batches of one stream in flight at once over the same
/// Vehicles (HSL's feed sends each vehicle every second) both commit: each
/// takes its row locks in id order, so one waits for the other instead of
/// deadlocking, whatever order the caller listed the ids in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_concurrent_batches_over_overlapping_ids_both_succeed() {
    let url = require_db!();
    let pool = pg::connect(&url, 4).await.expect("pool");
    let t = TenantId::new("pgbatchrace").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let s = Arc::new(PgEntityStore::new(pool.clone()));
    sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
        .bind(t.as_str())
        .execute(&pool)
        .await
        .expect("clean");
    let items = docs(1500);
    s.batch_upsert_replace(&t, &items).await.expect("seed");
    let ids: Vec<String> = items.into_iter().map(|(id, _)| id).collect();
    // ids 0..1000 and, in reverse, 500..1500: 500 Vehicles in both
    let first: Vec<String> = ids.iter().take(1000).cloned().collect();
    let second: Vec<String> = ids.iter().skip(500).rev().cloned().collect();

    let run = |ids: Vec<String>, value: i64| {
        let s = Arc::clone(&s);
        let t = t.clone();
        tokio::spawn(async move {
            s.batch_mutate(&t, &ids, |_, d| {
                d["speed"][0]["value"] = json!(value);
                Ok::<(), ()>(())
            })
            .await
        })
    };
    let (a, b) = tokio::join!(run(first, 1), run(second, 2));
    for res in [a, b] {
        let res = res.expect("task").expect("the batch commits");
        assert_eq!(res.len(), 1000);
        assert!(res.iter().all(|r| matches!(r, Some(Ok(())))), "{res:?}");
    }
    let shared = s
        .get(&t, "urn:ngsi-ld:Vehicle:race-700")
        .await
        .expect("get")
        .expect("present");
    let speed = shared["speed"][0]["value"].as_i64();
    assert!(matches!(speed, Some(1 | 2)), "one batch's value: {speed:?}");
}
