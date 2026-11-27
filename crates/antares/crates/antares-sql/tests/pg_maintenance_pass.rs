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

/// Statements run as the service under Tenant [`TENANT`], in one transaction.
async fn as_service(pool: &PgPool, sqls: &[&'static str]) {
    let mut tx = pool.begin().await.expect("begin");
    pg::set_service(&mut tx).await.expect("service");
    sqlx::query("SELECT set_config('antares.tenant', $1, true)")
        .bind(TENANT)
        .execute(&mut *tx)
        .await
        .expect("tenant");
    for sql in sqls {
        tx.execute(*sql).await.expect(sql);
    }
    tx.commit().await.expect("commit");
}

/// A session of its own holding the advisory lock `name`, as another pod's step does.
async fn other_pod(pool: &PgPool, name: &str) -> sqlx::PgConnection {
    let mut conn = pool.acquire().await.expect("a connection").detach();
    sqlx::query("SELECT pg_advisory_lock(hashtext($1))")
        .bind(name)
        .execute(&mut conn)
        .await
        .expect("lock");
    conn
}

/// The other pod gives its locks back and goes.
async fn other_pod_leaves(mut conn: sqlx::PgConnection) {
    sqlx::query("SELECT pg_advisory_unlock_all()")
        .execute(&mut conn)
        .await
        .expect("unlock");
    sqlx::Connection::close(conn).await.expect("close");
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

/// The claim row held by another session (a pod mid-pass, `FOR KEY SHARE` lets this one's
/// period claim through) keeps this pass and the retention drop off the partitions: the pass
/// says so, the drop leaves the expired week, and the drop runs once the row is free.
#[tokio::test(flavor = "multi_thread")]
async fn a_claim_row_another_session_holds_skips_the_pass_and_the_partition_drop() {
    let url = require_db!();
    let own = own_database(&url, "claimheld", &[]).await;
    let pool = pg::connect(&own, 4).await.expect("connect+migrate");
    if maintenance::detect_temporal_backend(&pool).await != Ok(TemporalBackend::Partitioned) {
        eprintln!("SKIP: new databases get timescaledb here (template1)");
        return;
    }
    pool.execute(
        "CREATE TABLE attr_instances_2001w02 PARTITION OF attr_instances
         FOR VALUES FROM ('2001-01-08') TO ('2001-01-15')",
    )
    .await
    .expect("old week");
    let mut holder = pool.begin().await.expect("begin");
    holder
        .execute("SELECT 1 FROM maintenance_jobs WHERE name = 'temporal_partitions' FOR KEY SHARE")
        .await
        .expect("the claim row");
    let summary = temporal_maintenance(&pool, TemporalBackend::Partitioned, None, Duration::ZERO)
        .await
        .expect("pass");
    assert_eq!(summary, "skipped: another instance holds the claim");
    let floors = [(TENANT.to_owned(), "2999-01-01T00:00:00Z".to_owned())];
    let kept = maintenance::drop_retained_partitions(&pool, &floors)
        .await
        .expect("drop");
    assert!(kept.is_empty(), "{kept:?}");
    assert!(exists(&pool, "attr_instances_2001w02").await);
    holder.rollback().await.expect("release");
    let dropped = maintenance::drop_retained_partitions(&pool, &floors)
        .await
        .expect("drop");
    assert!(
        !exists(&pool, "attr_instances_2001w02").await,
        "{dropped:?}"
    );
    pool.close().await;
}

/// A history walk of a type that runs out of its time on a full chunk stops after that chunk
/// and the next call goes on from there; a walk past the last Entity starts over.
#[tokio::test(flavor = "multi_thread")]
async fn a_history_walk_out_of_time_resumes_after_the_chunk_it_reached() {
    use antares_store::HistoryScope;
    let url = require_db!();
    let own = own_database(&url, "walk", &[]).await;
    let pool = pg::connect(&own, 4).await.expect("connect+migrate");
    as_service(
        &pool,
        &[
            "INSERT INTO temporal_entities (tenant_id, id, types, meta, created_at, modified_at)
           SELECT 'pass', 'urn:p:e' || CASE WHEN n = 0 THEN '' ELSE (n - 1)::text END,
                  ARRAY['https://example.org/Walk'], '{}'::jsonb, now(), now()
             FROM generate_series(0, 9) n",
        ],
    )
    .await;
    let scope = || HistoryScope::Type {
        type_iri: "https://example.org/Walk",
        except_ids: &[],
    };
    let before = "2026-01-01T00:00:00Z";
    // the superseded versions held past the walk's time, while it reads its first chunk
    let mut holder = pool.begin().await.expect("begin");
    holder
        .execute("LOCK TABLE attr_versions IN ACCESS EXCLUSIVE MODE")
        .await
        .expect("lock");
    let walking = pool.clone();
    let walk = tokio::spawn(async move {
        maintenance::retain_history(&walking, TENANT, scope(), before, 100, false).await
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    holder.rollback().await.expect("unlock");
    assert_eq!(walk.await.expect("walk task").expect("walk"), 0);

    // an old row of the first Entity: the next call starts past it, the one after that again
    instance(
        &pool,
        "attr_instances",
        "urn:i:walk",
        "2002-06-01T00:00:00Z",
        None,
    )
    .await;
    let resumed = maintenance::retain_history(&pool, TENANT, scope(), before, 100, false)
        .await
        .expect("resumed walk");
    assert_eq!(resumed, 0, "the walk went on after the tenth Entity");
    let again = maintenance::retain_history(&pool, TENANT, scope(), before, 100, false)
        .await
        .expect("walk from the start");
    assert_eq!(again, 1);
    pool.close().await;
}

/// The audit index step: another pod holding it builds nothing here, an index a failed build
/// left invalid is dropped and built again, and a name another relation took fails the step.
#[tokio::test(flavor = "multi_thread")]
async fn the_audit_index_step_yields_rebuilds_an_invalid_index_and_fails_on_a_taken_name() {
    let url = require_db!();
    let own = own_database(&url, "auditindex", &[]).await;
    let pool = pg::connect(&own, 4).await.expect("connect+migrate");
    if maintenance::detect_temporal_backend(&pool).await != Ok(TemporalBackend::Partitioned) {
        eprintln!("SKIP: new databases get timescaledb here (template1)");
        return;
    }
    pool.execute(
        "CREATE TABLE attr_instances_2001w02 PARTITION OF attr_instances
         FOR VALUES FROM ('2001-01-08') TO ('2001-01-15')",
    )
    .await
    .expect("old week");
    for (id, at) in [
        ("urn:i:a1", "2001-01-09T00:00:00Z"),
        ("urn:i:a2", "2001-01-10T00:00:00Z"),
    ] {
        instance(&pool, "attr_instances", id, at, None).await;
    }
    let pod = other_pod(&pool, "antares.history_audit_index").await;
    assert_eq!(
        maintenance::history_audit_index(&pool)
            .await
            .expect("held step"),
        None
    );
    other_pod_leaves(pod).await;

    // a build that failed (two rows of one Tenant under a unique index) leaves it invalid
    pool.execute(
        "CREATE UNIQUE INDEX CONCURRENTLY attr_instances_2001w02_audit
         ON attr_instances_2001w02 (tenant_id)",
    )
    .await
    .expect_err("a duplicate Tenant");
    let line = maintenance::history_audit_index(&pool)
        .await
        .expect("audit step")
        .expect("an index to build");
    assert!(line.starts_with("audit index built on "), "{line}");
    let valid: bool = sqlx::query_scalar(
        "SELECT indisvalid FROM pg_index
          WHERE indexrelid = 'public.attr_instances_2001w02_audit'::regclass",
    )
    .fetch_one(&pool)
    .await
    .expect("catalog");
    assert!(valid, "{line}");

    for sql in [
        "CREATE TABLE attr_instances_2001w03 PARTITION OF attr_instances
         FOR VALUES FROM ('2001-01-15') TO ('2001-01-22')",
        "CREATE SEQUENCE attr_instances_2001w03_audit",
    ] {
        pool.execute(sql).await.expect(sql);
    }
    let taken = maintenance::history_audit_index(&pool)
        .await
        .expect_err("the name is taken");
    assert!(taken.to_string().contains("already exists"), "{taken}");
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

/// What a pass reports beside its partitions: a DEFAULT partition grown past incidental, the
/// claim-check rows it reaped, nothing of a reap another process holds, and a leaf whose reap
/// the database refused, by name.
#[tokio::test(flavor = "multi_thread")]
async fn a_pass_reports_a_full_default_partition_claim_checks_a_held_reap_and_a_refused_leaf() {
    let url = require_db!();
    let own = own_database(&url, "report", &[]).await;
    let pool = pg::connect(&own, 4).await.expect("connect+migrate");
    if maintenance::detect_temporal_backend(&pool).await != Ok(TemporalBackend::Partitioned) {
        eprintln!("SKIP: new databases get timescaledb here (template1)");
        return;
    }
    as_service(
        &pool,
        &[
            "INSERT INTO attr_instances (tenant_id, entity_id, attr_id, instance_id, observed_at,
                                         created_at, modified_at, data)
             SELECT 'pass', 'urn:p:e', 'https://example.org/speed', 'urn:i:pile' || n,
                    timestamptz '2002-06-01Z' + n * interval '1 second', '2002-06-01Z',
                    '2002-06-01Z', '{\"type\": \"Property\", \"value\": 1}'::jsonb
               FROM generate_series(1, 120000) n",
            "INSERT INTO attr_instances (tenant_id, entity_id, attr_id, instance_id, observed_at,
                                         created_at, modified_at, data)
             VALUES ('pass', 'urn:p:e', 'https://example.org/speed', 'urn:i:refused',
                     '2002-07-01Z', '2002-07-01Z', '2002-07-01Z',
                     '{\"type\": \"Property\", \"value\": 1, \"expiresAt\": \"2001-01-01T00:00:00Z\"}')",
            "INSERT INTO outbox (tenant_id, event, published_at)
             VALUES ('pass', '{}'::jsonb, now() - interval '25 hours')",
        ],
    )
    .await;
    for sql in [
        "ANALYZE attr_instances_default",
        "CREATE FUNCTION antares_test_refuse_reap() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'reap refused'; END $$",
        "CREATE TRIGGER antares_test_refuse_reap BEFORE DELETE ON attr_instances_default
         FOR EACH ROW WHEN (OLD.instance_id = 'urn:i:refused')
         EXECUTE FUNCTION antares_test_refuse_reap()",
    ] {
        pool.execute(sql).await.expect(sql);
    }
    let reaper = other_pod(&pool, "antares.history_reap").await;
    let held = temporal_maintenance(&pool, TemporalBackend::Partitioned, None, Duration::ZERO)
        .await
        .expect("pass");
    other_pod_leaves(reaper).await;
    let got = lines(&held);
    assert!(
        got.iter().any(|l| l.starts_with("default partition ~")),
        "{held}"
    );
    assert!(
        got.contains(&"reaped 1 published claim-check rows"),
        "{held}"
    );
    assert!(
        !got.iter().any(|l| l.starts_with("instance reap")),
        "the reap is the other process's: {held}"
    );

    let refused = temporal_maintenance(&pool, TemporalBackend::Partitioned, None, Duration::ZERO)
        .await
        .expect("pass");
    assert!(
        lines(&refused).iter().any(|l| {
            l.starts_with("instance reap skipped on attr_instances_default (")
                && l.contains("reap refused")
        }),
        "{refused}"
    );
    pool.close().await;
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
        ("leaseclaim", "attr_instances"),
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

/// The hypertable pass against timescale's API as a stand-in gives it (`drop_chunks` and the
/// chunk catalog, answering nothing): retention drops the chunks of both history parents, on a
/// server without timescaledb too.
#[tokio::test(flavor = "multi_thread")]
async fn a_hypertable_pass_hands_retention_to_drop_chunks_for_both_parents() {
    let url = require_db!();
    let own = own_database(&url, "chunkapi", &[]).await;
    let pool = pg::connect(&own, 4).await.expect("connect+migrate");
    if maintenance::detect_temporal_backend(&pool).await != Ok(TemporalBackend::Partitioned) {
        eprintln!("SKIP: new databases get timescaledb here (template1)");
        return;
    }
    for sql in [
        "CREATE FUNCTION public.drop_chunks(relation regclass, older_than interval)
         RETURNS SETOF text LANGUAGE sql AS $$ SELECT NULL::text WHERE false $$",
        "CREATE SCHEMA timescaledb_information",
        "CREATE VIEW timescaledb_information.chunks AS
         SELECT NULL::name AS chunk_schema, NULL::name AS chunk_name, false AS is_compressed
          WHERE false",
    ] {
        pool.execute(sql).await.expect(sql);
    }
    let summary =
        temporal_maintenance(&pool, TemporalBackend::Hypertable, Some(30), Duration::ZERO)
            .await
            .expect("retention pass");
    assert!(
        lines(&summary).contains(&"timescale drop_chunks older than 30d"),
        "{summary}"
    );
    assert!(
        !summary.contains("partition "),
        "a hypertable has no weeks to make: {summary}"
    );
    pool.close().await;
}

/// A week the pass would make waits while an index build holds the DEFAULT partition (and the
/// expiry build leaves that DEFAULT alone too); once the build is gone the weeks are made, on
/// `attr_rows` from the cut-over on.
#[tokio::test(flavor = "multi_thread")]
async fn weeks_wait_for_a_default_index_build_and_are_keyed_from_the_cut_over() {
    let url = require_db!();
    let own = own_database(&url, "keyedweeks", &[]).await;
    let pool = pg::connect(&own, 4).await.expect("connect+migrate");
    if maintenance::detect_temporal_backend(&pool).await != Ok(TemporalBackend::Partitioned) {
        eprintln!("SKIP: new databases get timescaledb here (template1)");
        return;
    }
    pool.execute("UPDATE history_cutover SET at = date_trunc('week', now()) - interval '2 weeks'")
        .await
        .expect("cut-over");
    let suffix: String =
        sqlx::query_scalar("SELECT to_char(date_trunc('week', now()), 'IYYY\"w\"IW')")
            .fetch_one(&pool)
            .await
            .expect("this week");
    let build = other_pod(&pool, "antares.history_default_index").await;
    let waiting = temporal_maintenance(&pool, TemporalBackend::Partitioned, None, Duration::ZERO)
        .await
        .expect("pass");
    other_pod_leaves(build).await;
    let weeks: Vec<&str> = lines(&waiting)
        .into_iter()
        .filter(|l| l.starts_with("partition "))
        .collect();
    assert_eq!(weeks.len(), 5, "{waiting}");
    assert!(
        weeks
            .iter()
            .all(|l| l.ends_with(": waits for the DEFAULT partition's index")),
        "{waiting}"
    );
    assert!(!exists(&pool, &format!("attr_rows_{suffix}")).await);

    let made = temporal_maintenance(&pool, TemporalBackend::Partitioned, None, Duration::ZERO)
        .await
        .expect("pass");
    assert!(
        lines(&made).contains(&format!("partition {suffix}: ok").as_str()),
        "{made}"
    );
    assert!(
        exists(&pool, &format!("attr_rows_{suffix}")).await,
        "{made}"
    );
    assert!(
        !exists(&pool, &format!("attr_instances_{suffix}")).await,
        "{made}"
    );
    pool.close().await;
}

/// An expired Entity another session holds is left to the next batch, which takes it; a bare
/// sweep without budget stops before its first Tenant and resumes at it.
#[tokio::test(flavor = "multi_thread")]
async fn a_locked_expired_entity_waits_for_the_next_batch_and_a_spent_bare_sweep_resumes() {
    let url = require_db!();
    let own = own_database(&url, "lockedreap", &[]).await;
    let pool = pg::connect(&own, 4).await.expect("connect+migrate");
    let tenant = antares_model::TenantId::new(TENANT).expect("tenant");
    pg::ensure_tenant(&pool, &tenant).await.expect("tenant row");
    let id = "urn:ngsi-ld:T:lockedreap";
    let doc = serde_json::json!({"id": id, "type": ["https://example.org/T"],
        "createdAt": "2001-01-01T00:00:00Z", "modifiedAt": "2001-01-01T00:00:00Z",
        "expiresAt": "2001-01-02T00:00:00Z"});
    assert!(pg::entity::PgEntityStore::new(pool.clone())
        .create(&tenant, id, &doc)
        .await
        .expect("create"));
    let mut holder = pool.begin().await.expect("begin");
    pg::set_tenant(&mut holder, &tenant).await.expect("tenant");
    sqlx::query("SELECT 1 FROM entities WHERE tenant_id = $1 AND id = $2 FOR UPDATE")
        .bind(TENANT)
        .bind(id)
        .execute(&mut *holder)
        .await
        .expect("row lock");
    assert_eq!(
        maintenance::reap_expired_entity_batch(&pool, 10)
            .await
            .expect("batch"),
        0
    );
    holder.rollback().await.expect("release");
    assert_eq!(
        maintenance::reap_expired_entity_batch(&pool, 10)
            .await
            .expect("batch"),
        1
    );

    for _ in 0..3 {
        let sweep = maintenance::mark_bare(&pool, Duration::ZERO)
            .await
            .expect("sweep");
        assert_eq!((sweep.checked, sweep.marked), (0, 0), "{sweep:?}");
    }
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
