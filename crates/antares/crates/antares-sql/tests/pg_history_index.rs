// SPDX-License-Identifier: EUPL-1.2
//! The history on one btree (migration 0017, T-5057): a fresh database gets
//! `u_attr_instances_time` alone, and a database migrated before it moves
//! there online, one partition a maintenance pass, with every read and
//! upsert answering as before. Needs ANTARES_TEST_DATABASE_URL (see
//! tests/pg.rs recipe); runs in a database of its own, since it rebuilds the
//! history indexes the other tests read.

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::pg;
use antares_sql::store::pg::maintenance::{self, TemporalBackend};
use antares_sql::store::pg::temporal::PgTemporalStore;
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

async fn own_database(url: &str) -> sqlx::PgPool {
    let admin = pg::connect(url, 2).await.expect("connect+migrate");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let name = format!("antares_oneindex_{run}");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("own database");
    let (base, _) = url.rsplit_once('/').expect("a database URL");
    pg::connect(&format!("{base}/{name}"), 4)
        .await
        .expect("connect+migrate the own database")
}

async fn present(pool: &sqlx::PgPool, index: &str) -> bool {
    sqlx::query_scalar("SELECT to_regclass('public.' || $1) IS NOT NULL")
        .bind(index)
        .fetch_one(pool)
        .await
        .expect("catalog")
}

async fn instance_rows(pool: &sqlx::PgPool) -> i64 {
    let mut tx = pool.begin().await.expect("begin");
    sqlx::query("SELECT set_config('antares.service', 'on', true)")
        .execute(&mut *tx)
        .await
        .expect("service");
    let n = sqlx::query_scalar("SELECT count(*) FROM attr_instances")
        .fetch_one(&mut *tx)
        .await
        .expect("count");
    tx.commit().await.expect("commit");
    n
}

#[tokio::test(flavor = "multi_thread")]
async fn a_migrated_history_moves_onto_one_index_and_reads_the_same() {
    let url = require_db!();
    let pool = own_database(&url).await;
    // Fresh: the one index only.
    assert!(!present(&pool, "i_attr_instances_lookup").await);
    assert!(!present(&pool, "u_attr_instances").await);
    let valid: bool = sqlx::query_scalar(
        "SELECT indisvalid FROM pg_index WHERE indexrelid = 'public.u_attr_instances_time'::regclass",
    )
    .fetch_one(&pool)
    .await
    .expect("the one index");
    assert!(valid);
    let backend = maintenance::detect_temporal_backend(&pool)
        .await
        .expect("backend");
    if backend == TemporalBackend::Hypertable {
        // A hypertable with history keeps both indexes (no concurrent unique
        // build in timescale); only the fresh shape above applies.
        return;
    }

    // The shape every database migrated before 0017 has.
    for ddl in [
        "DROP INDEX u_attr_instances_time",
        "CREATE INDEX i_attr_instances_lookup ON attr_instances (tenant_id, entity_id, attr_id, observed_at DESC)",
        "CREATE UNIQUE INDEX u_attr_instances ON attr_instances (tenant_id, entity_id, attr_id, instance_id, observed_at)",
    ] {
        sqlx::query(ddl).execute(&pool).await.expect("legacy shape");
    }
    // Weekly partitions around now, and rows in them and in DEFAULT.
    maintenance::temporal_maintenance(&pool, backend, None)
        .await
        .expect("maintenance");
    let s = PgTemporalStore::new(pool.clone());
    let t = TenantId::new("oneindex").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let id = "urn:ngsi-ld:Vehicle:oneindex";
    let speed = "https://uri.etsi.org/ngsi-ld/default-context/speed";
    let now = chrono::Utc::now();
    let at = |mins: i64| (now - chrono::Duration::minutes(mins)).to_rfc3339();
    let doc = json!({
        "id": id, "type": "Vehicle", "createdAt": at(0), "modifiedAt": at(0),
        speed: [
            {"type": "Property", "value": 80, "observedAt": at(5), "instanceId": "urn:ngsi-ld:Instance:1"},
            {"type": "Property", "value": 81, "observedAt": at(60 * 24 * 8), "instanceId": "urn:ngsi-ld:Instance:2"},
            {"type": "Property", "value": 82, "observedAt": "2001-01-01T00:00:00Z", "instanceId": "urn:ngsi-ld:Instance:3"},
            {"type": "Property", "value": 83, "observedAt": at(1), "datasetId": "urn:ngsi-ld:Dataset:a", "instanceId": "urn:ngsi-ld:Instance:4"}
        ]
    });
    assert!(s.create(&t, id, &doc).await.expect("create"));
    let before = s.get(&t, id).await.expect("get").expect("present");

    let mut lines = Vec::new();
    for _ in 0..20 {
        match maintenance::history_one_index(&pool)
            .await
            .expect("one index step")
        {
            Some(line) => lines.push(line),
            None => break,
        }
    }
    assert!(
        lines.iter().any(|l| l.contains("swapped")),
        "never swapped: {lines:?}"
    );
    assert!(!present(&pool, "i_attr_instances_lookup").await);
    assert!(!present(&pool, "u_attr_instances").await);
    let (valid, children, partitions): (bool, i64, i64) = sqlx::query_as(
        "SELECT (SELECT indisvalid FROM pg_index WHERE indexrelid = 'public.u_attr_instances_time'::regclass),
                (SELECT count(*) FROM pg_inherits WHERE inhparent = 'public.u_attr_instances_time'::regclass),
                (SELECT count(*) FROM pg_inherits WHERE inhparent = 'public.attr_instances'::regclass)",
    )
    .fetch_one(&pool)
    .await
    .expect("swap state");
    assert!(valid, "the parent index is valid");
    assert_eq!(
        children, partitions,
        "every partition carries the one index"
    );
    assert!(maintenance::history_one_index(&pool)
        .await
        .expect("done")
        .is_none());

    // Reads answer the same; the upsert's conflict now rides the one index.
    assert_eq!(s.get(&t, id).await.expect("get").expect("present"), before);
    let rows = instance_rows(&pool).await;
    let r = s.mutate(&t, id, |d| {
        d[speed][0]["value"] = json!(90);
        Ok::<(), ()>(())
    });
    assert!(matches!(r.await, Ok(Some(Ok(())))));
    assert_eq!(instance_rows(&pool).await, rows, "updated in place");
    let got = s.get(&t, id).await.expect("get").expect("present");
    assert_eq!(got[speed][0]["value"], json!(90));
}
