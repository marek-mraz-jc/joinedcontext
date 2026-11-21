// SPDX-License-Identifier: EUPL-1.2
//! An upsert that changes only attribute values takes PostgreSQL's HOT path
//! on `entities` (T-7487). jc-prod counted 0 HOT updates out of 50.7M: the
//! partial index of 0010 read the `entity` document in its predicate, so
//! every upsert wrote a new entry into all 19 indexes. Its own binary, so no
//! other test's writes move the table's counters while this one reads them.
//! Skips loudly without ANTARES_TEST_DATABASE_URL (see tests/pg.rs recipe).

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::pg;
use antares_sql::store::pg::entity::PgEntityStore;
use antares_sql::store::pg::maintenance::history_overrides;
use antares_store::HISTORY_RETENTION;
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

const UPSERTS: i64 = 20;

fn doc(id: &str, n: i64, retention: Option<&str>) -> serde_json::Value {
    let mut d = json!({
        "id": id, "type": "https://example.org/Sensor",
        "location": {"type": "GeoProperty",
                     "value": {"type": "Point", "coordinates": [14.42, 50.08]}},
        "n": {"type": "Property", "value": n}
    });
    if let Some(r) = retention {
        d[HISTORY_RETENTION] = json!([{"type": "Property", "value": r}]);
    }
    d
}

/// (n_tup_upd, n_tup_hot_upd) of `entities`, after the pool's one backend
/// flushed what it counted (PostgreSQL 15+ flushes on idle, at most once a
/// second, so the read polls until the updates it waits for show).
async fn counters(pool: &sqlx::PgPool, at_least_upd: i64) -> (i64, i64) {
    sqlx::query("SELECT pg_stat_force_next_flush()")
        .execute(pool)
        .await
        .expect("flush");
    let mut last = (0, 0);
    for _ in 0..50 {
        sqlx::query("SELECT pg_stat_clear_snapshot()")
            .execute(pool)
            .await
            .expect("snapshot");
        last = sqlx::query_as(
            "SELECT pg_stat_get_tuples_updated('entities'::regclass),
                    pg_stat_get_tuples_hot_updated('entities'::regclass)",
        )
        .fetch_one(pool)
        .await
        .expect("counters");
        if last.0 >= at_least_upd {
            return last;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    last
}

#[tokio::test(flavor = "multi_thread")]
async fn an_attribute_only_upsert_is_a_hot_update_and_retention_still_finds_its_entities() {
    let url = require_db!();
    // One connection: every upsert runs on the backend whose stats the
    // counters flush.
    let pool = pg::connect(&url, 1).await.expect("connect");
    let s = PgEntityStore::new(pool.clone());
    let t = TenantId::new("pghot").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let (plain, own) = ("urn:ngsi-ld:Sensor:hot1", "urn:ngsi-ld:Sensor:hot2");
    for id in [plain, own] {
        let _ = s.delete(&t, id).await;
    }
    s.batch_upsert_replace(
        &t,
        &[
            (plain.to_owned(), doc(plain, 0, None)),
            (own.to_owned(), doc(own, 0, Some("P7D"))),
        ],
    )
    .await
    .expect("seed");
    let (upd0, hot0) = counters(&pool, 0).await;

    for n in 1..=UPSERTS {
        s.batch_upsert_replace(
            &t,
            &[
                (plain.to_owned(), doc(plain, n, None)),
                (own.to_owned(), doc(own, n, Some("P7D"))),
            ],
        )
        .await
        .expect("upsert");
    }
    let (upd, hot) = counters(&pool, upd0 + 2 * UPSERTS).await;
    let (upd, hot) = (upd - upd0, hot - hot0);
    assert!(upd >= 2 * UPSERTS, "the upserts were counted: {upd}");
    assert!(
        hot * 2 > upd,
        "an upsert changing no indexed value must take the HOT path: {hot} of {upd} updates"
    );

    let overrides = |rows: Vec<(String, String, serde_json::Value)>| {
        let mut ids: Vec<String> = rows
            .into_iter()
            .filter(|(tenant, _, _)| tenant == "pghot")
            .map(|(_, id, _)| id)
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(
        overrides(history_overrides(&pool, 10_000).await.expect("overrides")),
        [own],
        "the retention sweep still finds the Entity carrying its own historyRetention"
    );

    // The flag follows the document both ways.
    s.batch_upsert_replace(
        &t,
        &[
            (plain.to_owned(), doc(plain, 0, Some("P1D"))),
            (own.to_owned(), doc(own, 0, None)),
        ],
    )
    .await
    .expect("swap");
    assert_eq!(
        overrides(history_overrides(&pool, 10_000).await.expect("overrides")),
        [plain],
        "adding or removing historyRetention moves the Entity in or out of the sweep"
    );
}
