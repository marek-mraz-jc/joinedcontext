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
        .as_nanos();
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

/// The step belongs to the pod that holds its advisory lock: another pod's
/// pass finds it taken, builds nothing and reports nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_step_another_pod_holds_is_left_to_it() {
    use sqlx::Connection;
    let url = require_db!();
    let pool = own_database(&url).await;
    let backend = maintenance::detect_temporal_backend(&pool)
        .await
        .expect("backend");
    if backend == TemporalBackend::Hypertable {
        return;
    }
    sqlx::query(
        "CREATE INDEX i_attr_instances_lookup ON attr_instances (tenant_id, entity_id, attr_id, observed_at DESC)",
    )
    .execute(&pool)
    .await
    .expect("the pre-0017 index the step moves off");
    let mut other_pod = pool.acquire().await.expect("a connection").detach();
    let held: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext('antares.history_one_index'))")
            .fetch_one(&mut other_pod)
            .await
            .expect("lock");
    assert!(held, "the lock was free");
    assert_eq!(
        maintenance::history_one_index(&pool)
            .await
            .expect("one index step"),
        None
    );
    assert!(present(&pool, "i_attr_instances_lookup").await);
    other_pod.close().await.expect("close");
}

/// Partitions of `attr_instances` without a valid `{partition}_u_time` index.
async fn partitions_without_one_index(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
          WHERE i.inhparent = 'public.attr_instances'::regclass
            AND NOT EXISTS (SELECT 1 FROM pg_class ic JOIN pg_index x ON x.indexrelid = ic.oid
                             WHERE ic.relname = c.relname || '_u_time' AND x.indisvalid)",
    )
    .fetch_one(pool)
    .await
    .expect("catalog")
}

/// An index a failed build left on a partition is dropped and built again, and a week created
/// while the step reads the partition set waits for an index of its own instead of being
/// swapped onto the one index without one.
#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_partition_index_is_rebuilt_and_a_late_week_waits_for_its_own() {
    use sqlx::Executor;
    let url = require_db!();
    let pool = own_database(&url).await;
    let backend = maintenance::detect_temporal_backend(&pool)
        .await
        .expect("backend");
    if backend == TemporalBackend::Hypertable {
        return;
    }
    for ddl in [
        "DROP INDEX u_attr_instances_time",
        "CREATE INDEX i_attr_instances_lookup ON attr_instances (tenant_id, entity_id, attr_id, observed_at DESC)",
        "CREATE UNIQUE INDEX u_attr_instances ON attr_instances (tenant_id, entity_id, attr_id, instance_id, observed_at)",
    ] {
        pool.execute(ddl).await.expect("legacy shape");
    }
    let mut tx = pool.begin().await.expect("begin");
    sqlx::query("SELECT set_config('antares.service', 'on', true)")
        .execute(&mut *tx)
        .await
        .expect("service");
    sqlx::query(
        "INSERT INTO attr_instances (tenant_id, entity_id, attr_id, instance_id, observed_at,
                                     created_at, modified_at, data)
         SELECT 'oneindex', 'urn:e', 'https://example.org/speed', 'urn:i:' || n,
                timestamptz '2002-06-01Z' + n * interval '1 second', '2002-06-01Z',
                '2002-06-01Z', '{\"type\": \"Property\", \"value\": 1}'::jsonb
           FROM generate_series(1, 2) n",
    )
    .execute(&mut *tx)
    .await
    .expect("two rows of one Tenant");
    tx.commit().await.expect("commit");
    pool.execute(
        "CREATE UNIQUE INDEX CONCURRENTLY attr_instances_default_u_time
         ON attr_instances_default (tenant_id)",
    )
    .await
    .expect_err("a build that fails leaves its index invalid");

    let mut lines = Vec::new();
    for _ in 0..20 {
        if partitions_without_one_index(&pool).await == 0 {
            break;
        }
        lines.push(
            maintenance::history_one_index(&pool)
                .await
                .expect("one index step")
                .expect("a partition to build on"),
        );
    }
    assert!(
        lines.contains(&"history index built on attr_instances_default".to_owned()),
        "{lines:?}"
    );

    // a pod creating a week holds the parent while the step reads the partitions
    let mut week = pool.begin().await.expect("begin");
    week.execute(
        "CREATE TABLE attr_instances_2001w02 PARTITION OF attr_instances
         FOR VALUES FROM ('2001-01-08') TO ('2001-01-15')",
    )
    .await
    .expect("a late week");
    let stepping = pool.clone();
    let step = tokio::spawn(async move { maintenance::history_one_index(&stepping).await });
    let mut waiting = 0i64;
    for _ in 0..200 {
        waiting = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
              WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(&pool)
        .await
        .expect("activity");
        if waiting > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(waiting > 0, "the step never waited on the parent");
    week.commit().await.expect("the week lands");
    assert_eq!(
        step.await.expect("step task").expect("one index step"),
        Some("history index: a new partition waits for its own".to_owned())
    );
    assert_eq!(
        maintenance::history_one_index(&pool)
            .await
            .expect("one index step"),
        Some("history index built on attr_instances_2001w02".to_owned())
    );
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
    maintenance::temporal_maintenance(&pool, backend, None, std::time::Duration::ZERO)
        .await
        .expect("maintenance");
    let s = PgTemporalStore::new(pool.clone());
    let t = TenantId::new("oneindex").expect("tenant");
    pg::ensure_tenant(&pool, &t).await.expect("tenant row");
    let id = "urn:ngsi-ld:Vehicle:oneindex";
    let speed = "https://uri.etsi.org/ngsi-ld/default-context/speed";
    let now = jc_time::TimestampTz::now();
    let at = |mins: i64| (now - jiff::SignedDuration::from_mins(mins)).to_rfc3339();
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
