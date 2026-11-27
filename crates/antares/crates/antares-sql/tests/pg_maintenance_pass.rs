// SPDX-License-Identifier: EUPL-1.2
//! One temporal maintenance pass end to end (T-7594): platform retention drops an expired week,
//! keeps one that holds an audit row, purges the DEFAULT partition and the superseded versions;
//! a pass whose role may touch nothing reports every step it skipped instead of failing; the
//! hypertable pass drops chunks; and the backend probe refuses a database it cannot name. Needs
//! ANTARES_TEST_DATABASE_URL (see tests/pg.rs recipe); each test runs in a database of its own,
//! since retention drops partitions every other test's rows live in.

#![cfg(feature = "postgres")]

use antares_sql::store::pg;
use antares_sql::store::pg::maintenance::{self, temporal_maintenance, TemporalBackend};
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

const TENANT: &str = "pass";

/// A fresh database on the test server and its URL; `setup` runs in it before anything else.
async fn own_database(url: &str, tag: &str, setup: &[&'static str]) -> String {
    let admin = PgPool::connect(url).await.expect("connect");
    let run = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let name = format!("antares_pass_{tag}_{run}");
    admin
        .execute(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .await
        .expect("own database");
    admin.close().await;
    let (base, _) = url.rsplit_once('/').expect("a database URL");
    let own = format!("{base}/{name}");
    let raw = PgPool::connect(&own)
        .await
        .expect("connect the own database");
    for sql in setup {
        raw.execute(*sql).await.expect(sql);
    }
    raw.close().await;
    own
}

/// The summary's lines, split as the pass joins them.
fn lines(summary: &str) -> Vec<&str> {
    summary.split("; ").collect()
}

/// One instance row straight into `table`, observed at `at`, with `provenance`.
async fn instance(pool: &PgPool, table: &str, id: &str, at: &str, provenance: Option<&str>) {
    let mut tx = pool.begin().await.expect("begin");
    pg::set_service(&mut tx).await.expect("service");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO {table} (tenant_id, entity_id, attr_id, instance_id, observed_at, created_at,
                              modified_at, data, provenance)
         VALUES ($1, 'urn:p:e', 'https://example.org/speed', $2, $3::timestamptz, $3::timestamptz,
                 $3::timestamptz, '{{\"type\": \"Property\", \"value\": 1}}', $4::jsonb)"
    )))
    .bind(TENANT)
    .bind(id)
    .bind(at)
    .bind(provenance)
    .execute(&mut *tx)
    .await
    .expect("instance");
    tx.commit().await.expect("commit");
}

async fn superseded_version(pool: &PgPool, at: &str) {
    let mut tx = pool.begin().await.expect("begin");
    pg::set_service(&mut tx).await.expect("service");
    sqlx::query(
        "INSERT INTO temporal_entities (tenant_id, id, types, meta, created_at, modified_at)
         VALUES ($1, 'urn:p:e', ARRAY['https://example.org/V'], '{}'::jsonb, now(), now())
         ON CONFLICT DO NOTHING",
    )
    .bind(TENANT)
    .execute(&mut *tx)
    .await
    .expect("shell");
    sqlx::query(
        "INSERT INTO attr_versions (tenant_id, entity_id, attr_id, instance_id, observed_at, version, data)
         VALUES ($1, 'urn:p:e', 'https://example.org/speed', 'urn:i:v', $2::timestamptz, 1, '{}'::jsonb)",
    )
    .bind(TENANT)
    .bind(at)
    .execute(&mut *tx)
    .await
    .expect("superseded version");
    tx.commit().await.expect("commit");
}

async fn exists(pool: &PgPool, relname: &str) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_class WHERE relname = $1
                        AND relnamespace = 'public'::regnamespace)",
    )
    .bind(relname)
    .fetch_one(pool)
    .await
    .expect("catalog")
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    let mut tx = pool.begin().await.expect("begin");
    pg::set_service(&mut tx).await.expect("service");
    let n = sqlx::query_scalar(sql)
        .fetch_one(&mut *tx)
        .await
        .expect(sql);
    tx.commit().await.expect("commit");
    n
}

/// Platform retention on the partitioned backend: the expired week without rows goes, the
/// expired week holding an audit row stays and says why, the DEFAULT partition loses its old
/// plain row and keeps its old audit row, and the old superseded version goes.
#[tokio::test(flavor = "multi_thread")]
async fn a_retention_pass_drops_an_expired_week_purges_default_and_keeps_audit_rows() {
    let url = require_db!();
    let own = own_database(&url, "retain", &[]).await;
    let pool = pg::connect(&own, 4).await.expect("connect+migrate");
    if maintenance::detect_temporal_backend(&pool).await != Ok(TemporalBackend::Partitioned) {
        eprintln!("SKIP: new databases get timescaledb here (template1)");
        return;
    }
    pg::ensure_tenant(
        &pool,
        &antares_model::TenantId::new(TENANT).expect("tenant"),
    )
    .await
    .expect("tenant row");
    for (name, lo, hi) in [
        ("attr_instances_2001w02", "2001-01-08", "2001-01-15"),
        ("attr_instances_2001w03", "2001-01-15", "2001-01-22"),
    ] {
        pool.execute(sqlx::AssertSqlSafe(format!(
            "CREATE TABLE {name} PARTITION OF attr_instances FOR VALUES FROM ('{lo}') TO ('{hi}')"
        )))
        .await
        .expect("old week");
    }
    instance(
        &pool,
        "attr_instances",
        "urn:i:held",
        "2001-01-16T00:00:00Z",
        Some(r#"{"audit": true}"#),
    )
    .await;
    // 2002 has no week of its own: both land in DEFAULT
    instance(
        &pool,
        "attr_instances",
        "urn:i:old",
        "2002-06-01T00:00:00Z",
        None,
    )
    .await;
    instance(
        &pool,
        "attr_instances",
        "urn:i:old-audit",
        "2002-06-01T00:00:00Z",
        Some(r#"{"audit": true}"#),
    )
    .await;
    superseded_version(&pool, "2002-06-01T00:00:00Z").await;
    superseded_version_fresh(&pool).await;

    let summary = temporal_maintenance(
        &pool,
        TemporalBackend::Partitioned,
        Some(30),
        Duration::ZERO,
    )
    .await
    .expect("pass");
    let got = lines(&summary);
    for want in [
        "deleted 1 superseded versions older than 30d",
        "dropped expired partition attr_instances_2001w02",
        "expired partition attr_instances_2001w03 kept: it holds audit rows",
        "purged 1 expired rows from DEFAULT partition",
    ] {
        assert!(got.contains(&want), "{want:?} in {summary}");
    }
    assert!(!exists(&pool, "attr_instances_2001w02").await, "{summary}");
    assert!(exists(&pool, "attr_instances_2001w03").await, "{summary}");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM attr_instances_default").await,
        1,
        "the audit row outlives the purge: {summary}"
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM attr_versions").await,
        1,
        "the fresh version stays: {summary}"
    );

    // the same pass again in the same period: the claim is taken
    let again = temporal_maintenance(
        &pool,
        TemporalBackend::Partitioned,
        Some(30),
        Duration::from_secs(3600),
    )
    .await
    .expect("second pass");
    assert_eq!(again, "skipped: another instance runs this period");
    pool.close().await;
}

async fn superseded_version_fresh(pool: &PgPool) {
    let mut tx = pool.begin().await.expect("begin");
    pg::set_service(&mut tx).await.expect("service");
    sqlx::query(
        "INSERT INTO attr_versions (tenant_id, entity_id, attr_id, instance_id, observed_at, version, data)
         VALUES ($1, 'urn:p:e', 'https://example.org/speed', 'urn:i:fresh', now(), 1, '{}'::jsonb)",
    )
    .bind(TENANT)
    .execute(&mut *tx)
    .await
    .expect("fresh version");
    tx.commit().await.expect("commit");
}

/// A pass whose role holds the claim and nothing else: no week can be made, so each is left in
/// DEFAULT, and every later step reports that it was skipped instead of failing the pass. The
/// broker's loop logs the summary and ticks again, so one refused step never stops the others.
#[tokio::test(flavor = "multi_thread")]
async fn a_pass_without_rights_reports_every_step_it_skipped() {
    let url = require_db!();
    let own = own_database(&url, "norights", &[]).await;
    let owner = pg::connect(&own, 2).await.expect("connect+migrate");
    if maintenance::detect_temporal_backend(&owner).await != Ok(TemporalBackend::Partitioned) {
        eprintln!("SKIP: new databases get timescaledb here (template1)");
        return;
    }
    let role = "antares_pass_probe";
    for sql in [
        format!(
            "DO $$ BEGIN CREATE ROLE {role} NOLOGIN;
             EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
        ),
        format!("GRANT USAGE ON SCHEMA public TO {role}"),
        format!("GRANT SELECT, INSERT, UPDATE ON maintenance_jobs TO {role}"),
        format!("GRANT SELECT ON history_cutover TO {role}"),
        // an expired week, so retention reaches the partition probe it may not read
        "CREATE TABLE attr_instances_2001w02 PARTITION OF attr_instances
         FOR VALUES FROM ('2001-01-08') TO ('2001-01-15')"
            .to_owned(),
    ] {
        owner
            .execute(sqlx::AssertSqlSafe(sql.clone()))
            .await
            .expect(&sql);
    }
    let session = format!("SET ROLE {role}");
    let limited = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .after_connect(move |conn, _| {
            let session = session.clone();
            Box::pin(async move {
                conn.execute(sqlx::AssertSqlSafe(session)).await?;
                Ok(())
            })
        })
        .connect(&own)
        .await
        .expect("role pool");

    let summary = temporal_maintenance(
        &limited,
        TemporalBackend::Partitioned,
        Some(30),
        Duration::ZERO,
    )
    .await
    .expect("a refused step is a line, never the pass's error");
    let got = lines(&summary);
    assert!(
        got.iter()
            .any(|l| l.starts_with("partition ") && l.ends_with(": left in default")),
        "{summary}"
    );
    for want in [
        "version retention skipped (",
        "retention skipped (",
        "bare sweep skipped (",
        "entity reap skipped (",
        "instance reap skipped",
        "claim-check reap skipped (",
    ] {
        assert!(
            got.iter()
                .any(|l| l.starts_with(want) && l.contains("permission denied")),
            "{want:?} in {summary}"
        );
    }
    assert!(
        exists(&owner, "attr_instances_2001w02").await,
        "nothing dropped: {summary}"
    );
    limited.close().await;
    owner.close().await;
}

/// A pass held on `blocked` (a table another session locks exclusively) while its lease's session
/// is terminated: the summary it returns once the lock goes. `None` where the server has
/// timescaledb, which the partitioned pass does not run on.
async fn pass_whose_lease_session_dies(url: &str, tag: &str, blocked: &str) -> Option<String> {
    let own = own_database(url, tag, &[]).await;
    let owner = pg::connect(&own, 3).await.expect("connect+migrate");
    if maintenance::detect_temporal_backend(&owner).await != Ok(TemporalBackend::Partitioned) {
        eprintln!("SKIP: new databases get timescaledb here (template1)");
        return None;
    }
    let mut blocker = owner.begin().await.expect("begin");
    blocker
        .execute(sqlx::AssertSqlSafe(format!(
            "LOCK TABLE {blocked} IN ACCESS EXCLUSIVE MODE"
        )))
        .await
        .expect("lock");
    let pass_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&own)
        .await
        .expect("pass pool");
    let running = pass_pool.clone();
    let pass = tokio::spawn(async move {
        temporal_maintenance(
            &running,
            TemporalBackend::Partitioned,
            Some(30),
            Duration::ZERO,
        )
        .await
    });
    let mut waiting = 0i64;
    for _ in 0..400 {
        waiting = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
              WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(&owner)
        .await
        .expect("activity");
        if waiting > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(waiting > 0, "the pass never reached the locked table");
    let lease_pid: i32 = sqlx::query_scalar(
        "SELECT l.pid FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid
          WHERE l.locktype = 'advisory' AND l.granted AND a.state = 'idle'
            AND a.datname = current_database()
          LIMIT 1",
    )
    .fetch_one(&owner)
    .await
    .expect("the lease's session");
    let ended: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(lease_pid)
        .fetch_one(&owner)
        .await
        .expect("terminate");
    assert!(ended);
    blocker.rollback().await.expect("unlock");
    let summary = pass.await.expect("pass task").expect("pass");
    pass_pool.close().await;
    owner.close().await;
    Some(summary)
}

/// A pass whose lease's session dies while it waits on the superseded versions, or on the
/// Entities, stops at the next step boundary and says so instead of running on unprotected.
#[tokio::test(flavor = "multi_thread")]
async fn a_pass_whose_lease_session_dies_stops_and_says_so() {
    let url = require_db!();
    for (tag, blocked) in [
        ("leaseversions", "attr_versions"),
        ("leaseentities", "entities"),
    ] {
        let Some(summary) = pass_whose_lease_session_dies(&url, tag, blocked).await else {
            return;
        };
        assert!(
            lines(&summary).last() == Some(&"stopped: the lease was lost"),
            "{blocked}: {summary}"
        );
    }
}

/// The hypertable pass: platform retention drops old chunks (never partitions), and a pass
/// without retention and without work says so. Runs only where the server has timescaledb.
#[tokio::test(flavor = "multi_thread")]
async fn a_hypertable_pass_drops_old_chunks_and_otherwise_has_nothing_to_do() {
    let url = require_db!();
    let admin = PgPool::connect(&url).await.expect("connect");
    let available: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'timescaledb')",
    )
    .fetch_one(&admin)
    .await
    .expect("extensions");
    admin.close().await;
    if !available {
        eprintln!("SKIP: no timescaledb on this server");
        return;
    }
    let own = own_database(
        &url,
        "hyper",
        &["CREATE EXTENSION IF NOT EXISTS timescaledb"],
    )
    .await;
    let pool = pg::connect(&own, 4).await.expect("connect+migrate");
    assert_eq!(
        maintenance::detect_temporal_backend(&pool).await,
        Ok(TemporalBackend::Hypertable)
    );
    pg::ensure_tenant(
        &pool,
        &antares_model::TenantId::new(TENANT).expect("tenant"),
    )
    .await
    .expect("tenant row");
    // the first pass builds the expiry index on the one leaf; the next finds nothing to do
    let first = temporal_maintenance(&pool, TemporalBackend::Hypertable, None, Duration::ZERO)
        .await
        .expect("first pass");
    assert_eq!(first, "expiry index built on 1 of 1 leaves");
    let quiet = temporal_maintenance(&pool, TemporalBackend::Hypertable, None, Duration::ZERO)
        .await
        .expect("quiet pass");
    assert_eq!(quiet, "nothing to do");

    instance(
        &pool,
        "attr_instances",
        "urn:i:old",
        "2002-06-01T00:00:00Z",
        None,
    )
    .await;
    superseded_version(&pool, "2002-06-01T00:00:00Z").await;
    let summary =
        temporal_maintenance(&pool, TemporalBackend::Hypertable, Some(30), Duration::ZERO)
            .await
            .expect("retention pass");
    let got = lines(&summary);
    for want in [
        "timescale drop_chunks older than 30d",
        "deleted 1 superseded versions older than 30d",
    ] {
        assert!(got.contains(&want), "{want:?} in {summary}");
    }
    assert_eq!(
        count(&pool, "SELECT count(*) FROM attr_instances").await,
        0,
        "the 2002 chunk went: {summary}"
    );
    pool.close().await;
}

/// The probe names the backend or refuses: a database the migrations never reached, a plain
/// `attr_instances` (a hypertable whose extension went, or one never made), and a pool that no
/// longer answers.
#[tokio::test(flavor = "multi_thread")]
async fn the_backend_probe_refuses_what_it_cannot_name() {
    let url = require_db!();
    let own = own_database(&url, "probe", &[]).await;
    let raw = PgPool::connect(&own).await.expect("connect");
    let err = maintenance::detect_temporal_backend(&raw)
        .await
        .expect_err("no attr_instances");
    assert_eq!(
        err,
        "attr_instances has unexpected relkind None — migrations did not run?"
    );

    raw.execute("CREATE TABLE attr_instances (x int)")
        .await
        .expect("plain table");
    let with_timescale = raw
        .execute("CREATE EXTENSION IF NOT EXISTS timescaledb")
        .await
        .is_ok();
    let err = maintenance::detect_temporal_backend(&raw)
        .await
        .expect_err("a plain table is no backend");
    if with_timescale {
        assert!(err.starts_with("attr_instances is a plain table"), "{err}");
    } else {
        // without the extension its catalog view does not exist: the probe's own query fails
        assert!(err.contains("timescaledb_information"), "{err}");
    }

    raw.close().await;
    let err = maintenance::detect_temporal_backend(&raw)
        .await
        .expect_err("a closed pool");
    assert!(err.contains("closed"), "{err}");
}
