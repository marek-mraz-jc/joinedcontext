// SPDX-License-Identifier: EUPL-1.2
//! The batch write path's statement shape (T-5050): a batch of 1,000
//! Entities runs the same statements as a batch of 10 (one multi-row
//! statement per step, never one per Entity), a batch that changes nothing
//! writes no row, and `synchronous_commit = off` reaches the batch
//! transaction only when the store is told so. The statements are counted
//! from sqlx's own per-statement `sqlx::query` events, so the count is what
//! went over the wire. Skips loudly without ANTARES_TEST_DATABASE_URL.

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::pg;
use antares_sql::store::pg::entity::PgEntityStore;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

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

/// Every statement sqlx sends, as its full text (or its summary when
/// the summary is the whole statement).
#[derive(Clone, Default)]
struct Statements(Arc<Mutex<Vec<String>>>);

impl Statements {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

struct Sql(String, String);

impl tracing::field::Visit for Sql {
    fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
        match f.name() {
            "summary" => self.0 = v.to_owned(),
            "db.statement" => self.1 = v.trim().to_owned(),
            _ => {}
        }
    }
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        self.record_str(f, &format!("{v:?}"));
    }
}

impl<S: tracing::Subscriber> Layer<S> for Statements {
    fn on_event(&self, e: &tracing::Event<'_>, _: Context<'_, S>) {
        if e.metadata().target() == "sqlx::query" {
            let mut sql = Sql(String::new(), String::new());
            e.record(&mut sql);
            let text = if sql.1.is_empty() { sql.0 } else { sql.1 };
            self.0.lock().unwrap_or_else(|e| e.into_inner()).push(text);
        }
    }
}

fn docs(prefix: &str, n: usize, value: i64) -> Vec<(String, serde_json::Value)> {
    (0..n)
        .map(|i| {
            let id = format!("urn:ngsi-ld:Vehicle:{prefix}-{i}");
            let doc = json!({
                "id": id, "type": ["Vehicle"],
                "createdAt": "2026-10-21T18:00:00Z", "modifiedAt": "2026-10-21T18:00:00Z",
                "speed": [{"type": "Property", "value": value}]
            });
            (id, doc)
        })
        .collect()
}

/// Statements of one batch read-modify-write that sets `speed` to `value`.
async fn mutate(
    s: &PgEntityStore,
    t: &TenantId,
    ids: &[String],
    value: i64,
    log: &Statements,
) -> Vec<String> {
    log.take();
    let res = s
        .batch_mutate(t, ids, |_, d| {
            d["speed"][0]["value"] = json!(value);
            Ok::<(), ()>(())
        })
        .await
        .expect("batch mutate");
    assert!(res.iter().all(|r| matches!(r, Some(Ok(())))));
    log.take()
}

#[tokio::test(flavor = "current_thread")]
async fn a_batch_of_1000_runs_the_statements_a_batch_of_10_does() {
    let url = require_db!();
    let log = Statements::default();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::registry().with(
            log.clone().with_filter(
                tracing_subscriber::filter::Targets::new()
                    .with_target("sqlx::query", tracing::Level::TRACE),
            ),
        ),
    );
    let pool = pg::connect(&url, 2).await.expect("pool");
    let t = TenantId::new("pgbatchstmt").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let s = PgEntityStore::new(pool.clone());
    sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
        .bind(t.as_str())
        .execute(&pool)
        .await
        .expect("clean");

    let mut shapes = Vec::new();
    for n in [10, 1000] {
        let items = docs(&format!("n{n}"), n, 0);
        log.take();
        s.batch_upsert_replace(&t, &items).await.expect("upsert");
        let upsert = log.take();
        let ids: Vec<String> = items.iter().map(|(id, _)| id.clone()).collect();
        let update = mutate(&s, &t, &ids, 1, &log).await;
        shapes.push((upsert, update));
    }
    assert_eq!(shapes[0], shapes[1], "1,000 Entities, the statements of 10");
    let (upsert, update) = &shapes[1];
    assert_eq!(
        upsert
            .iter()
            .filter(|q| q.contains("INSERT INTO entities"))
            .count(),
        1,
        "{upsert:#?}"
    );
    assert_eq!(
        update
            .iter()
            .filter(|q| q.contains("UPDATE entities"))
            .count(),
        1,
        "{update:#?}"
    );
    assert!(
        !update.iter().any(|q| q.contains("synchronous_commit")),
        "synchronous unless asked: {update:#?}"
    );

    // a batch that leaves every document as it is writes no row
    let ids: Vec<String> = docs("n1000", 1000, 0)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let same = mutate(&s, &t, &ids, 1, &log).await;
    assert!(
        !same.iter().any(|q| q.contains("UPDATE entities")),
        "an unchanged batch writes nothing: {same:#?}"
    );

    // asked, the batch transaction and only it commits asynchronously
    s.set_async_batch_commit(true);
    let fast = mutate(&s, &t, &ids, 2, &log).await;
    assert!(
        fast.iter()
            .any(|q| q.contains("SET LOCAL synchronous_commit = off")),
        "{fast:#?}"
    );
    log.take();
    s.batch_upsert_replace(&t, &docs("n10", 10, 3))
        .await
        .expect("upsert");
    assert!(log
        .take()
        .iter()
        .any(|q| q.contains("SET LOCAL synchronous_commit = off")));
    let mode: String = sqlx::query_scalar("SHOW synchronous_commit")
        .fetch_one(&pool)
        .await
        .expect("show");
    assert_eq!(mode, "on", "SET LOCAL leaves the pooled session as it was");
    let stored = s
        .get(&t, "urn:ngsi-ld:Vehicle:n1000-7")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(stored["speed"][0]["value"], 2, "the async batch is stored");
}
