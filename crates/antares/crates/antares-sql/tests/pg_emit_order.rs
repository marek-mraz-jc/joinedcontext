// SPDX-License-Identifier: EUPL-1.2
//! S5-16: on the Postgres arm the change hook (bus=local: history and the
//! matcher) runs after the commit, so without an order a later write's hook
//! can finish before an earlier one's and leave the consumer holding the
//! older state last. Skips loudly without ANTARES_TEST_DATABASE_URL.

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::any::{AnyStore, PgBackend};
use antares_sql::store::pg;
use antares_store::Kind;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

const ID: &str = "urn:ngsi-ld:Vehicle:order-1";

fn doc(speed: i64) -> Value {
    json!({"id": ID, "type": ["Vehicle"],
           "createdAt": "2026-11-22T00:00:00Z", "modifiedAt": "2026-11-22T00:00:00Z",
           "speed": [{"type": "Property", "value": speed}]})
}

fn speed(v: &Value) -> i64 {
    v["speed"][0]["value"].as_i64().unwrap_or(-1)
}

/// The first write's hook is held while a second write of the same Entity
/// commits: the second hook runs after the first, in commit order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hooks_of_one_entity_fire_in_commit_order() {
    let url = require_db!();
    let pool = pg::connect(&url, 5).await.expect("connect");
    let t = TenantId::new("pgemitorder").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    sqlx::query("DELETE FROM entities WHERE tenant_id = $1")
        .bind(t.as_str())
        .execute(&pool)
        .await
        .expect("clean");
    let store = Arc::new(AnyStore::Pg(PgBackend::new(pool)));
    let seen: Arc<Mutex<Vec<(i64, i64)>>> = Arc::default();
    let entered = Arc::new(tokio::sync::Notify::new());
    {
        let (seen, entered) = (seen.clone(), entered.clone());
        store.set_change_hook(Arc::new(move |_t, before, after| {
            let (seen, entered) = (seen.clone(), entered.clone());
            Box::pin(async move {
                let (b, a) = (
                    before.as_ref().map_or(-1, speed),
                    after.as_ref().map_or(-1, speed),
                );
                if a == 1 {
                    entered.notify_one();
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                seen.lock().expect("seen").push((b, a));
            })
        }));
    }
    assert!(store
        .create(&t, Kind::Entity, ID, doc(0))
        .await
        .expect("create"));
    let first = {
        let (store, t) = (store.clone(), t.clone());
        tokio::spawn(async move { store.upsert(&t, Kind::Entity, ID, doc(1)).await })
    };
    entered.notified().await;
    store
        .upsert(&t, Kind::Entity, ID, doc(2))
        .await
        .expect("second write");
    first.await.expect("join").expect("first write");
    store.delete(&t, Kind::Entity, ID).await.expect("delete");
    assert_eq!(
        *seen.lock().expect("seen"),
        [(-1, 0), (0, 1), (1, 2), (2, -1)],
        "each hook sees the state the one before it left"
    );
}
