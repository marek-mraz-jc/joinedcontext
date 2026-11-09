// SPDX-License-Identifier: EUPL-1.2
//! Postgres foundation: ONE shared pool, embedded migrations,
//! transaction-scoped tenancy. Store implementations build on top.

pub mod doc;
pub mod entity;
pub mod maintenance;
pub mod outbox;
pub mod seal;
pub mod temporal;

use antares_model::TenantId;
use serde_json::Value;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Postgres, Transaction};

/// Embedded migrations, run at start (like Scorpio's Flyway, but once).
///
/// `ANTARES_MIGRATE=0`/`false` skips that run on this process, so serving
/// replicas do not race the same DDL and a deployment can migrate once from a
/// separate job or init container against the same database. Unset — the
/// default — migrates on boot exactly as before.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Is this connect failure a database whose migration history can never match
/// this binary? A database still booting is worth waiting for; a history from
/// another release is not, and a caller that retries it reports the timeout
/// instead of the cause. Only the history errors are permanent: an execution
/// failure can be a lock the next attempt gets.
/// The filter values a stored text column can equal. PostgreSQL `text` holds
/// no NUL, so a request string carrying one names nothing the store holds;
/// binding it would raise `22021` and turn a query that matches nothing into
/// an error (T-3464). Callers bind what this keeps.
pub(crate) fn storable<S: AsRef<str>>(values: &[S]) -> Vec<String> {
    values
        .iter()
        .map(AsRef::as_ref)
        .filter(|v| !v.contains('\0'))
        .map(str::to_owned)
        .collect()
}

pub fn is_schema_mismatch(e: &sqlx::Error) -> bool {
    use sqlx::migrate::MigrateError::{
        Dirty, VersionMismatch, VersionMissing, VersionNotPresent, VersionTooNew, VersionTooOld,
    };
    match e {
        sqlx::Error::Migrate(m) => matches!(
            **m,
            VersionMissing(_)
                | VersionMismatch(_)
                | VersionNotPresent(_)
                | VersionTooOld(..)
                | VersionTooNew(..)
                | Dirty(_)
        ),
        _ => false,
    }
}

/// What the boot path does with the migrations a newer release applied
/// (`newer`: version and description of each applied version above `newest`,
/// this binary's own newest). A rollback starts the previous image on the
/// schema the release it rolls back already migrated, and sqlx refuses every
/// applied version it does not ship, so without this the previous image could
/// never boot (T-5105, docs/src/operations.md "Rollback"). A migration whose file name
/// starts with `breaking_` is one this release cannot serve: the way back is
/// the pre-release backup, so the boot stops with that version. `Ok(true)`
/// means newer, compatible migrations are applied and tolerated.
fn tolerate_newer(
    newest: i64,
    newer: &[(i64, String)],
) -> Result<bool, sqlx::migrate::MigrateError> {
    match newer.iter().find(|(_, d)| d.starts_with("breaking ")) {
        Some((v, _)) => Err(sqlx::migrate::MigrateError::VersionTooNew(*v, newest)),
        None => Ok(!newer.is_empty()),
    }
}

/// The first applied version at or below this binary's newest that `known`
/// (the versions [`MIGRATOR`] ships) lacks: a history from another release,
/// which the lenient run below would otherwise pass over (F-ANT1-012).
fn first_unknown_older(applied: &[i64], known: &[i64]) -> Option<i64> {
    applied.iter().copied().find(|v| !known.contains(v))
}

/// Version and description of each applied migration above `newest`.
async fn newer_migrations(
    conn: &mut sqlx::PgConnection,
    newest: i64,
) -> Result<Vec<(i64, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT version, description FROM _sqlx_migrations WHERE version > $1 ORDER BY version",
    )
    .bind(newest)
    .fetch_all(conn)
    .await
    .or_else(|e| match &e {
        // a database never migrated has no history table yet
        sqlx::Error::Database(d) if d.code().as_deref() == Some("42P01") => Ok(Vec::new()),
        _ => Err(e),
    })
}

/// Run [`MIGRATOR`], tolerating compatible migrations of a newer release
/// ([`tolerate_newer`]). An unknown applied version at or below this binary's
/// newest is still a history from another release and refused as before.
async fn migrate(conn: &mut sqlx::PgConnection) -> Result<(), sqlx::Error> {
    let newest = MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
    let newer = newer_migrations(conn, newest).await?;
    let tolerate = tolerate_newer(newest, &newer).map_err(|e| {
        tracing::error!(
            "the database holds a breaking migration of a newer release ({newer:?}): this \
             release cannot serve it; restore the backup taken before that release"
        );
        sqlx::Error::Migrate(Box::new(e))
    })?;
    if !tolerate {
        return MIGRATOR.run(conn).await.map_err(Into::into);
    }
    tracing::warn!(
        "the database holds migrations {newer:?} of a newer release, all marked compatible: \
         serving it as a rollback; roll forward again or restore the pre-release backup \
         before applying another schema change"
    );
    let applied: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE version <= $1")
            .bind(newest)
            .fetch_all(&mut *conn)
            .await?;
    let known: Vec<i64> = MIGRATOR.iter().map(|m| m.version).collect();
    if let Some(v) = first_unknown_older(&applied, &known) {
        return Err(sqlx::Error::Migrate(Box::new(
            sqlx::migrate::MigrateError::VersionMissing(v),
        )));
    }
    let mut lenient = sqlx::migrate::Migrator::with_migrations(MIGRATOR.iter().cloned().collect());
    lenient.set_ignore_missing(true);
    lenient.run(conn).await.map_err(Into::into)
}

/// The `ANTARES_MIGRATE` switch: off only for an explicit `0`/`false`.
fn migrate_enabled(v: Option<&str>) -> bool {
    !matches!(v, Some("0" | "false"))
}

/// One shared pool for all tenants — never per-tenant pools.
/// `max_connections` ≈ 2× the PG box's cores; the default suits a small dev
/// Postgres, deployments size it via config.
///
/// Every acquisition and session is bounded: a saturated pool fails the one
/// request after 5 s instead of queueing forever; idle/aged connections are
/// recycled; and each session carries `statement_timeout`/`lock_timeout` so
/// a runaway query or lost lock can never wedge a pooled connection.
pub async fn connect(url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    connect_with(url, max_connections, std::time::Duration::from_secs(30)).await
}

/// How long a released connection may take to answer its liveness ping
/// before it is dropped instead of going back to the pool (T-4298).
pub const RELEASE_PING: std::time::Duration = std::time::Duration::from_secs(2);

/// How long a pooled connection may sit idle before an acquire pings it
/// first (T-6606). One released inside this window answered its release
/// ping that long ago at most.
pub const ACQUIRE_PING_IDLE: std::time::Duration = std::time::Duration::from_secs(1);

/// A liveness ping bounded by [`RELEASE_PING`]: `true` keeps the connection.
///
/// A failed ping answers `false`, never `Err` (T-6000). On `Err` sqlx closes
/// "hard": it only flushes and shuts the socket, and over TLS (sqlx 0.9
/// rustls) a flush with nothing to write waits to READ, which on a healthy
/// idle server never comes: the slot was held for good (prod 2026-10-28, 13
/// of 13, each session idle after the ROLLBACK of a request dropped while
/// its statement failed). On `false` sqlx sends Terminate first, the server
/// closes, and the read ends. A ping that ran out may sit on a stalled
/// stream, where no Terminate gets through either: the hook holds the
/// connection only by reference, so a fresh one takes its place and the
/// stalled socket is dropped unflushed; sqlx then closes the fresh one.
async fn bounded_ping(
    conn: &mut sqlx::PgConnection,
    connect: &sqlx::postgres::PgConnectOptions,
) -> bool {
    use sqlx::Connection;
    match tokio::time::timeout(RELEASE_PING, conn.ping()).await {
        Ok(Ok(())) => true,
        Ok(Err(e)) => {
            tracing::debug!(%e, "pooled connection failed its ping; closing it");
            false
        }
        Err(_) => {
            tracing::debug!("pooled connection did not answer its ping; closing it");
            let fresh =
                tokio::time::timeout(RELEASE_PING, sqlx::PgConnection::connect_with(connect)).await;
            if let Ok(Ok(fresh)) = fresh {
                drop(std::mem::replace(conn, fresh));
            }
            false
        }
    }
}

/// How long the server keeps a broker session that sits idle outside a
/// transaction (`idle_session_timeout`, T-6000). A pooled connection never
/// gets there: the pool closes an idle one after [`POOL_IDLE`], and its
/// reaper runs every [`POOL_IDLE`], so it is gone within twice that. A
/// session idle longer is one the pool no longer serves from: prod
/// 2026-10-28 held 13 of 13 such sessions, idle after `ROLLBACK` for 21 min
/// to 11 h, while every acquire timed out. The server ends it, the socket
/// closes, and whatever waited on it (or the pool slot behind it) is freed.
pub const IDLE_SESSION: std::time::Duration = std::time::Duration::from_secs(600);

/// The blocks a broker session's Bitmap Heap Scan reads ahead
/// (`effective_io_concurrency`, T-6277); see `options`.
pub const EFFECTIVE_IO_CONCURRENCY: u32 = 256;

/// How long a connection may sit idle in the pool before the pool closes it.
pub const POOL_IDLE: std::time::Duration = std::time::Duration::from_secs(240);

/// [`connect`] with the per-session `statement_timeout` chosen by the
/// deployment (`ANTARES_PG_STATEMENT_TIMEOUT_MS`).
pub async fn connect_with(
    url: &str,
    max_connections: u32,
    statement_timeout: std::time::Duration,
) -> Result<PgPool, sqlx::Error> {
    let pool = open(url, max_connections, statement_timeout).await?;
    // DDL is exempt from the per-session statement timeout: building an index
    // on a large attr_instances legitimately runs longer than a query ever
    // should. The lock timeout stays — a migration blocked on a lock must
    // fail rather than hold the boot path.
    //
    // `SET` is session-scoped and the pool has no release hook, so the
    // connection is DETACHED first: it never goes back into the pool, and no
    // request is ever served by a connection with the runaway-query wall
    // switched off. The pool opens a fresh one (running `after_connect`) on
    // demand.
    if migrate_enabled(std::env::var("ANTARES_MIGRATE").ok().as_deref()) {
        use sqlx::{Connection, Executor};
        let mut migrate_conn = pool.acquire().await?.detach();
        migrate_conn
            .execute(sqlx::raw_sql("SET statement_timeout = 0"))
            .await?;
        migrate(&mut migrate_conn).await?;
        migrate_conn.close().await?;
    }
    Ok(pool)
}

/// The write reserve (`ANTARES_PG_WRITE_RESERVE`, T-5150): a second pool of
/// `max_connections` with the same session settings and bounds as
/// [`connect_with`], which only the batch writes draw from, so reads queued
/// on the main pool cannot hold a pipeline's batch upsert behind them. It
/// runs no migrations: [`connect_with`] already did, on the main pool.
pub async fn connect_reserve(
    url: &str,
    max_connections: u32,
    statement_timeout: std::time::Duration,
) -> Result<PgPool, sqlx::Error> {
    open(url, max_connections, statement_timeout).await
}

/// A pool with the bounds and session settings every broker pool shares.
async fn open(
    url: &str,
    max_connections: u32,
    statement_timeout: std::time::Duration,
) -> Result<PgPool, sqlx::Error> {
    let connect: sqlx::postgres::PgConnectOptions = url.parse()?;
    options(connect.clone(), max_connections, statement_timeout)
        .connect_with(connect)
        .await
}

fn options(
    connect: sqlx::postgres::PgConnectOptions,
    max_connections: u32,
    statement_timeout: std::time::Duration,
) -> PgPoolOptions {
    use std::time::Duration;
    // No parallel workers: a request reads a page, and a page whose filter
    // the planner thinks selective (a `q` under RLS is never an index
    // condition) got a Gather of two workers, two processes started per
    // statement. On the distinct-1000 rig (2 PostgreSQL cores, 1 000 req/s)
    // that took a `q` page from 2.5 ms to 106 ms (T-5136).
    // No JIT: a `q` page over `LIVE_ENTITY` is costed at ~5e8 by its expiry
    // subplans, far past `jit_above_cost`, so PostgreSQL compiled ~900 LLVM
    // functions (4.4 s) for a page that runs in 1 ms, and a compile never
    // checks for interrupts, so `statement_timeout` could not end it (T-5904).
    // Prefetch: a `q` or a count over a type reads its rows by a Bitmap Heap
    // Scan, and the rows of one type lie scattered over the heap (no update
    // is HOT: an index reads `entity`, and pipelines move `expires_at` daily).
    // At PostgreSQL 16's default of 1 that scan waits on each block in turn:
    // dev's 67 742 Helsinki Trees sat on 56 819 blocks, and a count with a
    // `q` read them in 10.6-15.9 s, against 2.1 s at 64 and 1.6 s at 256
    // (T-6277). Without parallel workers this is the read's only I/O depth.
    let session = format!(
        "SET statement_timeout = '{}ms'; SET lock_timeout = '5s'; \
         SET max_parallel_workers_per_gather = 0; SET jit = off; \
         SET effective_io_concurrency = {EFFECTIVE_IO_CONCURRENCY}; \
         SET idle_session_timeout = '{}ms'",
        statement_timeout.as_millis(),
        IDLE_SESSION.as_millis()
    );
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(POOL_IDLE)
        .max_lifetime(Duration::from_secs(1800))
        // sqlx tests a released connection with a ping it never times out,
        // and holds its pool slot until the ping answers. Through the mesh
        // proxy, which accepts the bytes, a stream whose far side stalled
        // never answers: on dev each such stream held a slot for hours, until
        // all 20 were gone and every read answered 503 (T-4298). This ping is
        // bounded; one that fails or runs out drops the socket at once, and
        // the slot opens a fresh connection on demand. sqlx's own ping after
        // it costs one more round trip on a connection that just answered.
        // How a failed or stalled ping is closed: [`bounded_ping`] (T-6000).
        .after_release({
            let connect = connect.clone();
            move |conn, _meta| {
                let connect = connect.clone();
                Box::pin(async move { Ok(bounded_ping(conn, &connect).await) })
            }
        })
        // sqlx's own acquire test is the same unbounded ping, run on every
        // acquire, so a connection released a moment ago, and pinged twice
        // then, paid a third round trip before its next statement (T-6606,
        // R2-003). The hook below pings only a connection idle for
        // [`ACQUIRE_PING_IDLE`] or longer, the one a server restart or a
        // stalled stream may have taken while it sat in the pool, and bounds
        // that ping as the release hook does.
        .test_before_acquire(false)
        .before_acquire(move |conn, meta| {
            let connect = connect.clone();
            Box::pin(async move {
                if meta.idle_for < ACQUIRE_PING_IDLE {
                    return Ok(true);
                }
                Ok(bounded_ping(conn, &connect).await)
            })
        })
        .after_connect(move |conn, _meta| {
            let session = session.clone();
            Box::pin(async move {
                use sqlx::Executor;
                // the only interpolated value is a validated integer of
                // milliseconds (parsed by the broker), never client text
                conn.execute(sqlx::raw_sql(sqlx::AssertSqlSafe(session)))
                    .await?;
                Ok(())
            })
        })
}

/// Make RLS effective for this transaction — SET LOCAL only (transaction
/// scoped), so a recycled pooled connection carries no tenant residue.
/// Call first in EVERY transaction that touches tenant data.
pub async fn set_tenant(
    tx: &mut Transaction<'_, Postgres>,
    tenant: &TenantId,
) -> Result<(), sqlx::Error> {
    sqlx::query(crate::SET_TENANT_SQL)
        .bind(tenant.as_str())
        .execute(&mut **tx)
        .await
        .map(|_| ())
}

/// Does the connected role bypass RLS (superuser or BYPASSRLS)? RLS is
/// a belt only when the role wears it — the broker warns at startup when the
/// belt is off.
///
/// A probe that cannot answer fails CLOSED: an unreachable or erroring
/// database is reported as bypassing, so the strict gate refuses the boot
/// instead of passing on an error.
pub async fn role_bypasses_rls(pool: &PgPool) -> bool {
    bypasses(
        sqlx::query_scalar(
            "SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user",
        )
        .fetch_one(pool)
        .await,
    )
}

/// What the connected server is, read ONCE at startup and served from
/// `/q/health` afterwards: server version plus the version of each extension
/// the broker's behaviour depends on. A probe that fails is not fatal — the
/// broker serves fine without knowing its own server version — so the caller
/// gets an empty object and health simply says nothing.
pub async fn version_info(pool: &PgPool) -> Value {
    // The pool's own shape is what an operator sizes against, and it is
    // known without asking the server: how many connections this process may
    // hold, and how long a request waits for one before the broker answers
    // 503. Both are reported even when the version probe fails.
    let mut m = serde_json::Map::new();
    m.insert("engine".into(), "postgres".into());
    m.insert(
        "poolSize".into(),
        pool.options().get_max_connections().into(),
    );
    m.insert(
        "poolAcquireTimeoutSeconds".into(),
        pool.options().get_acquire_timeout().as_secs().into(),
    );
    let row: Result<(String, Option<String>, Option<String>), sqlx::Error> = sqlx::query_as(
        "SELECT current_setting('server_version'), \
         (SELECT extversion FROM pg_extension WHERE extname = 'postgis'), \
         (SELECT extversion FROM pg_extension WHERE extname = 'timescaledb')",
    )
    .fetch_one(pool)
    .await;
    match row {
        Ok((server, postgis, timescale)) => {
            m.insert("server".into(), server.into());
            if let Some(v) = postgis {
                m.insert("postgis".into(), v.into());
            }
            if let Some(v) = timescale {
                m.insert("timescaledb".into(), v.into());
            }
        }
        Err(e) => {
            tracing::warn!("server version probe failed ({e}); /q/health will not report it");
        }
    }
    Value::Object(m)
}

/// Open a transaction, timing what it cost to get one. `Pool::begin` is the
/// only way to a `Transaction<'static>` (sqlx owns the pooled connection
/// inside it), so the measurement covers the pool wait plus one BEGIN round
/// trip; the round trip is sub-millisecond and the wait is what grows, up to
/// the acquire timeout. Every transactional store call goes through here, so
/// `antares_pg_transaction_begin_seconds` is where pool pressure shows.
pub(crate) async fn begin(
    pool: &PgPool,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, sqlx::Error> {
    let t0 = std::time::Instant::now();
    let tx = pool.begin().await;
    metrics::histogram!("antares_pg_transaction_begin_seconds").record(t0.elapsed().as_secs_f64());
    tx
}

fn bypasses(probe: Result<bool, sqlx::Error>) -> bool {
    probe.unwrap_or_else(|e| {
        tracing::error!(
            "row-level-security probe failed ({e}) — treating the role as RLS-bypassing"
        );
        true
    })
}

/// Arm the transaction-scoped `antares.service` escape (0001_init.sql) for
/// the two internal cross-tenant jobs: outbox drain and temporal retention.
/// NEVER call from a request path — request queries carry explicit tenant
/// predicates and run under `set_tenant` only.
pub async fn set_service(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('antares.service', 'on', true)")
        .execute(&mut **tx)
        .await
        .map(|_| ())
}

/// The write's own claim on its tenant (ADR-0001: "every implicit tenant
/// creation inserts its row in the same transaction as the document"). Two
/// things ride on it being IN the document's transaction rather than before
/// it: the tenant row and the rows it accounts for commit together, and the
/// claim holds a lock the purge's exclusive one conflicts with,
/// so the purge waits for an in-flight write instead of stepping over it.
/// Committed separately, a write racing a purge leaves rows behind for a
/// tenant the inventory no longer names — readable to whoever sends that
/// tenant header, listed by nothing, and reclaimable by no further purge.
///
/// The lock is SHARED. An upsert whose `DO UPDATE` re-set the tenant id took
/// the exclusive lock and wrote a row version on every write, so writes in
/// one tenant serialised behind each other and a table with one row per
/// tenant collected one dead tuple per write in the broker. A shared lock
/// conflicts with the purge's exclusive one exactly as that did, and is
/// compatible with itself, so concurrent writers hold it together.
///
/// Idempotent, so two concurrent first writes both succeed (vs Scorpio's
/// CREATE DATABASE + Flyway deadlock).
/// The purge's side of the tenant fence [`claim_tenant`] holds shared: one
/// bound parameter, the tenant id.
pub const PURGE_FENCE_SQL: &str =
    "SELECT pg_advisory_xact_lock(hashtext('antares.tenant'), hashtext($1))";

pub async fn claim_tenant(
    tx: &mut Transaction<'_, Postgres>,
    tenant: &TenantId,
) -> Result<(), sqlx::Error> {
    // A shared advisory lock keyed by the tenant fences the write from a
    // purge, which takes it exclusively (`AnyStore::purge_tenant`). It
    // replaced `FOR SHARE` on the tenant's row (T-6606, R2-005): a row lock
    // writes the locker into the tuple's xmax, so every write of a busy
    // tenant dirtied the one `tenants` page and, with several writers at
    // once, minted a multixact; the advisory lock touches no heap page.
    //
    // One statement: the lock is taken before the INSERT checks its
    // conflict, and that check reads the index as it stands now, not the
    // statement's snapshot, so a purge that committed while this waited
    // finds its row re-inserted here. `DO NOTHING` takes no row lock and
    // writes nothing for a tenant that exists; a concurrent first write
    // waits on the winner's uncommitted index entry, then does nothing.
    sqlx::query(
        "INSERT INTO tenants (tenant_id) \
         SELECT $1 FROM (SELECT pg_advisory_xact_lock_shared(\
             hashtext('antares.tenant'), hashtext($1))) AS fence \
         ON CONFLICT (tenant_id) DO NOTHING",
    )
    .bind(tenant.as_str())
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

/// The same claim outside a document transaction, for the callers that have
/// no document to pair it with (test seeds, the maintenance paths).
pub async fn ensure_tenant(pool: &PgPool, tenant: &TenantId) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO tenants (tenant_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(tenant.as_str())
        .execute(pool)
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{
        bypasses, first_unknown_older, is_schema_mismatch, migrate_enabled, tolerate_newer,
    };

    /// F-ANT1-012: the lenient run of a rollback ignores missing versions,
    /// so an applied version at or below the newest that this binary lacks
    /// is found before it runs.
    #[test]
    fn a_rollback_still_refuses_an_unknown_older_version() {
        assert_eq!(first_unknown_older(&[1, 2, 3], &[1, 2, 3]), None);
        assert_eq!(first_unknown_older(&[1, 5, 2], &[1, 2, 3]), Some(5));
        assert_eq!(first_unknown_older(&[], &[1]), None);
    }

    /// The RLS probe answers a security gate: a probe that errors must read as
    /// "unsafe", never as "the role does not bypass RLS".
    #[test]
    fn rls_probe_fails_closed() {
        assert!(!bypasses(Ok(false)), "a role without BYPASSRLS is safe");
        assert!(bypasses(Ok(true)), "superuser/BYPASSRLS bypasses");
        assert!(
            bypasses(Err(sqlx::Error::PoolTimedOut)),
            "an unanswerable probe must not pass the gate"
        );
        assert!(bypasses(Err(sqlx::Error::RowNotFound)));
    }

    /// A database from another release is a permanent failure; a database
    /// that is not up yet is not. The boot path retries the second and dies on
    /// the first, so the classification decides which cause an operator is
    /// shown — the squash of the pre-1.0 migration set left a 0.1.0 database
    /// reporting "not reachable after 30 s" for a schema it would never match.
    #[test]
    fn a_history_from_another_release_is_permanent_and_a_booting_database_is_not() {
        use sqlx::migrate::MigrateError;
        for e in [
            MigrateError::VersionMissing(5),
            MigrateError::VersionMismatch(1),
            MigrateError::VersionNotPresent(9),
            MigrateError::VersionTooOld(1, 4),
            MigrateError::VersionTooNew(9, 4),
            MigrateError::Dirty(3),
        ] {
            let shown = e.to_string();
            assert!(
                is_schema_mismatch(&sqlx::Error::Migrate(Box::new(e))),
                "{shown} can never be resolved by waiting"
            );
        }
        assert!(
            !is_schema_mismatch(&sqlx::Error::PoolTimedOut),
            "a database still booting is worth another attempt"
        );
        assert!(!is_schema_mismatch(&sqlx::Error::RowNotFound));
        assert!(
            !is_schema_mismatch(&sqlx::Error::Migrate(Box::new(MigrateError::Execute(
                sqlx::Error::PoolTimedOut
            )))),
            "a migration that failed to execute may be a lock the next attempt gets"
        );
    }

    /// A rollback boots on a newer release's compatible migrations and stops
    /// on a breaking one, naming its version (T-5105).
    #[test]
    fn a_rollback_tolerates_compatible_migrations_and_stops_on_a_breaking_one() {
        use sqlx::migrate::MigrateError;
        assert_eq!(tolerate_newer(16, &[]).ok(), Some(false));
        let compatible = [(17, "type counts".to_owned())];
        assert_eq!(tolerate_newer(16, &compatible).ok(), Some(true));
        let breaking = [
            (17, "type counts".to_owned()),
            (18, "breaking drop names".to_owned()),
        ];
        let refused = tolerate_newer(16, &breaking).err().map(|e| e.to_string());
        assert_eq!(
            refused,
            Some(MigrateError::VersionTooNew(18, 16).to_string())
        );
        let named = [(17, "not breaking yet".to_owned())];
        assert_eq!(tolerate_newer(16, &named).ok(), Some(true));
    }

    /// Migrations stay on unless a deployment explicitly turns them off.
    #[test]
    fn migrate_switch_defaults_on() {
        assert!(migrate_enabled(None));
        assert!(migrate_enabled(Some("1")));
        assert!(migrate_enabled(Some("true")));
        assert!(!migrate_enabled(Some("0")));
        assert!(!migrate_enabled(Some("false")));
    }
}
