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

/// A database of the test's own. The second batch waits on the first one's row locks for the
/// whole of its transaction, under the pool's 5 s lock_timeout. In the shared database that
/// transaction ran beside antares-api's query_id_pattern_count_pg, which seeds 205 k rows into
/// the same `entities` and `entity_values` and ANALYZEs them: every run that overlapped it took
/// 2.5-6.9 s against 0.6-1.7 s alone, and two of them failed 55P03 "while locking tuple"
/// (ci 46127454490, 46131795730).
async fn own_database(url: &str) -> (sqlx::PgPool, sqlx::PgPool, String) {
    let admin = pg::connect(url, 2).await.expect("connect+migrate");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let name = format!("antares_batchrace_{run}");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("own database");
    let (base, _) = url.rsplit_once('/').expect("a database URL");
    let pool = pg::connect(&format!("{base}/{name}"), 4)
        .await
        .expect("connect+migrate the own database");
    (admin, pool, name)
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
    let (admin, pool, name) = own_database(&url).await;
    let t = TenantId::new("pgbatchrace").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let s = Arc::new(PgEntityStore::new(pool.clone()));
    // 300 Vehicles: the second batch waits for the first one's whole transaction, mostly its
    // UPDATE and the entity_values trigger. At 1000 rows that UPDATE took up to 0.6 s beside a
    // neighbour's 205 k-row seed on 4 local cores and passed 5 s on the ci runner; at 200 rows
    // the wait stayed under 0.1 s on the same load.
    let items = docs(300);
    s.batch_upsert_replace(&t, &items).await.expect("seed");
    let ids: Vec<String> = items.into_iter().map(|(id, _)| id).collect();
    // ids 0..200 and, in reverse, 100..300: 100 Vehicles in both
    let first: Vec<String> = ids.iter().take(200).cloned().collect();
    let second: Vec<String> = ids.iter().skip(100).rev().cloned().collect();

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
        assert_eq!(res.len(), 200);
        assert!(res.iter().all(|r| matches!(r, Some(Ok(())))), "{res:?}");
    }
    let shared = s
        .get(&t, "urn:ngsi-ld:Vehicle:race-150")
        .await
        .expect("get")
        .expect("present");
    let speed = shared["speed"][0]["value"].as_i64();
    assert!(matches!(speed, Some(1 | 2)), "one batch's value: {speed:?}");
    drop(s);
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("drop own database");
}
