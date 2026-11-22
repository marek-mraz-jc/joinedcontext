// SPDX-License-Identifier: EUPL-1.2
//! The 4.22 reap of history (T-7517): it reads the `{leaf}_expiry` index in bounded batches
//! that resume, a batch that times out keeps the batches and the prune committed before it,
//! audit rows stay, and one process reaps at a time. Needs ANTARES_TEST_DATABASE_URL (see
//! tests/pg.rs recipe); each test runs in a database of its own, since the reap's locks and
//! indexes are the database's.

#![cfg(feature = "postgres")]

use antares_model::TenantId;
use antares_sql::store::pg;
use antares_sql::store::pg::maintenance::{self, TemporalBackend};
use sqlx::{Executor, PgPool};
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

const TENANT: &str = "reap";
/// The non-superuser the reap runs as, bound by forced RLS.
const ROLE: &str = "antares_reap_probe";

struct Db {
    url: String,
    owner: PgPool,
    backend: TemporalBackend,
}

async fn own_database(url: &str, tag: &str) -> Db {
    let admin = pg::connect(url, 2).await.expect("connect+migrate");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    let name = format!("antares_reap_{tag}_{run}");
    admin
        .execute(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .await
        .expect("own database");
    let (base, _) = url.rsplit_once('/').expect("a database URL");
    let url = format!("{base}/{name}");
    let owner = pg::connect(&url, 4)
        .await
        .expect("connect+migrate the own database");
    let tenant = TenantId::new(TENANT).expect("tenant");
    pg::ensure_tenant(&owner, &tenant)
        .await
        .expect("tenant row");
    let backend = maintenance::detect_temporal_backend(&owner)
        .await
        .expect("backend");
    Db {
        url,
        owner,
        backend,
    }
}

impl Db {
    /// `count` instances observed in 2007, whose `expiresAt` is `expires` (SQL over `n`).
    async fn instances(&self, kind: &str, count: i32, expires: &str, provenance: Option<&str>) {
        let mut tx = self.owner.begin().await.expect("begin");
        sqlx::query("SELECT set_config('antares.tenant', $1, true)")
            .bind(TENANT)
            .execute(&mut *tx)
            .await
            .expect("tenant");
        let sql = format!(
            "INSERT INTO attr_instances (tenant_id, entity_id, attr_id, instance_id, observed_at,
                                         created_at, modified_at, data, provenance)
             SELECT $1, 'urn:r:e', 'https://example.org/' || $2, 'urn:i:' || $2 || n,
                    timestamptz '2007-03-06Z' + n * interval '1 second', '2007-03-06Z',
                    '2007-03-06Z',
                    jsonb_build_object('type', 'Property', 'value', n, 'expiresAt', {expires}),
                    $4::jsonb
               FROM generate_series(1, $3) n"
        );
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(TENANT)
            .bind(kind)
            .bind(count)
            .bind(provenance)
            .execute(&mut *tx)
            .await
            .expect("instances");
        tx.commit().await.expect("commit");
    }

    async fn stored(&self, kind: &str) -> i64 {
        let mut tx = self.owner.begin().await.expect("begin");
        pg::set_service(&mut tx).await.expect("service");
        sqlx::query_scalar("SELECT count(*) FROM attr_instances WHERE attr_id = $1")
            .bind(format!("https://example.org/{kind}"))
            .fetch_one(&mut *tx)
            .await
            .expect("count")
    }

    async fn indexed(&self) {
        maintenance::history_expiry_index(&self.owner, self.backend)
            .await
            .expect("index build")
            .expect("leaves to build on");
        assert!(
            maintenance::history_expiry_index(&self.owner, self.backend)
                .await
                .expect("second build")
                .is_none(),
            "one pass indexes every leaf of a small database"
        );
    }

    /// A pool whose every session is the maintenance role's: no superuser, bound by RLS.
    async fn as_role(&self, timeout: Duration) -> PgPool {
        let mut tx = self.owner.begin().await.expect("begin");
        for sql in [
            "SET LOCAL lock_timeout = '60s'".to_owned(),
            "SELECT pg_advisory_xact_lock(4334)".to_owned(),
            format!(
                "DO $$ BEGIN CREATE ROLE {ROLE} NOLOGIN;
                 EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
            ),
            "DO $$ BEGIN CREATE ROLE antares_maintenance NOLOGIN;
             EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
                .to_owned(),
            format!("GRANT antares_maintenance TO {ROLE}"),
            format!("GRANT USAGE ON SCHEMA public TO {ROLE}"),
            format!(
                "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {ROLE}"
            ),
        ] {
            tx.execute(sqlx::AssertSqlSafe(sql.clone()))
                .await
                .expect(&sql);
        }
        tx.commit().await.expect("commit");
        let session = format!(
            "SET ROLE {ROLE}; SET statement_timeout = {}",
            timeout.as_millis()
        );
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .after_connect(move |conn, _| {
                let session = session.clone();
                Box::pin(async move {
                    conn.execute(sqlx::AssertSqlSafe(session)).await?;
                    Ok(())
                })
            })
            .connect(&self.url)
            .await
            .expect("role pool")
    }
}

/// Expired one second apart from 2020-01-01.
const EXPIRED: &str =
    "to_char(timestamp '2020-01-01' + n * interval '1 second', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')";

#[tokio::test(flavor = "multi_thread")]
async fn a_timed_out_batch_keeps_the_prune_and_the_batches_before_it_and_the_next_pass_resumes() {
    let db = own_database(&require_db!(), "timeout").await;
    if db.backend != TemporalBackend::Partitioned {
        eprintln!("SKIP: the locked row is addressed in a partition");
        return;
    }
    db.instances("speed", 2_500, EXPIRED, None).await;
    db.indexed().await;
    let stamp = "2026-08-04T09:00:00Z";
    let attr = "https://example.org/speed";
    let doc = serde_json::json!({"id": "urn:r:e", "type": ["T"],
        "createdAt": stamp, "modifiedAt": stamp,
        attr: [{"type": "Property", "value": 1, "datasetId": "urn:d:gone",
                "expiresAt": "2020-01-01T00:00:00Z"},
               {"type": "Property", "value": 2}]});
    let tenant = TenantId::new(TENANT).expect("tenant");
    assert!(pg::entity::PgEntityStore::new(db.owner.clone())
        .create(&tenant, "urn:r:e", &doc)
        .await
        .expect("create"));

    // A writer holds a row of the second batch for longer than a statement may run.
    let mut writer = db.owner.begin().await.expect("begin");
    pg::set_service(&mut writer).await.expect("service");
    sqlx::query("SELECT 1 FROM attr_instances WHERE instance_id = 'urn:i:speed1500' FOR UPDATE")
        .execute(&mut *writer)
        .await
        .expect("row lock");

    let reaper = db.as_role(Duration::from_secs(1)).await;
    let first = maintenance::reap_expired_instances(&reaper, db.backend)
        .await
        .expect("a pass that outlives its timeout")
        .expect("the lock is free");
    assert_eq!(first.deleted, 1_000, "{first:?}");
    assert_eq!(first.failed.len(), 1, "{first:?}");
    assert_eq!(first.pruned, 1, "{first:?}");
    assert_eq!(db.stored("speed").await, 1_500);
    let mut read = db.owner.begin().await.expect("begin");
    pg::set_service(&mut read).await.expect("service");
    let current: serde_json::Value =
        sqlx::query_scalar("SELECT entity FROM entities WHERE id = 'urn:r:e'")
            .fetch_one(&mut *read)
            .await
            .expect("current state");
    drop(read);
    assert_eq!(
        current[attr].as_array().map(Vec::len),
        Some(1),
        "the prune outlived the timeout: {current}"
    );

    writer.rollback().await.expect("release");
    let second = maintenance::reap_expired_instances(&reaper, db.backend)
        .await
        .expect("second pass")
        .expect("the lock is free");
    assert_eq!(second.deleted, 1_500, "{second:?}");
    assert!(second.failed.is_empty(), "{second:?}");
    assert_eq!(db.stored("speed").await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn rows_stored_before_the_index_are_reaped_and_audit_rows_never() {
    let db = own_database(&require_db!(), "backfill").await;
    db.instances("old", 40, EXPIRED, None).await;
    db.instances("audit", 40, EXPIRED, Some(r#"{"audit": true}"#))
        .await;
    db.instances("comma", 3, "'2020-01-01T00:00:00,5+01:00'", None)
        .await;
    db.instances("day", 3, "'2020-01-01'", None).await;
    db.instances("future", 5, "'2999-01-01T00:00:00Z'", None)
        .await;
    for (kind, text) in [
        ("now", "now"),
        ("epoch", "epoch"),
        ("feb30", "2020-02-30T00:00:00Z"),
    ] {
        db.instances(kind, 1, &format!("'{text}'"), None).await;
    }
    let reaper = db.as_role(Duration::from_secs(30)).await;
    let unindexed = maintenance::reap_expired_instances(&reaper, db.backend)
        .await
        .expect("pass")
        .expect("the lock is free");
    assert_eq!(
        unindexed.deleted, 0,
        "a leaf without its index is not scanned"
    );

    db.indexed().await;
    let reap = maintenance::reap_expired_instances(&reaper, db.backend)
        .await
        .expect("pass")
        .expect("the lock is free");
    assert!(reap.failed.is_empty(), "{reap:?}");
    assert_eq!(reap.deleted, 46, "{reap:?}");
    for (kind, left) in [
        ("old", 0),
        ("comma", 0),
        ("day", 0),
        ("audit", 40),
        ("future", 5),
        ("now", 1),
        ("epoch", 1),
        ("feb30", 1),
    ] {
        assert_eq!(db.stored(kind).await, left, "{kind}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_leaf_large_enough_for_a_parallel_build_gets_a_valid_index_and_is_reaped() {
    let mut db = own_database(&require_db!(), "parallel").await;
    if db.backend != TemporalBackend::Partitioned {
        eprintln!("SKIP: the index is named after a partition");
        return;
    }
    db.instances("live", 20_000, "'2999-01-01T00:00:00Z'", None)
        .await;
    db.instances("gone", 2_000, EXPIRED, None).await;
    // Any leaf is large enough for a parallel btree build in this database.
    db.owner
        .execute(
            "DO $$ BEGIN
               EXECUTE format('ALTER DATABASE %I SET min_parallel_table_scan_size = 0', current_database());
               EXECUTE format('ALTER DATABASE %I SET max_parallel_maintenance_workers = 2', current_database());
               EXECUTE format('ALTER DATABASE %I SET maintenance_work_mem = %L', current_database(), '128MB');
             END $$",
        )
        .await
        .expect("parallel settings");
    db.owner.close().await;
    db.owner = pg::connect(&db.url, 4).await.expect("reconnect");

    let line = maintenance::history_expiry_index(&db.owner, db.backend)
        .await
        .expect("index build")
        .expect("leaves to build on");
    assert!(!line.contains("deferred"), "{line}");
    let (valid, parallel): (bool, String) = sqlx::query_as(
        "SELECT (SELECT indisvalid FROM pg_index
                  WHERE indexrelid = 'public.attr_instances_default_expiry'::regclass),
                (SELECT proparallel::text FROM pg_proc
                  WHERE oid = 'public.history_expires_at(jsonb)'::regprocedure)",
    )
    .fetch_one(&db.owner)
    .await
    .expect("catalog");
    assert!(valid, "{line}");
    // PostgreSQL 17 lets the build pass either way; 16 is what dev and prod run.
    assert_eq!(
        parallel, "u",
        "a subtransaction cannot start in parallel mode"
    );
    let reap = maintenance::reap_expired_instances(&db.owner, db.backend)
        .await
        .expect("pass")
        .expect("the lock is free");
    assert!(reap.failed.is_empty(), "{reap:?}");
    assert_eq!(reap.deleted, 2_000, "{reap:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn each_history_step_has_released_its_lock_when_it_returns() {
    let db = own_database(&require_db!(), "unlock").await;
    if db.backend != TemporalBackend::Partitioned {
        eprintln!("SKIP: the one-index and audit steps build on partitions");
        return;
    }
    let mut probe = db.owner.acquire().await.expect("probe");
    // A backend with temporary tables drops them before its exit releases its locks, so a
    // lock left to the close is still held when the probe asks.
    let slow_exit = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(|conn, _| {
            Box::pin(async move {
                conn.execute(
                    "DO $$ BEGIN FOR i IN 1..300 LOOP
                       EXECUTE format('CREATE TEMP TABLE slow_exit_%s (n int)', i);
                     END LOOP; END $$",
                )
                .await?;
                Ok(())
            })
        })
        .connect(&db.url)
        .await
        .expect("pool");
    for step in [
        "history_expiry_index",
        "history_reap",
        "history_one_index",
        "history_audit_index",
    ] {
        let ran = match step {
            "history_expiry_index" => maintenance::history_expiry_index(&slow_exit, db.backend)
                .await
                .expect(step)
                .is_some(),
            "history_reap" => maintenance::reap_expired_instances(&slow_exit, db.backend)
                .await
                .expect(step)
                .is_some(),
            "history_one_index" => maintenance::history_one_index(&slow_exit)
                .await
                .expect(step)
                .is_some(),
            _ => maintenance::history_audit_index(&slow_exit)
                .await
                .expect(step)
                .is_some(),
        };
        assert!(ran, "{step} had nothing to do");
        let free: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext($1))")
            .bind(format!("antares.{step}"))
            .fetch_one(&mut *probe)
            .await
            .expect("probe");
        assert!(free, "{step} returned with its lock still held");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_reap_reads_the_index_for_its_batch_alone_and_the_app_role_still_writes() {
    let db = own_database(&require_db!(), "plan").await;
    if db.backend != TemporalBackend::Partitioned {
        eprintln!("SKIP: the plan is read on a partition");
        return;
    }
    db.instances("live", 20_000, "'2999-01-01T00:00:00Z'", None)
        .await;
    db.instances("gone", 500, EXPIRED, None).await;
    db.indexed().await;
    db.owner
        .execute("ANALYZE attr_instances_default")
        .await
        .expect("analyze");
    let role = db.as_role(Duration::from_secs(30)).await;

    let mut tx = role.begin().await.expect("begin");
    pg::set_service(&mut tx).await.expect("service");
    let forced: bool = sqlx::query_scalar("SELECT row_security_active('attr_instances_default')")
        .fetch_one(&mut *tx)
        .await
        .expect("rls");
    assert!(forced, "the plan is the one RLS allows");
    tx.execute(maintenance::REAP_PLAN)
        .await
        .expect("plan settings");
    let sql = maintenance::REAP_ROWS
        .replace("{t}", "public.attr_instances_default")
        .replace("$1", "10");
    let plan: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}"
    )))
    .fetch_one(&mut *tx)
    .await
    .expect("explain");
    tx.rollback().await.expect("rollback");
    let text = plan.to_string();
    assert!(text.contains("attr_instances_default_expiry"), "{text}");
    assert!(text.contains("Tid Scan"), "{text}");
    assert!(!text.contains("Seq Scan"), "{text}");
    let top = &plan[0]["Plan"];
    let buffers = ["Shared Hit Blocks", "Shared Read Blocks"]
        .iter()
        .filter_map(|b| top[*b].as_u64())
        .sum::<u64>();
    assert!(
        buffers < 300,
        "10 rows of 20 500 took {buffers} buffers: {text}"
    );

    // The index calls history_expires_at on the writer's INSERT.
    let mut tx = role.begin().await.expect("begin");
    sqlx::query("SELECT set_config('antares.tenant', $1, true)")
        .bind(TENANT)
        .execute(&mut *tx)
        .await
        .expect("tenant");
    tx.execute(
        "INSERT INTO attr_instances (tenant_id, entity_id, attr_id, instance_id, observed_at,
                                     created_at, modified_at, data)
         VALUES ('reap', 'urn:r:e', 'https://example.org/new', 'urn:i:new', '2007-03-07Z',
                 '2007-03-07Z', '2007-03-07Z', '{\"expiresAt\": \"2020-01-01T00:00:00Z\"}')",
    )
    .await
    .expect("the app role writes an expiring instance");
    tx.commit().await.expect("commit");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_reaper_leaves_the_pass_to_the_one_that_runs() {
    let db = own_database(&require_db!(), "one").await;
    db.instances("gone", 5, EXPIRED, None).await;
    db.indexed().await;
    let mut other = db.owner.acquire().await.expect("connection").detach();
    sqlx::query("SELECT pg_advisory_lock(hashtext('antares.history_reap'))")
        .execute(&mut other)
        .await
        .expect("the first reaper's lock");
    assert!(maintenance::reap_expired_instances(&db.owner, db.backend)
        .await
        .expect("pass")
        .is_none());
    assert_eq!(db.stored("gone").await, 5);
    // The lock ends with its holder's session: a killed pod blocks no other.
    sqlx::Connection::close(other).await.expect("close");
    let mut reaped = 0;
    for _ in 0..50 {
        if let Some(reap) = maintenance::reap_expired_instances(&db.owner, db.backend)
            .await
            .expect("pass")
        {
            reaped = reap.deleted;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(reaped, 5);
}
