// SPDX-License-Identifier: EUPL-1.2
//! Temporal maintenance: the broker's own scheduled
//! job replaces TimescaleDB background workers in plain mode, and drives the
//! retention knob in both modes.
//!
//! Single-winner rule (S5-29): a pass starts with [`claim_period`], which
//! moves the job's `last_run` only when the period since it has passed, so
//! of N instances ticking on their own timers one runs per period; the
//! winner holds a session advisory lock for the whole pass, and a step that
//! finds the lease gone stops the pass. No coordinator.
//!
//! Plain-mode partitioning: weekly partitions are pre-created for a window
//! around now; everything else (historic backfill) lands in the DEFAULT
//! partition. Creating a partition whose range already has rows sitting in
//! the DEFAULT partition fails in PostgreSQL, so a single row written with an
//! `observedAt` past the window permanently blocks that week's partition and
//! sends all of its later traffic to DEFAULT as well. Such a range is
//! recovered rather than skipped: the rows are moved out of DEFAULT into a
//! standalone table, which is then ATTACHed as the partition.
//!
//! The recovery belongs here and NOT at ingest, for two reasons. The first is
//! internal: clamping an `observedAt` outside a horizon would let the
//! `observed_at` column disagree with the raw timestamp string in `data`, and
//! `compile::temporal::column_range_bound` may prune on that column only
//! because it is a superset of the byte-exact text window (4.11). The second
//! is normative: 4.8 defines `observedAt` as "the temporal Property at which a
//! certain Property or Relationship became valid or was observed" and requires
//! only that it be a 4.6.3 DateTime — there is no horizon and no error type
//! for one, and a forecast Property legitimately becomes valid in the future.
//! A well-formed future `observedAt` is therefore valid input the broker
//! stores, not input it may refuse.
//!
//! Residual, deliberate: rows whose `observed_at` lies beyond the pre-created
//! window stay in DEFAULT until their week enters it, and rows dated far
//! enough ahead stay there indefinitely — retention only purges DEFAULT rows
//! that are already OLD. Auto-adopting arbitrary future weeks is not the fix
//! either: one row per week over a century would trade an oversized DEFAULT
//! for thousands of partitions. The condition is reported instead (see
//! `default_partition_load`), so an operator sees it rather than discovering
//! it as a query slowdown.

use sqlx::postgres::PgPool;
use sqlx::{Acquire, Row};

/// True when the timescaledb extension is CREATED in this database
/// (per-database `pg_extension`, not "installed on the server").
pub async fn timescale_present(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT 1 FROM pg_extension WHERE extname = 'timescaledb'")
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// What the migrations actually built `attr_instances` as. Detected ONCE at
/// startup from the catalog and pinned — never re-probed per tick, so the
/// maintenance branch can never disagree with the DDL on disk (the
/// "extension installed after first boot" trap).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemporalBackend {
    /// timescale hypertable (relkind 'r' + timescaledb catalog entry)
    Hypertable,
    /// native PARTITION BY RANGE (relkind 'p')
    Partitioned,
}

/// Inspect the catalog. Errors when `attr_instances` is neither a hypertable
/// nor a partitioned table — that means the database was migrated under one
/// extension state and is now running under another; refusing beats running
/// the wrong maintenance jobs against mismatched DDL.
pub async fn detect_temporal_backend(pool: &PgPool) -> Result<TemporalBackend, String> {
    let relkind: Option<String> = sqlx::query_scalar(
        "SELECT c.relkind::text FROM pg_class c
         WHERE c.relname = 'attr_instances' AND c.relnamespace = 'public'::regnamespace",
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;
    match relkind.as_deref() {
        Some("p") => Ok(TemporalBackend::Partitioned),
        Some("r") => {
            let hyper = sqlx::query(
                "SELECT 1 FROM timescaledb_information.hypertables
                 WHERE hypertable_name = 'attr_instances'",
            )
            .fetch_optional(pool)
            .await
            .map_err(|e| e.to_string())?;
            if hyper.is_some() {
                Ok(TemporalBackend::Hypertable)
            } else {
                Err(
                    "attr_instances is a plain table — the database was migrated as a \
                     hypertable and the timescaledb extension has since been removed, or \
                     the catalog is damaged; refusing to run temporal maintenance"
                        .into(),
                )
            }
        }
        other => Err(format!(
            "attr_instances has unexpected relkind {other:?} — migrations did not run?"
        )),
    }
}

/// How long a published claim-check row outlives its message. The changes
/// stream keeps a message until every durable has acked it, so no bound is
/// derivable from the bus — this is the operator-visible ceiling on how long
/// a matcher may lag before an oversized change stops resolving.
// ponytail: fixed window, an env knob if a deployment's matcher lag exceeds it
const CLAIM_CHECK_HOURS: i64 = 24;

/// One period of a maintenance job, held by this instance: a session
/// advisory lock on one connection kept out of the pool for the pass. A pass
/// checks [`Lease::alive`] before each step; dropping the lease closes the
/// connection, which frees the lock even when the pass ended by an error.
pub struct Lease {
    conn: Option<sqlx::pool::PoolConnection<sqlx::Postgres>>,
    job: String,
}

impl Lease {
    /// Whether the lock's session still answers. A session that is gone
    /// took the lock with it, and another instance may run the job now.
    pub async fn alive(&mut self) -> bool {
        match self.conn.as_mut() {
            Some(conn) => sqlx::query("SELECT 1").execute(&mut **conn).await.is_ok(),
            None => false,
        }
    }

    /// Gives the lock back and the connection to the pool; a connection that
    /// cannot unlock is closed instead, never pooled with the lock held.
    pub async fn release(mut self) {
        let Some(mut conn) = self.conn.take() else {
            return;
        };
        let unlocked: Result<bool, _> =
            sqlx::query_scalar("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
                .bind(&self.job)
                .fetch_one(&mut *conn)
                .await;
        if !matches!(unlocked, Ok(true)) {
            drop(conn.detach());
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            // closing the session is what frees a session lock
            drop(conn.detach());
        }
    }
}

/// The lease on `job` for this period, or `None` when another instance runs
/// it now (its lock is held) or ran it less than `period` ago (`last_run`,
/// on the database's clock, so the instances' clocks do not matter). A job
/// row that does not exist yet is created by its first claim.
pub async fn claim_period(
    pool: &PgPool,
    job: &str,
    period: std::time::Duration,
) -> Result<Option<Lease>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
        .bind(job)
        .fetch_one(&mut *conn)
        .await?;
    if !locked {
        return Ok(None);
    }
    let mut lease = Lease {
        conn: Some(conn),
        job: job.to_owned(),
    };
    let Some(conn) = lease.conn.as_mut() else {
        return Ok(None);
    };
    let won = sqlx::query(
        "INSERT INTO maintenance_jobs (name, last_run) VALUES ($1, now())
         ON CONFLICT (name) DO UPDATE SET last_run = now()
          WHERE maintenance_jobs.last_run IS NULL
             OR maintenance_jobs.last_run <= now() - make_interval(secs => $2)
         RETURNING name",
    )
    .bind(job)
    .bind(period.as_secs_f64())
    .fetch_optional(&mut **conn)
    .await?;
    if won.is_none() {
        lease.release().await;
        return Ok(None);
    }
    Ok(Some(lease))
}

/// One maintenance pass. Returns a short human-readable summary ("skipped"
/// when another instance holds the claim). `retention_days = None` keeps
/// history forever — retention is a deliberate deployment knob, never a
/// default.
pub async fn temporal_maintenance(
    pool: &PgPool,
    backend: TemporalBackend,
    retention_days: Option<i64>,
    period: std::time::Duration,
) -> Result<String, sqlx::Error> {
    let Some(mut lease) = claim_period(pool, "temporal_partitions", period).await? else {
        return Ok("skipped: another instance runs this period".into());
    };
    let summary = temporal_pass(pool, backend, retention_days, &mut lease).await;
    lease.release().await;
    summary
}

/// The steps of one pass under `lease`: a lost lease stops it between steps.
async fn temporal_pass(
    pool: &PgPool,
    backend: TemporalBackend,
    retention_days: Option<i64>,
    lease: &mut Lease,
) -> Result<String, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let claimed = sqlx::query(
        "SELECT name FROM maintenance_jobs WHERE name = 'temporal_partitions'
         FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if claimed.is_none() {
        return Ok("skipped: another instance holds the claim".into());
    }
    // retention DML is cross-tenant service work (the `antares.service`
    // escape in 0001_init.sql)
    crate::store::pg::set_service(&mut tx).await?;
    let mut done: Vec<String> = Vec::new();
    // The 4.22 reaps run in their own transactions AFTER this one commits: a
    // reap that times out or deadlocks must not abort the partition
    // pre-creation that keeps ingest writable.
    if backend == TemporalBackend::Hypertable {
        // ponytail: drop_chunks takes a chunk's audit rows along with it
        // (T-6337); a timescale deployment that audits leaves
        // ANTARES_TEMPORAL_RETENTION_DAYS unset and lets the rule sweeps,
        // which keep them, do the work.
        if let Some(days) = retention_days {
            for sql in [
                "SELECT public.drop_chunks('attr_instances', older_than => make_interval(days => $1::int))",
                "SELECT public.drop_chunks('attr_rows', older_than => make_interval(days => $1::int))",
            ] {
                sqlx::query(sql).bind(days).execute(&mut *tx).await?;
            }
            done.push(format!("timescale drop_chunks older than {days}d"));
        }
    } else {
        // weekly partitions for [now-1w, now+4w); a week from the cut-over on
        // (migration 0020, T-5200) is a week of attr_rows, every other one of
        // attr_instances
        for off in -1i64..4 {
            let row = sqlx::query(
                "SELECT to_char(date_trunc('week', now()) + make_interval(weeks => $1::int), 'IYYY\"w\"IW') AS suffix,
                        (date_trunc('week', now()) + make_interval(weeks => $1::int))::text AS lo,
                        (date_trunc('week', now()) + make_interval(weeks => ($1::int) + 1))::text AS hi,
                        COALESCE(date_trunc('week', now()) + make_interval(weeks => $1::int)
                                   >= (SELECT at FROM history_cutover), false) AS keyed",
            )
            .bind(off)
            .fetch_one(&mut *tx)
            .await?;
            let (suffix, lo, hi): (String, String, String) =
                (row.get("suffix"), row.get("lo"), row.get("hi"));
            let keyed: bool = row.get("keyed");
            if default_is_building(&mut tx, keyed, &suffix).await? {
                done.push(format!(
                    "partition {suffix}: waits for the DEFAULT partition's index"
                ));
                continue;
            }
            // The failure below is EXPECTED (see module docs), and in
            // PostgreSQL a failed statement aborts the whole transaction —
            // every later one then returns 25P02. Tolerating an error means
            // owning a savepoint to roll back to; without it the first
            // already-occupied range poisons the entire maintenance pass.
            let mut sp = tx.begin().await?;
            let mut created = sqlx::query(sqlx::AssertSqlSafe(create_partition_sql(
                keyed, &suffix, &lo, &hi,
            )))
            .execute(&mut *sp)
            .await
            .map(|_| ());
            if created.is_ok() {
                created = sqlx::query(sqlx::AssertSqlSafe(partition_rls_sql(keyed, &suffix)))
                    .execute(&mut *sp)
                    .await
                    .map(|_| ());
            }
            match created {
                Ok(()) => {
                    sp.commit().await?;
                    done.push(format!("partition {suffix}: ok"));
                    continue;
                }
                Err(_) => sp.rollback().await?,
            }
            // Rows for this range already sit in DEFAULT. Adopt them: the move
            // empties the range, so the ATTACH's revalidation of DEFAULT
            // passes. Its ACCESS EXCLUSIVE lock is bounded by the connection's
            // lock_timeout, and a loser simply retries on the next tick.
            let mut sp = tx.begin().await?;
            let mut adopted = Ok(());
            for stmt in adopt_default_rows_sql(keyed, &suffix, &lo, &hi)
                .into_iter()
                .chain([partition_rls_sql(keyed, &suffix)])
            {
                adopted = sqlx::query(sqlx::AssertSqlSafe(stmt))
                    .execute(&mut *sp)
                    .await
                    .map(|_| ());
                if adopted.is_err() {
                    break;
                }
            }
            match adopted {
                Ok(()) => {
                    sp.commit().await?;
                    done.push(format!("partition {suffix}: adopted from default"));
                }
                Err(e) => {
                    // warn, not debug: a PERMANENTLY failing create
                    // (permissions) must be visible at default log level, or it
                    // silently degrades to "everything lands in DEFAULT".
                    sp.rollback().await?;
                    tracing::warn!("weekly history partition {suffix} not created: {e}");
                    done.push(format!("partition {suffix}: left in default"));
                }
            }
        }
    }
    tx.commit().await?;
    // Plain-mode retention runs AFTER the claim transaction commits, for the
    // same reason as the 4.22 reaps: DROP TABLE needs ACCESS EXCLUSIVE on the
    // parent and the DEFAULT purge grows with stored volume, so one lock
    // contention or one statement timeout would otherwise roll back the
    // partition pre-creation that keeps ingest writable.
    // The superseded versions (T-4665) live in one plain table beside the
    // partitions or chunks, and go with the platform retention all the same.
    if !lease.alive().await {
        done.push("stopped: the lease was lost".into());
        return Ok(done.join("; "));
    }
    if let Some(days) = retention_days {
        match versions_retention(pool, days).await {
            Ok(0) => {}
            Ok(n) => done.push(format!(
                "deleted {n} superseded versions older than {days}d"
            )),
            Err(e) => done.push(format!("version retention skipped ({e})")),
        }
    }
    if backend == TemporalBackend::Partitioned {
        // Before any drop: a partition without its audit index is kept.
        match history_audit_index(pool).await {
            Ok(Some(line)) => done.push(line),
            Ok(None) => {}
            Err(e) => done.push(format!("audit index build deferred ({e})")),
        }
        if let Some(days) = retention_days {
            match plain_retention(pool, days).await {
                Ok(lines) => done.extend(lines),
                Err(e) => done.push(format!("retention skipped ({e})")),
            }
        }
        match drop_emptied_partitions(pool).await {
            Ok(lines) => done.extend(lines),
            Err(e) => done.push(format!("emptied partitions kept ({e})")),
        }
        if let Ok(Some(line)) = default_partition_load(pool).await {
            done.push(line);
        }
        match history_one_index(pool).await {
            Ok(Some(line)) => done.push(line),
            Ok(None) => {}
            Err(e) => done.push(format!("history index build deferred ({e})")),
        }
    }
    if !lease.alive().await {
        done.push("stopped: the lease was lost".into());
        return Ok(done.join("; "));
    }
    match mark_bare(pool, BARE_BUDGET).await {
        Ok(BareSweep { marked: 0, .. }) => {}
        Ok(b) => done.push(format!(
            "bare: {} of {} Entities checked changed",
            b.marked, b.checked
        )),
        Err(e) => done.push(format!("bare sweep skipped ({e})")),
    }
    // 4.22 garbage collection, on both backends: reads already refuse expired
    // entities and instances, so these reaps only bound storage — the clause
    // itself sanctions deletion lagging expiresAt.
    match reap_expired_entities(pool).await {
        Ok(0) => {}
        Ok(n) => {
            tracing::info!(reaped = n, "4.22 reap: expired transient entities deleted");
            done.push(format!("reaped {n} expired transient entities (4.22)"));
        }
        Err(e) => done.push(format!("entity reap skipped ({e})")),
    }
    match history_expiry_index(pool, backend).await {
        Ok(Some(line)) => done.push(line),
        Ok(None) => {}
        Err(e) => done.push(format!("expiry index build deferred ({e})")),
    }
    // 4.22 also names Properties/Relationships: an attribute instance whose
    // expiresAt has passed "should be deleted from an NGSI-LD system". Each
    // DELETE contends with concurrent ingest on the same rows, and
    // a deadlock must cost only the reap (observed under ~1.2k msg/s ingest).
    if !lease.alive().await {
        done.push("stopped: the lease was lost".into());
        return Ok(done.join("; "));
    }
    match reap_expired_instances(pool, backend).await {
        Ok(None) => {}
        Ok(Some(reap)) => {
            let n = reap.pruned.saturating_add(reap.deleted);
            if n > 0 {
                tracing::info!(
                    reaped = n,
                    pruned = reap.pruned,
                    rows = reap.deleted,
                    ms = u64::try_from(reap.took.as_millis()).unwrap_or(u64::MAX),
                    "4.22 reap: expired attribute instances pruned"
                );
                done.push(format!("reaped {n} expired attribute instances (4.22)"));
            }
            for (leaf, e) in reap.failed {
                tracing::warn!("4.22 reap of {leaf} left for the next tick: {e}");
                done.push(format!("instance reap skipped on {leaf} ({e})"));
            }
        }
        Err(e) => done.push(format!("instance reap skipped ({e})")),
    }
    // Claim-check rows the drain kept because the bus could not carry their
    // bodies. The consumer resolves them off the message, so the window has to
    // outlive the message: reaping one early costs the notification it was
    // carrying, keeping one costs a row.
    match crate::store::pg::outbox::reap_published(pool, CLAIM_CHECK_HOURS).await {
        Ok(0) => {}
        Ok(n) => done.push(format!("reaped {n} published claim-check rows")),
        Err(e) => done.push(format!("claim-check reap skipped ({e})")),
    }
    if done.is_empty() {
        done.push("nothing to do".into());
    }
    Ok(done.join("; "))
}

/// The superseded versions older than `days` days, on both backends.
async fn versions_retention(pool: &PgPool, days: i64) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    crate::store::pg::set_service(&mut tx).await?;
    let n = sqlx::query(
        "DELETE FROM attr_versions WHERE observed_at < now() - make_interval(days => $1::int)
           AND NOT coalesce(provenance @> '{\"audit\": true}', false)",
    )
    .bind(days)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(n)
}

/// Plain-mode retention, in its own transaction: drop every weekly partition
/// whose whole range is older than the horizon, then purge the DEFAULT
/// partition (which has no upper bound and is therefore never dropped) of the
/// historic-backfill rows that landed there before their week existed.
///
/// Both statements are idempotent, so a run that loses a lock simply repeats
/// on the next tick. Service role: retention is cross-tenant work.
async fn plain_retention(pool: &PgPool, days: i64) -> Result<Vec<String>, sqlx::Error> {
    let mut done: Vec<String> = Vec::new();
    let mut tx = pool.begin().await?;
    crate::store::pg::set_service(&mut tx).await?;
    let parts = sqlx::query(
        // The parent is pinned to the schema the migrations built. Without
        // it a same-named table in another schema contributes its children,
        // and the DROP below would name them by bare relname.
        "SELECT c.relname,
                pg_get_expr(c.relpartbound, c.oid) AS bound
         FROM pg_inherits i
         JOIN pg_class c ON c.oid = i.inhrelid
         JOIN pg_class p ON p.oid = i.inhparent
         WHERE p.relname IN ('attr_instances', 'attr_rows_long')
           AND p.relnamespace = 'public'::regnamespace",
    )
    .fetch_all(&mut *tx)
    .await?;
    for r in parts {
        let name: String = r.get("relname");
        let bound: String = r.get::<Option<String>, _>("bound").unwrap_or_default();
        // bound looks like: FOR VALUES FROM ('<lo>') TO ('<hi>')
        let Some(hi) = bound
            .split("TO ('")
            .nth(1)
            .and_then(|s| s.split('\'').next())
        else {
            continue; // DEFAULT partition — never dropped
        };
        let expired: bool =
            sqlx::query_scalar("SELECT $1::timestamptz < now() - make_interval(days => $2::int)")
                .bind(hi)
                .bind(days)
                .fetch_one(&mut *tx)
                .await?;
        if expired && !audit_free(&mut tx, &name).await? {
            done.push(format!(
                "expired partition {name} kept: it holds audit rows"
            ));
        } else if expired {
            sqlx::query(sqlx::AssertSqlSafe(drop_partition_sql(&name)))
                .execute(&mut *tx)
                .await?;
            done.push(format!("dropped expired partition {name}"));
        }
    }
    let mut purged = 0u64;
    for sql in [
        "DELETE FROM attr_instances_default
         WHERE observed_at < now() - make_interval(days => $1::int)
           AND NOT coalesce(provenance @> '{\"audit\": true}', false)",
        "DELETE FROM attr_rows_default
         WHERE observed_at < now() - make_interval(days => $1::int)
           AND NOT coalesce(provenance @> '{\"audit\": true}', false)",
    ] {
        let n = sqlx::query(sql)
            .bind(days)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        purged = purged.saturating_add(n);
    }
    tx.commit().await?;
    if purged > 0 {
        done.push(format!(
            "purged {purged} expired rows from DEFAULT partition"
        ));
    }
    Ok(done)
}

/// Rows the DEFAULT partition holds, and a warning once it stops being
/// incidental. Every row there is one no weekly partition covers — historic
/// backfill, or an `observedAt` dated beyond the pre-created window — so the
/// count is what makes an unpartitioned pile visible before it shows up as a
/// query that stopped pruning. `reltuples` is the planner's estimate, so this
/// costs a catalog lookup rather than a scan of the pile it is measuring;
/// `-1` means "never analysed", which is reported as nothing.
const DEFAULT_PARTITION_WARN_ROWS: i64 = 100_000;

async fn default_partition_load(pool: &PgPool) -> Result<Option<String>, sqlx::Error> {
    let rows: Option<f32> = sqlx::query_scalar(
        "SELECT sum(greatest(reltuples, 0))::real FROM pg_class
         WHERE relname IN ('attr_instances_default', 'attr_rows_default')
           AND relnamespace = 'public'::regnamespace",
    )
    .fetch_optional(pool)
    .await?;
    let est = rows.unwrap_or(-1.0) as i64;
    if est < DEFAULT_PARTITION_WARN_ROWS {
        return Ok(None);
    }
    tracing::warn!(
        "the DEFAULT history partitions hold ~{est} rows: instances are being written outside the \
         maintained partition window (historic backfill, or an observedAt far in the future); \
         queries over those ranges cannot prune"
    );
    Ok(Some(format!("default partition ~{est} rows")))
}

/// The 4.22 expired-entity DELETE, isolated so a reap that outruns
/// `statement_timeout` costs only itself. Served by the partial index on
/// `expires_at` (0001_init.sql) — without it this is a sequential scan of
/// every entity in the deployment. Service role: the reap is cross-tenant work
/// (RLS would hide other tenants' rows). An Entity its `expiresAt` ends takes
/// its history with it (DM-97, owner 2026-10-18), in the same transaction:
/// its instances under the service role, its temporal row under its own
/// Tenant, as `temporal_entities` keeps the plain tenant policy.
///
/// Bounded: one statement over every expired Entity of the deployment ran up
/// to the 30 s statement_timeout on dev (5.5M reaped, 29.9 s max, T-6214), and
/// a reap that times out reaps nothing. Each batch of [`REAP_BATCH`], earliest
/// expiry first, is its own transaction; a tick runs at most
/// [`REAP_BATCHES`] of them and the rest wait for the next tick.
async fn reap_expired_entities(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut reaped = 0u64;
    for _ in 0..REAP_BATCHES {
        // A batch holds one Tenant, so a short one says nothing of the others.
        let n = reap_expired_entity_batch(pool, REAP_BATCH).await?;
        reaped = reaped.saturating_add(n);
        if n == 0 {
            break;
        }
    }
    Ok(reaped)
}

/// Expired Entities one reap transaction deletes at most.
const REAP_BATCH: i64 = 5000;

/// Reap transactions one maintenance tick runs at most.
const REAP_BATCHES: usize = 50;

/// One batch of `reap_expired_entities`: at most `limit` Entities of the
/// Tenant whose Entity expired first, and their history. How many went.
///
/// The row lock runs under that Tenant's scope: `FOR UPDATE` applies the
/// `tenant_update` policy, which names the Tenant in scope and no service
/// escape (0005), so under the escape alone it matches no row and the reap
/// deleted nothing (T-7059, jc-dev 2026-11-15).
pub async fn reap_expired_entity_batch(pool: &PgPool, limit: i64) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    crate::store::pg::set_service(&mut tx).await?;
    let tenant: Option<String> = sqlx::query_scalar(
        "SELECT tenant_id FROM entities
          WHERE expires_at IS NOT NULL AND expires_at < now()
          ORDER BY expires_at LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(tenant) = tenant else {
        tx.commit().await?;
        return Ok(0);
    };
    sqlx::query(crate::SET_TENANT_SQL)
        .bind(&tenant)
        .execute(&mut *tx)
        .await?;
    let ids: Vec<String> = sqlx::query_scalar(
        // MATERIALIZED: the batch is chosen once; a semi-join may rescan a
        // LIMIT … SKIP LOCKED subquery and delete more than the batch.
        "WITH doomed AS MATERIALIZED (
           SELECT id FROM entities
            WHERE tenant_id = $1 AND expires_at IS NOT NULL AND expires_at < now()
            ORDER BY expires_at LIMIT $2
            FOR UPDATE SKIP LOCKED)
         DELETE FROM entities e USING doomed d
          WHERE e.tenant_id = $1 AND e.id = d.id
         RETURNING e.id",
    )
    .bind(&tenant)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    if !ids.is_empty() {
        for sql in [
            "DELETE FROM attr_instances WHERE tenant_id = $1 AND entity_id = ANY($2::text[]) \
             AND NOT coalesce(provenance @> '{\"audit\": true}', false)",
            "DELETE FROM attr_rows WHERE tenant_id = $1 AND entity_no IN \
             (SELECT i.id FROM iri_ids i WHERE i.tenant_id = $1 AND i.iri = ANY($2::text[])) \
             AND NOT coalesce(provenance @> '{\"audit\": true}', false)",
            "DELETE FROM attr_versions WHERE tenant_id = $1 AND entity_id = ANY($2::text[]) \
             AND NOT coalesce(provenance @> '{\"audit\": true}', false)",
        ] {
            sqlx::query(sql)
                .bind(&tenant)
                .bind(&ids)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("DELETE FROM temporal_entities WHERE tenant_id = $1 AND id = ANY($2::text[])")
            .bind(&tenant)
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(u64::try_from(ids.len()).unwrap_or(u64::MAX))
}

/// The keys [`retain_current`] removes: `$4` at most of Tenant `$1` whose
/// type is the one of `$2` and that are unchanged since `$3`. `types @> $2`
/// is no index condition under RLS (`arraycontains` is not leakproof), so the
/// type reads as the entity query's two leakproof arms (entity.rs `arms`,
/// T-4781): the Entities whose first type it is (`i_entities_first_type_id`)
/// and the multi-type ones whose first type is another
/// (`i_entities_multi_type_id`). The arms are disjoint; each is bounded, and
/// so is their union (T-7275). Public for tests/pg_explain.rs.
pub const RETAIN_CURRENT_ROWS: &str = "(SELECT tenant_id, id FROM entities
           WHERE tenant_id = $1 AND types[1] = ($2::text[])[1]
             AND expires_at IS NULL AND modified_at < $3::timestamptz
           LIMIT $4)
         UNION ALL
         (SELECT tenant_id, id FROM entities
           WHERE tenant_id = $1 AND cardinality(types) > 1
             AND types[1] <> ALL($2::text[]) AND types @> $2::text[]
             AND expires_at IS NULL AND modified_at < $3::timestamptz
           LIMIT $4)
         LIMIT $4";

/// Retention of current state (`/q/retention`, T-3900): at most `batch`
/// Entities of one Tenant and type that carry no `expiresAt` (a writer's 4.22
/// stamp wins) and were last modified before `before`
/// ([`RETAIN_CURRENT_ROWS`]). Service role, like every reap here; the 4.22
/// reap's precedent holds: rows go, no notification is owed for a removal
/// the client declared. `dry_run` counts the same rows and deletes none.
pub async fn retain_current(
    pool: &PgPool,
    tenant: &str,
    type_iri: &str,
    before: &str,
    batch: usize,
    dry_run: bool,
) -> Result<u64, sqlx::Error> {
    retain(
        pool,
        RETAIN_CURRENT_ROWS,
        "DELETE FROM entities WHERE (tenant_id, id) IN",
        (tenant, &[type_iri.to_owned()], None, before, batch),
        dry_run,
    )
    .await
}

/// One bounded retention statement: `rows` selects at most `$4` keys of
/// Tenant `$1` matching the names `$2` (and, given, leaving out the Entities
/// `$5`) before `$3`; `delete` removes them, a dry run counts them.
async fn retain(
    pool: &PgPool,
    rows: &str,
    delete: &str,
    (tenant, names, except_ids, before, batch): (&str, &[String], Option<&[String]>, &str, usize),
    dry_run: bool,
) -> Result<u64, sqlx::Error> {
    let sql = if dry_run {
        format!("SELECT count(*) FROM ({rows}) r")
    } else {
        format!("{delete} ({rows})")
    };
    let mut tx = pool.begin().await?;
    crate::store::pg::set_service(&mut tx).await?;
    // The rule's own Tenant as well: `temporal_entities` keeps the plain
    // tenant policy (no service escape), so without it the history JOIN sees
    // no row under the broker's role and deletes nothing (T-3900).
    sqlx::query(crate::SET_TENANT_SQL)
        .bind(tenant)
        .execute(&mut *tx)
        .await?;
    if !dry_run {
        // One batch past a third of the sweep's budget (15 s, antares-ops
        // retention) is cancelled rather than holding a connection and the
        // database for minutes (T-4084); the sweep logs it, goes on with the
        // other steps and retries it the next minute, so one slow (Tenant,
        // type) cannot spend the whole budget (T-5157).
        sqlx::query("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *tx)
            .await?;
    }
    // Both parts are constants of this module; every value is bound.
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(tenant)
        .bind(names)
        .bind(antares_store::filter::canonical_datetime(before).into_owned())
        .bind(i64::try_from(batch).unwrap_or(i64::MAX));
    if let Some(ids) = except_ids {
        query = query.bind(ids);
    }
    let n = if dry_run {
        let count: i64 = sqlx::Row::get(&query.fetch_one(&mut *tx).await?, 0);
        u64::try_from(count).unwrap_or_default()
    } else {
        query.execute(&mut *tx).await?.rows_affected()
    };
    tx.commit().await?;
    Ok(n)
}

/// At most `$4` instances of the Entities `$2` of Tenant `$1` observed
/// before `$3`, by the (tenant, entity) prefix of the history index.
const ENTITY_ROWS: &str = "SELECT a.tenant_id, a.entity_id, a.attr_id, a.instance_id, a.observed_at
           FROM attr_instances a
           WHERE a.tenant_id = $1 AND a.entity_id = ANY($2::text[])
             AND a.observed_at < $3::timestamptz
             AND NOT coalesce(a.provenance @> '{\"audit\": true}', false)
           LIMIT $4";
/// The same over `attr_rows` (T-5200), its keys mapped through the Tenant's
/// `iri_ids`; both of its children, the short one included (T-5249).
const KEYED_ROWS: &str = "SELECT a.tenant_id, a.entity_no, a.attr_no, a.instance_no, a.observed_at
           FROM attr_rows a
           WHERE a.tenant_id = $1
             AND a.entity_no IN (SELECT i.id FROM iri_ids i WHERE i.tenant_id = $1 AND i.iri = ANY($2::text[]))
             AND a.observed_at < $3::timestamptz
             AND NOT coalesce(a.provenance @> '{\"audit\": true}', false)
           LIMIT $4";
/// The same over the superseded versions (T-4665).
const VERSION_ROWS: &str =
    "SELECT a.tenant_id, a.entity_id, a.attr_id, a.instance_id, a.observed_at
           FROM attr_versions a
           WHERE a.tenant_id = $1 AND a.entity_id = ANY($2::text[])
             AND a.observed_at < $3::timestamptz
             AND NOT coalesce(a.provenance @> '{\"audit\": true}', false)
           LIMIT $4";

/// Retention of history (`/q/retention`, T-3900, DM-97): at most `batch`
/// attribute instances of the Entities `scope` names in one Tenant, observed
/// before `before`. Types live on `temporal_entities`; the instance key is
/// the unique index's, so the IN matches exactly the rows selected in either
/// backend. `observed_at` prunes the partitions to the old weeks.
pub async fn retain_history(
    pool: &PgPool,
    tenant: &str,
    scope: antares_store::HistoryScope<'_>,
    before: &str,
    batch: usize,
    dry_run: bool,
) -> Result<u64, sqlx::Error> {
    use antares_store::HistoryScope;
    const ENTITY: &str = ENTITY_ROWS;
    const TYPE: &str = "SELECT a.tenant_id, a.entity_id, a.attr_id, a.instance_id, a.observed_at
           FROM attr_instances a
           JOIN temporal_entities t ON t.tenant_id = a.tenant_id AND t.id = a.entity_id
           WHERE a.tenant_id = $1 AND t.types @> $2::text[]
             AND a.entity_id <> ALL($5::text[])
             AND a.observed_at < $3::timestamptz
             AND NOT coalesce(a.provenance @> '{\"audit\": true}', false)
           LIMIT $4";
    const REST: &str = "SELECT a.tenant_id, a.entity_id, a.attr_id, a.instance_id, a.observed_at
           FROM attr_instances a
           JOIN temporal_entities t ON t.tenant_id = a.tenant_id AND t.id = a.entity_id
           WHERE a.tenant_id = $1 AND NOT (t.types && $2::text[])
             AND a.entity_id <> ALL($5::text[])
             AND a.observed_at < $3::timestamptz
             AND NOT coalesce(a.provenance @> '{\"audit\": true}', false)
           LIMIT $4";
    // The type and rest selections over `attr_rows` (T-5200), its keys mapped
    // through the Tenant's `iri_ids`; KEYED_ROWS is the Entity one.
    const R_TYPE: &str = "SELECT a.tenant_id, a.entity_no, a.attr_no, a.instance_no, a.observed_at
           FROM attr_rows a
           JOIN iri_ids i ON i.tenant_id = a.tenant_id AND i.id = a.entity_no
           JOIN temporal_entities t ON t.tenant_id = i.tenant_id AND t.id = i.iri
           WHERE a.tenant_id = $1 AND t.types @> $2::text[]
             AND i.iri <> ALL($5::text[])
             AND a.observed_at < $3::timestamptz
             AND NOT coalesce(a.provenance @> '{\"audit\": true}', false)
           LIMIT $4";
    const R_REST: &str = "SELECT a.tenant_id, a.entity_no, a.attr_no, a.instance_no, a.observed_at
           FROM attr_rows a
           JOIN iri_ids i ON i.tenant_id = a.tenant_id AND i.id = a.entity_no
           JOIN temporal_entities t ON t.tenant_id = i.tenant_id AND t.id = i.iri
           WHERE a.tenant_id = $1 AND NOT (t.types && $2::text[])
             AND i.iri <> ALL($5::text[])
             AND a.observed_at < $3::timestamptz
             AND NOT coalesce(a.provenance @> '{\"audit\": true}', false)
           LIMIT $4";
    // A measurement past retention takes its superseded versions along
    // (T-4665): the same three selections over `attr_versions`, written out so
    // every statement on `attr_instances` stays a literal the tenant guard
    // reads (tests/attr_instances_tenant_predicate.rs).
    const V_ENTITY: &str = VERSION_ROWS;
    const V_TYPE: &str = "SELECT a.tenant_id, a.entity_id, a.attr_id, a.instance_id, a.observed_at
           FROM attr_versions a
           JOIN temporal_entities t ON t.tenant_id = a.tenant_id AND t.id = a.entity_id
           WHERE a.tenant_id = $1 AND t.types @> $2::text[]
             AND a.entity_id <> ALL($5::text[])
             AND a.observed_at < $3::timestamptz
             AND NOT coalesce(a.provenance @> '{\"audit\": true}', false)
           LIMIT $4";
    const V_REST: &str = "SELECT a.tenant_id, a.entity_id, a.attr_id, a.instance_id, a.observed_at
           FROM attr_versions a
           JOIN temporal_entities t ON t.tenant_id = a.tenant_id AND t.id = a.entity_id
           WHERE a.tenant_id = $1 AND NOT (t.types && $2::text[])
             AND a.entity_id <> ALL($5::text[])
             AND a.observed_at < $3::timestamptz
             AND NOT coalesce(a.provenance @> '{\"audit\": true}', false)
           LIMIT $4";
    // A sweep's delete of a type walks its Entities (T-5157): see
    // [`walk_history`]. A dry run counts the whole scope in one statement.
    if !dry_run {
        let one;
        let (ids, key, names, except) = match scope {
            HistoryScope::Entity(_) => (None, String::new(), &[][..], &[][..]),
            HistoryScope::Type {
                type_iri,
                except_ids,
            } => {
                one = [type_iri.to_owned()];
                (
                    Some(TYPE_IDS),
                    format!("{tenant}\n{type_iri}"),
                    &one[..],
                    except_ids,
                )
            }
            HistoryScope::Rest {
                except_types,
                except_ids,
            } => (
                Some(REST_IDS),
                format!("{tenant}\n"),
                except_types,
                except_ids,
            ),
        };
        if let Some(ids) = ids {
            let walk = Walk {
                ids,
                key,
                names,
                except,
            };
            return walk_history(pool, tenant, &walk, before, batch).await;
        }
    }
    let one;
    let (rows, keyed, versions, names, except) = match scope {
        HistoryScope::Entity(id) => {
            one = [id.to_owned()];
            (ENTITY, KEYED_ROWS, V_ENTITY, &one[..], None)
        }
        HistoryScope::Type {
            type_iri,
            except_ids,
        } => {
            one = [type_iri.to_owned()];
            (TYPE, R_TYPE, V_TYPE, &one[..], Some(except_ids))
        }
        HistoryScope::Rest {
            except_types,
            except_ids,
        } => (REST, R_REST, V_REST, except_types, Some(except_ids)),
    };
    let current = retain(
        pool,
        rows,
        "DELETE FROM attr_instances
         WHERE (tenant_id, entity_id, attr_id, instance_id, observed_at) IN",
        (tenant, names, except, before, batch),
        dry_run,
    )
    .await?;
    let current_keyed = retain(
        pool,
        keyed,
        "DELETE FROM attr_rows
         WHERE (tenant_id, entity_no, attr_no, instance_no, observed_at) IN",
        (tenant, names, except, before, batch),
        dry_run,
    )
    .await?;
    let superseded = retain(
        pool,
        versions,
        "DELETE FROM attr_versions
         WHERE (tenant_id, entity_id, attr_id, instance_id, observed_at) IN",
        (tenant, names, except, before, batch),
        dry_run,
    )
    .await?;
    Ok(current
        .saturating_add(current_keyed)
        .saturating_add(superseded))
}

/// The Entities of a type (`$2`), or of no type of `$2` for the rest, in
/// Tenant `$1` leaving out `$3`, after `$4` in id order, at most `$5`. The
/// primary key serves the order; `types` is only a filter, as no array
/// operator reaches an index under RLS (migration 0015). An Entity the bare
/// sweep found without history (migration 0030) holds no row to delete and
/// is passed over unprobed (T-6957: on jc-dev 128 678 such Entities of one
/// type held each sweep's step for its whole [`WALK_TIME`], and the old rows
/// behind them waited hours); a write of an instance clears the mark first.
const TYPE_IDS: &str = "SELECT t.id FROM temporal_entities t
       WHERE t.tenant_id = $1 AND t.types @> $2::text[] AND NOT t.bare
         AND t.id <> ALL($3::text[]) AND t.id > $4
       ORDER BY t.id LIMIT $5";
const REST_IDS: &str = "SELECT t.id FROM temporal_entities t
       WHERE t.tenant_id = $1 AND NOT (t.types && $2::text[]) AND NOT t.bare
         AND t.id <> ALL($3::text[]) AND t.id > $4
       ORDER BY t.id LIMIT $5";

/// How long one [`walk_history`] call goes on to the next chunk of Entities
/// when the ones before held nothing to delete.
const WALK_TIME: std::time::Duration = std::time::Duration::from_secs(1);

/// Where the walk of each (Tenant, scope) stopped, by [`Walk::key`]: the
/// next call, the next sweep included, starts after it. Per process; a
/// restart walks from the start again.
static WALKS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
    std::sync::LazyLock::new(Default::default);

/// One history retention scope as a walk over its Entities.
struct Walk<'a> {
    ids: &'static str,
    key: String,
    names: &'a [String],
    except: &'a [String],
}

impl Walk<'_> {
    fn at(&self) -> String {
        let walks = WALKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        walks.get(&self.key).cloned().unwrap_or_default()
    }

    fn stop(&self, after: String) {
        let mut walks = WALKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        walks.insert(self.key.clone(), after);
    }
}

/// History retention of a type (or the rest) as a walk over its Entities in
/// id order (T-5157): each statement deletes at most `batch` instances of a
/// chunk of `batch / 10` (10 to 100) Entities by the per-Entity index, so its
/// cost is bounded by the chunk and never by the plan a JOIN of the type to
/// the whole history gets. On dev that JOIN ran 4-10 s for a step with
/// nothing to delete and timed out every few minutes. A chunk is left when
/// it holds less than what is left of the batch; the walk goes on for up to
/// [`WALK_TIME`] and remembers where it stopped, so a type of 50 000
/// Entities is covered over a few sweeps instead of timing out in one. At
/// the end it starts over. Returns the instances of both history parents
/// and the versions deleted, at most `batch`.
async fn walk_history(
    pool: &PgPool,
    tenant: &str,
    walk: &Walk<'_>,
    before: &str,
    batch: usize,
) -> Result<u64, sqlx::Error> {
    let started = std::time::Instant::now();
    let per = (batch / 10).clamp(10, 100);
    let mut after = walk.at();
    let mut total = 0usize;
    loop {
        let ids = entity_chunk(pool, tenant, walk, &after, per).await?;
        let Some(last) = ids.last().cloned() else {
            walk.stop(String::new());
            return Ok(total as u64);
        };
        for (rows, delete) in [
            (
                ENTITY_ROWS,
                "DELETE FROM attr_instances
             WHERE (tenant_id, entity_id, attr_id, instance_id, observed_at) IN",
            ),
            (
                KEYED_ROWS,
                "DELETE FROM attr_rows
             WHERE (tenant_id, entity_no, attr_no, instance_no, observed_at) IN",
            ),
            (
                VERSION_ROWS,
                "DELETE FROM attr_versions
             WHERE (tenant_id, entity_id, attr_id, instance_id, observed_at) IN",
            ),
        ] {
            let left = batch.saturating_sub(total);
            if left == 0 {
                break;
            }
            let n = retain(
                pool,
                rows,
                delete,
                (tenant, &ids, None, before, left),
                false,
            )
            .await?;
            total = total.saturating_add(usize::try_from(n).unwrap_or(usize::MAX));
        }
        if total >= batch {
            // The chunk may hold more: the next call starts with it again.
            walk.stop(after);
            return Ok(total as u64);
        }
        if ids.len() < per {
            walk.stop(String::new());
            return Ok(total as u64);
        }
        after = last;
        if started.elapsed() >= WALK_TIME {
            walk.stop(after);
            return Ok(total as u64);
        }
    }
}

/// The next chunk of a [`Walk`]'s Entities after `after`, under the rule's
/// Tenant (`temporal_entities` has no service escape) and the same 5 s
/// statement timeout as each delete.
async fn entity_chunk(
    pool: &PgPool,
    tenant: &str,
    walk: &Walk<'_>,
    after: &str,
    per: usize,
) -> Result<Vec<String>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    crate::store::pg::set_service(&mut tx).await?;
    sqlx::query(crate::SET_TENANT_SQL)
        .bind(tenant)
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout = '5s'")
        .execute(&mut *tx)
        .await?;
    let ids = sqlx::query_scalar(walk.ids)
        .bind(tenant)
        .bind(walk.names)
        .bind(walk.except)
        .bind(after)
        .bind(i64::try_from(per).unwrap_or(i64::MAX))
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(ids)
}

/// How long one maintenance pass spends on [`mark_bare`]: the sweep resumes
/// where the last pass stopped, so a large store is covered over passes.
pub const BARE_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);
/// The Entities one [`mark_bare`] transaction locks and checks.
const BARE_BATCH: i64 = 500;
/// Where [`mark_bare`] resumes: the Tenant and the last id it checked.
static BARE_CURSOR: std::sync::Mutex<(String, String)> =
    std::sync::Mutex::new((String::new(), String::new()));

/// What one [`mark_bare`] call did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BareSweep {
    /// Entities checked.
    pub checked: u64,
    /// Entities whose `bare` changed.
    pub marked: u64,
    /// The walk reached the last Tenant's last Entity and starts over next time.
    pub wrapped: bool,
}

/// Sets `temporal_entities.bare` (migration 0030, T-5835) from what the
/// history holds, Tenant by Tenant in id order, for at most `budget`. Each
/// batch locks its rows `FOR UPDATE SKIP LOCKED` first and reads the history
/// in the next statement, whose snapshot is younger than the lock: a write
/// that committed rows before the lock is seen, and one still open holds the
/// row `FOR SHARE` (`insert_rows`), so it is skipped or, once the sweep
/// commits, clears the mark itself. An Entity whose rows retention dropped
/// becomes bare on the next pass, once its superseded versions went too: a
/// version is history the walk must still reach (S5-22), so an Entity
/// holding one stays in the walk.
pub async fn mark_bare(
    pool: &PgPool,
    budget: std::time::Duration,
) -> Result<BareSweep, sqlx::Error> {
    let started = std::time::Instant::now();
    let (from, mut after) = BARE_CURSOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let tenants: Vec<String> = sqlx::query_scalar(
        "SELECT tenant_id FROM tenants WHERE tenant_id >= $1 ORDER BY tenant_id",
    )
    .bind(&from)
    .fetch_all(pool)
    .await?;
    let mut out = BareSweep::default();
    for tenant in tenants {
        if tenant != from {
            after.clear();
        }
        loop {
            if started.elapsed() >= budget {
                *BARE_CURSOR
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = (tenant, after);
                return Ok(out);
            }
            let mut tx = pool.begin().await?;
            sqlx::query(crate::SET_TENANT_SQL)
                .bind(&tenant)
                .execute(&mut *tx)
                .await?;
            let ids: Vec<String> = sqlx::query_scalar(
                "SELECT id FROM temporal_entities WHERE tenant_id = $1 AND id > $2
                 ORDER BY id LIMIT $3 FOR UPDATE SKIP LOCKED",
            )
            .bind(&tenant)
            .bind(&after)
            .bind(BARE_BATCH)
            .fetch_all(&mut *tx)
            .await?;
            let Some(last) = ids.last().cloned() else {
                tx.commit().await?;
                break;
            };
            let marked = sqlx::query(
                "UPDATE temporal_entities m SET bare = c.bare
                 FROM (SELECT e.id, NOT (
                         EXISTS (SELECT 1 FROM attr_instances ai
                                 WHERE ai.tenant_id = e.tenant_id AND ai.entity_id = e.id)
                         OR EXISTS (SELECT 1 FROM attr_rows r
                                    WHERE r.tenant_id = e.tenant_id
                                      AND r.entity_no = iri_no(e.tenant_id, e.id))
                         OR EXISTS (SELECT 1 FROM attr_versions v
                                    WHERE v.tenant_id = e.tenant_id AND v.entity_id = e.id)) AS bare
                       FROM temporal_entities e
                       WHERE e.tenant_id = $1 AND e.id = ANY($2)) c
                 WHERE m.tenant_id = $1 AND m.id = c.id AND m.bare <> c.bare",
            )
            .bind(&tenant)
            .bind(&ids)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            tx.commit().await?;
            out.checked = out
                .checked
                .saturating_add(u64::try_from(ids.len()).unwrap_or(u64::MAX));
            out.marked = out.marked.saturating_add(marked);
            after = last;
        }
    }
    *BARE_CURSOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Default::default();
    out.wrapped = true;
    Ok(out)
}

/// The Entities of every Tenant that carry their own history retention
/// (DM-97), served by the partial index of migration 0045 on the
/// `history_retention` flag a trigger sets from the document, so the sweep
/// reads them without scanning `entities`.
pub async fn history_overrides(
    pool: &PgPool,
    limit: usize,
) -> Result<Vec<(String, String, serde_json::Value)>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    crate::store::pg::set_service(&mut tx).await?;
    let rows = sqlx::query(
        "SELECT tenant_id, id, entity -> 'https://uri.etsi.org/ngsi-ld/default-context/historyRetention' AS own
         FROM entities
         WHERE history_retention
         LIMIT $1",
    )
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get("tenant_id"), r.get("id"), r.get("own")))
        .collect())
}

/// Drop every weekly partition below the pre-created window that holds no
/// row, so the disk the retention sweeps freed goes back to the node
/// (DM-97): rows deleted from a partition that keeps one free nothing, as new
/// writes land in the current week. See [`drop_partitions`].
async fn drop_emptied_partitions(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    drop_partitions(pool, &[], &[]).await
}

/// Retention by partition (T-4906): drop every weekly partition whose rows
/// all belong to Tenants of `floors` (Tenant, UTC DateTime) and whose upper
/// bound is at or below that Tenant's floor, the oldest cut any of its rules
/// keeps. The rows such a partition holds are all ones the row sweep would
/// delete, so dropping the table replaces millions of row DELETEs, their WAL
/// and the vacuum after them (dev 2026-10-19: P28D deleted ~2M rows a night
/// and starved the broker). A partition holding one row of a Tenant without a
/// floor, or with a floor below its bound, stays for the row sweep.
pub async fn drop_retained_partitions(
    pool: &PgPool,
    floors: &[(String, String)],
) -> Result<Vec<String>, sqlx::Error> {
    if floors.is_empty() {
        return Ok(Vec::new());
    }
    let (tenants, cuts): (Vec<String>, Vec<String>) = floors
        .iter()
        .map(|(t, c)| {
            (
                t.clone(),
                antares_store::filter::canonical_datetime(c).into_owned(),
            )
        })
        .unzip();
    drop_partitions(pool, &tenants, &cuts).await
}

/// Whether partition `{p}` may go: every Tenant with a row in it has a floor
/// in (`$1` Tenants, `$2` cuts) at or above its upper bound `$3`. The
/// Tenants present are read by a loose scan of the partition's
/// `(tenant_id, ...)` index, one descent per Tenant, never the rows.
const DROPPABLE: &str = "WITH RECURSIVE t(id) AS (
           (SELECT tenant_id FROM {p} ORDER BY tenant_id LIMIT 1)
           UNION ALL
           SELECT (SELECT tenant_id FROM {p} WHERE tenant_id > t.id ORDER BY tenant_id LIMIT 1)
           FROM t WHERE t.id IS NOT NULL)
         SELECT NOT EXISTS (
           SELECT 1 FROM t
           LEFT JOIN unnest($1::text[], $2::timestamptz[]) AS f(tenant, cut) ON f.tenant = t.id
           WHERE t.id IS NOT NULL AND (f.cut IS NULL OR f.cut < $3::timestamptz))";

/// Drop the past weekly partitions [`DROPPABLE`] admits under (`tenants`,
/// `cuts`); with none, the empty ones below the pre-created window. One
/// transaction locks the parent, and with it every partition, `ACCESS
/// EXCLUSIVE` within a 2 s `lock_timeout` and checks each candidate again
/// under that lock, each check held to a 2 s `statement_timeout`, so a late
/// write into the week either committed before (and is counted) or waits and
/// lands in DEFAULT once the week is gone, and no history read or write
/// waits longer than both bounds; a lock not had in time leaves it all for
/// the next tick. Nothing to drop takes no lock.
async fn drop_partitions(
    pool: &PgPool,
    tenants: &[String],
    cuts: &[String],
) -> Result<Vec<String>, sqlx::Error> {
    // A candidate ends at or below the latest floor, or below the window.
    const OLD_PARTITIONS: &str = r"SELECT c.relname, b.hi::text AS hi
         FROM pg_inherits i
         JOIN pg_class c ON c.oid = i.inhrelid
         JOIN pg_class p ON p.oid = i.inhparent
         CROSS JOIN LATERAL (SELECT substring(pg_get_expr(c.relpartbound, c.oid)
                                    FROM 'TO \(''([^'']+)''\)')::timestamptz AS hi) b
         WHERE p.relname IN ('attr_instances', 'attr_rows_long')
           AND p.relnamespace = 'public'::regnamespace
           AND pg_get_expr(c.relpartbound, c.oid) LIKE 'FOR VALUES FROM%'
           AND b.hi <= GREATEST((SELECT max(x) FROM unnest($1::timestamptz[]) x),
                                date_trunc('week', now()) - interval '1 week')
         ORDER BY b.hi";
    let mut done = Vec::new();
    // Unlocked first, each check in a transaction of its own: one that held
    // a partition's lock while waiting on the next deadlocks with another
    // pod's LOCK below, which takes them all in turn.
    let mut tx = pool.begin().await?;
    let found: Vec<(String, String)> = sqlx::query_as(OLD_PARTITIONS)
        .bind(cuts)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut any = false;
    for (name, hi) in &found {
        let mut tx = pool.begin().await?;
        crate::store::pg::set_service(&mut tx).await?;
        let ok = droppable(&mut tx, name, hi, tenants, cuts).await?;
        tx.commit().await?;
        if ok {
            any = true;
            break;
        }
    }
    if !any {
        return Ok(done);
    }
    let mut tx = pool.begin().await?;
    // The maintenance claim: one pod drops at a time, and a writer that holds
    // it (a pass creating a week) never sees its new week dropped while empty.
    let claimed = sqlx::query(
        "SELECT name FROM maintenance_jobs WHERE name = 'temporal_partitions'
         FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if claimed.is_none() {
        return Ok(done);
    }
    crate::store::pg::set_service(&mut tx).await?;
    sqlx::query("SET LOCAL lock_timeout = '2s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL statement_timeout = '2s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("LOCK TABLE public.attr_instances, public.attr_rows IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await?;
    // Read again under the lock: another pod may have dropped one meanwhile.
    let found: Vec<(String, String)> = sqlx::query_as(OLD_PARTITIONS)
        .bind(cuts)
        .fetch_all(&mut *tx)
        .await?;
    for (name, hi) in found {
        if droppable(&mut tx, &name, &hi, tenants, cuts).await? {
            sqlx::query(sqlx::AssertSqlSafe(drop_partition_sql(&name)))
                .execute(&mut *tx)
                .await?;
            done.push(format!("dropped partition {name}"));
        }
    }
    tx.commit().await?;
    Ok(done)
}

/// Whether [`DROPPABLE`] admits the partition `name` with upper bound `hi`
/// and it holds no audit row ([`audit_free`]). The partition is read
/// directly, under its own copy of the parent's policies
/// ([`partition_rls_sql`]): the caller's transaction runs as the service
/// (`set_service`), or the read would see no Tenant's rows and call a full
/// partition empty.
async fn droppable(
    tx: &mut sqlx::PgConnection,
    name: &str,
    hi: &str,
    tenants: &[String],
    cuts: &[String],
) -> Result<bool, sqlx::Error> {
    if !audit_free(&mut *tx, name).await? {
        return Ok(false);
    }
    let quoted = format!("public.\"{}\"", name.replace('"', "\"\""));
    sqlx::query_scalar(sqlx::AssertSqlSafe(DROPPABLE.replace("{p}", &quoted)))
        .bind(tenants)
        .bind(cuts)
        .bind(hi)
        .fetch_one(tx)
        .await
}

/// Drop one partition, named the way the catalog row spells it. The name
/// comes back from `pg_class.relname` as the identifier itself, not as SQL:
/// unquoted it would be folded to lower case and would need to be a bare
/// identifier to parse at all, so a partition whose name is neither — one
/// created by hand, or by a migration that quoted it — is a partition
/// retention fails on, every tick, for as long as it exists. Schema-
/// qualified for the same reason the catalog query is: the row names a table
/// in `public`, and `search_path` is not what should decide which one.
fn drop_partition_sql(name: &str) -> String {
    format!("DROP TABLE public.\"{}\"", name.replace('"', "\"\""))
}

/// Row-level security on the weekly partition `create_partition_sql` and
/// `adopt_default_rows_sql` make for `suffix`: its parent's policies,
/// ENABLEd and FORCEd (`antares_partition_rls`, migration 0041, which did the
/// same for the partitions made before it). PostgreSQL applies a parent's
/// policies only to statements that name the parent, so a statement naming
/// the partition itself would otherwise read and write every Tenant's rows.
/// A partition that has them already is left alone.
fn partition_rls_sql(keyed: bool, suffix: &str) -> String {
    if keyed {
        format!("SELECT public.antares_partition_rls('public.attr_rows_{suffix}'::regclass)")
    } else {
        format!("SELECT public.antares_partition_rls('public.attr_instances_{suffix}'::regclass)")
    }
}

/// One weekly partition, created directly under the parent: `attr_rows_long`,
/// the logged child of `attr_rows` (0023, T-5249), when `keyed`, else
/// `attr_instances`. Fails while the parent's DEFAULT
/// partition still holds a row in `[lo, hi)`.
fn create_partition_sql(keyed: bool, suffix: &str, lo: &str, hi: &str) -> String {
    if keyed {
        format!(
            "CREATE TABLE IF NOT EXISTS attr_rows_{suffix} PARTITION OF attr_rows_long \
             FOR VALUES FROM ('{lo}') TO ('{hi}')"
        )
    } else {
        format!(
            "CREATE TABLE IF NOT EXISTS attr_instances_{suffix} PARTITION OF attr_instances \
             FOR VALUES FROM ('{lo}') TO ('{hi}')"
        )
    }
}

/// Recovery for a range DEFAULT already holds rows for, in execution order:
/// build the week's table STANDALONE (a `PARTITION OF` would fail again), move
/// exactly `[lo, hi)` out of DEFAULT into it, then ATTACH. The move is what
/// makes the ATTACH legal, and its bounds are what keep every other row in
/// DEFAULT. Run as one unit — a partial application would strand rows in an
/// unattached table.
fn adopt_default_rows_sql(keyed: bool, suffix: &str, lo: &str, hi: &str) -> [String; 3] {
    if keyed {
        return [
            format!(
                "CREATE TABLE IF NOT EXISTS attr_rows_{suffix} \
                 (LIKE attr_rows_long INCLUDING DEFAULTS INCLUDING CONSTRAINTS)"
            ),
            format!(
                "WITH moved AS (DELETE FROM attr_rows_default \
                   WHERE observed_at >= '{lo}' AND observed_at < '{hi}' RETURNING *) \
                 INSERT INTO attr_rows_{suffix} SELECT * FROM moved"
            ),
            format!(
                "ALTER TABLE attr_rows_long ATTACH PARTITION attr_rows_{suffix} \
                 FOR VALUES FROM ('{lo}') TO ('{hi}')"
            ),
        ];
    }
    [
        format!(
            "CREATE TABLE IF NOT EXISTS attr_instances_{suffix} \
             (LIKE attr_instances INCLUDING DEFAULTS INCLUDING CONSTRAINTS)"
        ),
        format!(
            "WITH moved AS (DELETE FROM attr_instances_default \
               WHERE observed_at >= '{lo}' AND observed_at < '{hi}' RETURNING *) \
             INSERT INTO attr_instances_{suffix} SELECT * FROM moved"
        ),
        format!(
            "ALTER TABLE attr_instances ATTACH PARTITION attr_instances_{suffix} \
             FOR VALUES FROM ('{lo}') TO ('{hi}')"
        ),
    ]
}

/// Entities whose stored current state one tick rewrites at most; the rest
/// wait for the next tick, earliest expiry first.
const PRUNE_BATCH: i64 = 1000;

/// The rows of a history table an `{leaf}_expiry` index holds. A statement
/// that reads by the index spells this predicate the same way, or the
/// planner cannot prove the partial index fits. Audit rows are no part of
/// it: they would hold the index's front for as long as they live.
macro_rules! expiring {
    () => {
        "data ? 'expiresAt' AND NOT coalesce(provenance @> '{\"audit\": true}', false)"
    };
}

/// History rows one reap transaction deletes at most.
const INSTANCE_REAP_BATCH: i64 = 1000;
/// How long one tick deletes expired history rows for; the leaf it stopped
/// at opens the next one.
const INSTANCE_REAP_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);
/// The leaf the last batch ran on.
static REAP_CURSOR: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// What one [`reap_expired_instances`] pass did.
#[derive(Debug, Default)]
pub struct InstanceReap {
    /// Current documents rewritten without their expired instances.
    pub pruned: u64,
    /// History rows deleted.
    pub deleted: u64,
    /// How long the pass took.
    pub took: std::time::Duration,
    /// The leaves whose batch failed, each with why; the next tick retries.
    pub failed: Vec<(String, sqlx::Error)>,
}

/// The 4.22 expired-instance reap (T-7517): the current state's expired
/// instances first, committed on their own, then the history leaf by leaf
/// through its `{leaf}_expiry` index, `INSTANCE_REAP_BATCH` rows a
/// transaction, round after round until no leaf has more or
/// `INSTANCE_REAP_BUDGET` is spent. A leaf that times out or deadlocks with
/// ingest costs that batch alone. One reaper per database: `None` while
/// another process holds the advisory lock. A leaf without its index yet is
/// not read ([`history_expiry_index`] builds it); reads hide what expired
/// either way. Service role: the reap is cross-tenant work (RLS would hide
/// other tenants' rows).
pub async fn reap_expired_instances(
    pool: &PgPool,
    backend: TemporalBackend,
) -> Result<Option<InstanceReap>, sqlx::Error> {
    // A session lock over many transactions: a connection of its own, so the
    // lock ends with it also when this future is dropped half way.
    let mut conn = pool.acquire().await?.detach();
    let held: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext('antares.history_reap'))")
            .fetch_one(&mut conn)
            .await?;
    let reap = if held {
        reap_pass(&mut conn, backend).await.map(Some)
    } else {
        Ok(None)
    };
    end_session(conn).await?;
    reap
}

/// End the detached connection of a step under a session advisory lock:
/// release the lock, then close. The close alone does not wait for the
/// backend's exit, so the next call could read the lock as held and skip.
/// When the release fails, or the step is dropped before it, the socket's
/// close ends the session and the lock with it.
async fn end_session(mut conn: sqlx::PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_unlock_all()")
        .execute(&mut conn)
        .await?;
    sqlx::Connection::close(conn).await
}

async fn reap_pass(
    conn: &mut sqlx::PgConnection,
    backend: TemporalBackend,
) -> Result<InstanceReap, sqlx::Error> {
    let started = std::time::Instant::now();
    let mut reap = InstanceReap::default();
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    crate::store::pg::set_service(&mut tx).await?;
    reap.pruned = super::entity::prune_expired_current(&mut tx, PRUNE_BATCH).await?;
    tx.commit().await?;

    let mut due: Vec<Leaf> = expiry_leaves(&mut *conn, backend).await?;
    due.retain(|leaf| leaf.indexed == Some(true));
    due.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    let mut cursor = REAP_CURSOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let after = due.partition_point(|leaf| leaf.name <= cursor);
    due.rotate_left(after);
    'rounds: while !due.is_empty() {
        let mut more = Vec::new();
        for leaf in due {
            if started.elapsed() >= INSTANCE_REAP_BUDGET {
                break 'rounds;
            }
            cursor.clone_from(&leaf.name);
            match reap_batch(&mut *conn, &leaf).await {
                Ok(n) => {
                    reap.deleted = reap.deleted.saturating_add(n);
                    if i64::try_from(n).unwrap_or(i64::MAX) >= INSTANCE_REAP_BATCH {
                        more.push(leaf);
                    }
                }
                Err(e) => reap.failed.push((leaf.name, e)),
            }
        }
        due = more;
    }
    *REAP_CURSOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = cursor;
    reap.took = started.elapsed();
    Ok(reap)
}

/// Whether the earliest expiry of leaf `{t}` has passed: one index entry.
pub const REAP_DUE: &str = concat!(
    "SELECT coalesce((SELECT public.history_expires_at(data) < now() FROM ONLY {t}
       WHERE ",
    expiring!(),
    "
       ORDER BY public.history_expires_at(data) LIMIT 1), false)"
);

/// At most `$1` expired rows of leaf `{t}`. Under forced RLS neither `?` nor
/// the key's function is leakproof, so no range condition reaches the
/// index; its order does, and `ctid = ANY` (leakproof) fetches what that
/// picked. The expiry and audit tests repeat on the row deleted, which a
/// writer may have replaced since it was picked. Public for tests/pg_reap.rs.
pub const REAP_ROWS: &str = concat!(
    "DELETE FROM ONLY {t} WHERE ctid = ANY(ARRAY(
       SELECT picked.pos FROM (
         SELECT ctid AS pos, public.history_expires_at(data) AS expires FROM ONLY {t}
          WHERE ",
    expiring!(),
    "
          ORDER BY public.history_expires_at(data) LIMIT $1) picked
        WHERE picked.expires < now()))
       AND NOT coalesce(provenance @> '{\"audit\": true}', false)
       AND public.history_expires_at(data) < now()"
);

/// The plan both reap statements need: the expiry index read in its order.
/// The planner guesses `?` at a share of the table and would sort all of the
/// index's rows for the LIMIT.
pub const REAP_PLAN: &str = "SELECT set_config('enable_seqscan', 'off', true),
            set_config('enable_bitmapscan', 'off', true),
            set_config('enable_sort', 'off', true)";

/// One transaction of the history reap on one leaf. The rows deleted.
async fn reap_batch(conn: &mut sqlx::PgConnection, leaf: &Leaf) -> Result<u64, sqlx::Error> {
    let table = leaf.table();
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    crate::store::pg::set_service(&mut tx).await?;
    sqlx::query(REAP_PLAN).execute(&mut *tx).await?;
    let due: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(REAP_DUE.replace("{t}", &table)))
        .persistent(false)
        .fetch_one(&mut *tx)
        .await?;
    let n = if due {
        sqlx::query(sqlx::AssertSqlSafe(REAP_ROWS.replace("{t}", &table)))
            .persistent(false)
            .bind(INSTANCE_REAP_BATCH)
            .execute(&mut *tx)
            .await?
            .rows_affected()
    } else {
        0
    };
    tx.commit().await?;
    Ok(n)
}

/// A table that holds history rows itself.
#[derive(Debug, sqlx::FromRow)]
struct Leaf {
    schema: String,
    name: String,
    /// Whether its `{name}_expiry` index is valid; `None` without one.
    indexed: Option<bool>,
    /// A DEFAULT partition.
    fallback: bool,
}

impl Leaf {
    /// The leaf, quoted and qualified as the catalog spells it.
    fn table(&self) -> String {
        format!("{}.{}", quoted(&self.schema), quoted(&self.name))
    }

    /// Its expiry index, quoted bare and qualified.
    fn expiry_index(&self) -> (String, String) {
        let index = quoted(&format!("{}_expiry", self.name));
        let qualified = format!("{}.{index}", quoted(&self.schema));
        (index, qualified)
    }
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Every table under the two history parents that holds rows (partitions of
/// any depth, DEFAULT and timescale chunks included) and `attr_versions`,
/// smallest first by `relpages`, a catalog read that takes no lock.
const EXPIRY_LEAVES: &str = "WITH RECURSIVE below(oid) AS (
       SELECT inhrelid FROM pg_inherits
        WHERE inhparent IN ('public.attr_instances'::regclass, 'public.attr_rows'::regclass)
       UNION ALL
       SELECT i.inhrelid FROM pg_inherits i JOIN below b ON i.inhparent = b.oid)
     SELECT n.nspname AS schema, c.relname AS name, x.indisvalid AS indexed,
            coalesce(pg_get_expr(c.relpartbound, c.oid) = 'DEFAULT', false) AS fallback
       FROM (SELECT oid FROM below
             UNION ALL SELECT 'public.attr_versions'::regclass::oid) l
       JOIN pg_class c ON c.oid = l.oid
       JOIN pg_namespace n ON n.oid = c.relnamespace
       LEFT JOIN pg_class ic ON ic.relname = c.relname || '_expiry'
                            AND ic.relnamespace = c.relnamespace
       LEFT JOIN pg_index x ON x.indexrelid = ic.oid
      WHERE c.relkind = 'r'";

/// The compressed chunks, which the reap leaves out.
///
/// ponytail: an instance that expires after its chunk was compressed stays
/// stored (reads hide it) until retention drops the chunk: timescale
/// decompresses every row of a chunk for a DELETE by ctid and refuses past
/// 100 000. Reap them when timescale can index a compressed expression.
const NOT_COMPRESSED: &str = " AND NOT EXISTS (
         SELECT 1 FROM timescaledb_information.chunks k
          WHERE k.chunk_schema = n.nspname AND k.chunk_name = c.relname AND k.is_compressed)";

async fn expiry_leaves(
    conn: &mut sqlx::PgConnection,
    backend: TemporalBackend,
) -> Result<Vec<Leaf>, sqlx::Error> {
    let uncompressed = match backend {
        TemporalBackend::Hypertable => NOT_COMPRESSED,
        TemporalBackend::Partitioned => "",
    };
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{EXPIRY_LEAVES}{uncompressed} ORDER BY c.relpages"
    )))
    .fetch_all(conn)
    .await
}

/// Held shared by a pass that creates a week and alone by an index build on
/// a DEFAULT partition: `CREATE TABLE .. PARTITION OF` holds the parent
/// against every write while it waits for that build's lock on DEFAULT.
const DEFAULT_BUILD_LOCK: &str = "antares.history_default_index";

/// Whether the week `suffix` is still to be created and a DEFAULT index
/// build is in the way; otherwise the build waits until this pass commits.
async fn default_is_building(
    conn: &mut sqlx::PgConnection,
    keyed: bool,
    suffix: &str,
) -> Result<bool, sqlx::Error> {
    let parent = if keyed {
        "public.attr_rows"
    } else {
        "public.attr_instances"
    };
    sqlx::query_scalar(
        "SELECT CASE WHEN to_regclass($1) IS NULL
                THEN NOT pg_try_advisory_xact_lock_shared(hashtext($2))
                ELSE false END",
    )
    .bind(format!("{parent}_{suffix}"))
    .bind(DEFAULT_BUILD_LOCK)
    .fetch_one(conn)
    .await
}

/// How long one maintenance pass builds expiry indexes for.
const EXPIRY_INDEX_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);

/// Give every history leaf its `{leaf}_expiry` index (T-7517) as
/// [`history_audit_index`] gives the audit one: `CREATE INDEX CONCURRENTLY`,
/// smallest first, for up to `EXPIRY_INDEX_BUDGET`, one pod at a time. The
/// build indexes the rows already stored, so no backfill rewrites history. A
/// leaf created since gets its own on the next pass. One leaf's failure
/// leaves the others their turn; a second ends the pass, and a build left
/// invalid is retried after the leaves never tried. `None` when nothing was
/// missing or another pod holds the step.
pub async fn history_expiry_index(
    pool: &PgPool,
    backend: TemporalBackend,
) -> Result<Option<String>, sqlx::Error> {
    let mut missing = expiry_leaves(&mut *pool.acquire().await?, backend).await?;
    missing.retain(|leaf| leaf.indexed != Some(true));
    if missing.is_empty() {
        return Ok(None);
    }
    // As in history_one_index: a concurrent build runs outside a transaction
    // on a connection of its own, whose end releases the advisory locks.
    let mut conn = pool.acquire().await?.detach();
    let held: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext('antares.history_expiry_index'))")
            .fetch_one(&mut conn)
            .await?;
    let line = if held {
        expiry_index_step(&mut conn, backend).await.map(Some)
    } else {
        Ok(None)
    };
    end_session(conn).await?;
    line
}

async fn expiry_index_step(
    conn: &mut sqlx::PgConnection,
    backend: TemporalBackend,
) -> Result<String, sqlx::Error> {
    use sqlx::Executor;
    // Read again under the lock: another pod may have built some meanwhile.
    let mut missing = expiry_leaves(&mut *conn, backend).await?;
    missing.retain(|leaf| leaf.indexed != Some(true));
    missing.sort_by_key(|leaf| leaf.indexed.is_some());
    conn.execute(sqlx::AssertSqlSafe(format!(
        "SET statement_timeout = 0; SET lock_timeout = '{INDEX_BUILD_WAIT}'"
    )))
    .await?;
    let started = std::time::Instant::now();
    let (mut built, mut first_error) = (0usize, None);
    for leaf in &missing {
        if started.elapsed() >= EXPIRY_INDEX_BUDGET {
            break;
        }
        match expiry_index_build(&mut *conn, leaf).await {
            Ok(true) => built = built.saturating_add(1),
            Ok(false) => {}
            Err(e) => {
                // an old snapshot fails every build alike, each after INDEX_BUILD_WAIT
                if first_error.is_some() {
                    break;
                }
                first_error = Some((leaf.name.as_str(), e));
            }
        }
    }
    let mut line = format!("expiry index built on {built} of {} leaves", missing.len());
    if let Some((leaf, e)) = first_error {
        tracing::warn!("expiry index on {leaf} not built: {e}");
        line.push_str(&format!("; {leaf} deferred ({e})"));
    }
    Ok(line)
}

/// Build one leaf's index. `false`: a DEFAULT partition that waits while a
/// pass creates a week.
async fn expiry_index_build(
    conn: &mut sqlx::PgConnection,
    leaf: &Leaf,
) -> Result<bool, sqlx::Error> {
    use sqlx::Executor;
    if leaf.fallback {
        let free: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext($1))")
            .bind(DEFAULT_BUILD_LOCK)
            .fetch_one(&mut *conn)
            .await?;
        if !free {
            return Ok(false);
        }
    }
    let (index, qualified) = leaf.expiry_index();
    let mut result = Ok(());
    if leaf.indexed == Some(false) {
        result = conn
            .execute(sqlx::AssertSqlSafe(format!(
                "DROP INDEX CONCURRENTLY IF EXISTS {qualified}"
            )))
            .await
            .map(|_| ());
    }
    if result.is_ok() {
        result = conn
            .execute(sqlx::AssertSqlSafe(format!(
                "CREATE INDEX CONCURRENTLY {index} ON {} (public.history_expires_at(data)) WHERE {}",
                leaf.table(),
                expiring!()
            )))
            .await
            .map(|_| ());
    }
    if leaf.fallback {
        sqlx::query("SELECT pg_advisory_unlock(hashtext($1))")
            .bind(DEFAULT_BUILD_LOCK)
            .execute(&mut *conn)
            .await?;
    }
    result.map(|()| true)
}

/// The columns of the one history btree (migration 0017, T-5057).
const ONE_INDEX_COLUMNS: &str = "(tenant_id, entity_id, attr_id, observed_at, instance_id)";

/// The standalone index [`history_one_index`] builds on partition `part`,
/// quoted bare (CREATE INDEX puts it in the table's schema) and qualified,
/// and the partition qualified. A weekly suffix keeps the name under the
/// 63-byte identifier limit.
fn one_index_names(part: &str) -> (String, String, String) {
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let index = q(&format!("{part}_u_time"));
    (
        index.clone(),
        format!("public.{index}"),
        format!("public.{}", q(part)),
    )
}

/// The partitions of `attr_instances` without a valid `{partition}_u_time`
/// index, with whether an invalid one (a build that failed) is in the way.
const PARTITIONS_WITHOUT_ONE_INDEX: &str = "SELECT c.relname, x.indexrelid IS NOT NULL AS stale
     FROM pg_inherits i
     JOIN pg_class c ON c.oid = i.inhrelid
     LEFT JOIN pg_class ic ON ic.relname = c.relname || '_u_time'
                          AND ic.relnamespace = 'public'::regnamespace
     LEFT JOIN pg_index x ON x.indexrelid = ic.oid
     WHERE i.inhparent = 'public.attr_instances'::regclass
       AND x.indisvalid IS NOT TRUE
     ORDER BY pg_relation_size(c.oid)";

/// Move a plain-mode history onto one btree (migration 0017, T-5057), one
/// step a pass: while `i_attr_instances_lookup` exists, build
/// `{partition}_u_time` on the smallest partition lacking it with `CREATE
/// INDEX CONCURRENTLY` (writes go on), and once every partition has its own,
/// make them the partitions of `u_attr_instances_time` and drop the two old
/// indexes in one transaction of catalog changes held to a 2 s
/// `lock_timeout`. Standalone indexes first, never an invalid parent filled
/// in place: a partition retention drops before its turn would leave such a
/// parent invalid for good, as PostgreSQL validates it only on an attach.
/// `None` once done, or while another pod holds the step.
///
/// ponytail: the build takes the disk of one more index per partition until
/// the swap frees two; a timescale hypertable keeps both (no concurrent
/// unique build there).
pub async fn history_one_index(pool: &PgPool) -> Result<Option<String>, sqlx::Error> {
    let pending: bool =
        sqlx::query_scalar("SELECT to_regclass('public.i_attr_instances_lookup') IS NOT NULL")
            .fetch_one(pool)
            .await?;
    if !pending {
        return Ok(None);
    }
    // CREATE INDEX CONCURRENTLY runs outside a transaction and for as long as
    // the partition takes, so it gets a connection of its own the pool never
    // sees again (as the migrations do in `connect_with`).
    let mut conn = pool.acquire().await?.detach();
    let held: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext('antares.history_one_index'))")
            .fetch_one(&mut conn)
            .await?;
    if !held {
        return Ok(None);
    }
    let line = one_index_step(&mut conn).await;
    end_session(conn).await?;
    line.map(Some)
}

async fn one_index_step(conn: &mut sqlx::PgConnection) -> Result<String, sqlx::Error> {
    use sqlx::Executor;
    let missing: Vec<(String, bool)> = sqlx::query_as(PARTITIONS_WITHOUT_ONE_INDEX)
        .fetch_all(&mut *conn)
        .await?;
    if let Some((part, stale)) = missing.first() {
        let (bare, index, table) = one_index_names(part);
        // A concurrent build waits for older transactions to end: those waits
        // count against lock_timeout, and one that runs out leaves an invalid
        // index the next pass drops and builds again.
        conn.execute("SET statement_timeout = 0; SET lock_timeout = '60s'")
            .await?;
        if *stale {
            conn.execute(sqlx::AssertSqlSafe(format!(
                "DROP INDEX CONCURRENTLY IF EXISTS {index}"
            )))
            .await?;
        }
        conn.execute(sqlx::AssertSqlSafe(format!(
            "CREATE UNIQUE INDEX CONCURRENTLY {bare} ON {table} {ONE_INDEX_COLUMNS}"
        )))
        .await?;
        return Ok(format!("history index built on {part}"));
    }
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    sqlx::query("SET LOCAL lock_timeout = '2s'")
        .execute(&mut *tx)
        .await?;
    // The partition set is read again under the parent's lock: a week created
    // since has no index of its own yet, and its turn comes next pass.
    sqlx::query("LOCK TABLE public.attr_instances IN SHARE MODE")
        .execute(&mut *tx)
        .await?;
    let late: Option<String> = sqlx::query_scalar(PARTITIONS_WITHOUT_ONE_INDEX)
        .fetch_optional(&mut *tx)
        .await?;
    if late.is_some() {
        tx.rollback().await?;
        return Ok("history index: a new partition waits for its own".into());
    }
    let parts: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
         WHERE i.inhparent = 'public.attr_instances'::regclass",
    )
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE UNIQUE INDEX u_attr_instances_time ON ONLY public.attr_instances {ONE_INDEX_COLUMNS}"
    )))
    .execute(&mut *tx)
    .await?;
    for part in &parts {
        let (_, index, _) = one_index_names(part);
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER INDEX public.u_attr_instances_time ATTACH PARTITION {index}"
        )))
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DROP INDEX public.i_attr_instances_lookup, public.u_attr_instances")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(format!(
        "history on one index: {} partitions swapped",
        parts.len()
    ))
}

/// The partial index on each history partition that finds its audit rows
/// (provenance `audit: true`, ADR-N-054 2.8, DM-146), quoted bare and
/// qualified, and the partition qualified. A weekly suffix keeps the name
/// under the 63-byte identifier limit.
fn audit_index_names(part: &str) -> (String, String, String) {
    let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let index = q(&format!("{part}_audit"));
    (
        index.clone(),
        format!("public.{index}"),
        format!("public.{}", q(part)),
    )
}

/// The leaf partitions of both history parents without a valid
/// `{partition}_audit` index, with whether an invalid one (a build that
/// failed) is in the way, smallest first by `relpages` (a catalog read that
/// takes no lock). Not the DEFAULT partitions: no drop takes them, and a
/// build there would hold up each new week's `CREATE TABLE`, which locks
/// the parent against every write while it checks DEFAULT.
///
/// ponytail: the audit sweep reads a DEFAULT partition whole each minute;
/// it holds only backfill (`default_partition_load` warns past 100 000 rows).
const LEAVES_WITHOUT_AUDIT_INDEX: &str = "SELECT c.relname, x.indexrelid IS NOT NULL AS stale
     FROM (SELECT relid FROM pg_partition_tree('public.attr_instances') WHERE isleaf
           UNION ALL
           SELECT relid FROM pg_partition_tree('public.attr_rows') WHERE isleaf) t
     JOIN pg_class c ON c.oid = t.relid
     LEFT JOIN pg_class ic ON ic.relname = c.relname || '_audit'
                          AND ic.relnamespace = 'public'::regnamespace
     LEFT JOIN pg_index x ON x.indexrelid = ic.oid
     WHERE c.relkind = 'r' AND x.indisvalid IS NOT TRUE
       AND pg_get_expr(c.relpartbound, c.oid) <> 'DEFAULT'
     ORDER BY c.relpages";

/// Whether partition `name` holds no audit row, read through its
/// `{name}_audit` index. Without a valid one only an empty partition counts
/// as free: reading a full one whole could take minutes under the drop's
/// lock.
async fn audit_free(conn: &mut sqlx::PgConnection, name: &str) -> Result<bool, sqlx::Error> {
    let (_, index, table) = audit_index_names(name);
    let indexed: bool = sqlx::query_scalar(
        "SELECT coalesce((SELECT x.indisvalid FROM pg_index x
                          WHERE x.indexrelid = to_regclass($1)), false)",
    )
    .bind(&index)
    .fetch_one(&mut *conn)
    .await?;
    let probe = if indexed {
        format!(
            "SELECT NOT EXISTS (SELECT 1 FROM {table} WHERE provenance @> '{{\"audit\": true}}')"
        )
    } else {
        format!("SELECT NOT EXISTS (SELECT 1 FROM {table})")
    };
    sqlx::query_scalar(sqlx::AssertSqlSafe(probe))
        .fetch_one(conn)
        .await
}

/// How long one maintenance pass builds audit indexes for.
const AUDIT_INDEX_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);
/// How long one audit or expiry index build waits for a lock or an older
/// snapshot.
const INDEX_BUILD_WAIT: &str = "10s";

/// Give every history partition its `{partition}_audit` index (T-6337) with
/// `CREATE INDEX CONCURRENTLY`, so writes go on, smallest first, for up to
/// `AUDIT_INDEX_BUDGET`; a week created since gets its own on the next
/// pass. Until a partition has one, no partition drop takes it unless it is
/// empty. A concurrent build waits for every transaction with an older
/// snapshot, so one that waits `INDEX_BUILD_WAIT` gives up, leaving an
/// invalid index the next pass drops and builds again. `None` when nothing
/// was missing or another pod holds the step.
pub async fn history_audit_index(pool: &PgPool) -> Result<Option<String>, sqlx::Error> {
    use sqlx::Executor;
    let missing: Vec<(String, bool)> = sqlx::query_as(LEAVES_WITHOUT_AUDIT_INDEX)
        .fetch_all(pool)
        .await?;
    if missing.is_empty() {
        return Ok(None);
    }
    // As in history_one_index: a concurrent build runs outside a transaction
    // on a connection of its own, whose end releases the advisory lock.
    let mut conn = pool.acquire().await?.detach();
    let held: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext('antares.history_audit_index'))")
            .fetch_one(&mut conn)
            .await?;
    if !held {
        return Ok(None);
    }
    let started = std::time::Instant::now();
    let mut built = 0usize;
    let mut result = conn
        .execute(sqlx::AssertSqlSafe(format!(
            "SET statement_timeout = 0; SET lock_timeout = '{INDEX_BUILD_WAIT}'"
        )))
        .await
        .map(|_| ());
    for (part, stale) in &missing {
        if result.is_err() || started.elapsed() >= AUDIT_INDEX_BUDGET {
            break;
        }
        let (bare, index, table) = audit_index_names(part);
        if *stale {
            result = conn
                .execute(sqlx::AssertSqlSafe(format!(
                    "DROP INDEX CONCURRENTLY IF EXISTS {index}"
                )))
                .await
                .map(|_| ());
        }
        if result.is_ok() {
            result = conn
                .execute(sqlx::AssertSqlSafe(format!(
                    "CREATE INDEX CONCURRENTLY {bare} ON {table} (observed_at)
                     WHERE provenance @> '{{\"audit\": true}}'"
                )))
                .await
                .map(|_| ());
        }
        if result.is_ok() {
            built = built.saturating_add(1);
        }
    }
    end_session(conn).await?;
    result?;
    Ok(Some(format!(
        "audit index built on {built} of {} partitions",
        missing.len()
    )))
}

/// The audit sweep (T-6337, ADR-N-054 2.7): delete at most `batch` audit
/// rows (provenance `audit: true`) of each history table observed before
/// `before`, a UTC DateTime, across Tenants. Erasure (ADR-N-054 2.6) still
/// deletes them earlier; every other retention keeps them. Returns how many
/// went.
pub async fn retain_audit(pool: &PgPool, before: &str, batch: usize) -> Result<u64, sqlx::Error> {
    const AUDIT_SWEEP: [&str; 3] = [
        "DELETE FROM attr_instances
         WHERE provenance @> '{\"audit\": true}' AND observed_at < $1::timestamptz
           AND (tenant_id, entity_id, attr_id, instance_id, observed_at) IN (
             SELECT tenant_id, entity_id, attr_id, instance_id, observed_at FROM attr_instances
             WHERE provenance @> '{\"audit\": true}' AND observed_at < $1::timestamptz LIMIT $2)",
        "DELETE FROM attr_rows
         WHERE provenance @> '{\"audit\": true}' AND observed_at < $1::timestamptz
           AND (tenant_id, entity_no, attr_no, instance_no, observed_at) IN (
             SELECT tenant_id, entity_no, attr_no, instance_no, observed_at FROM attr_rows
             WHERE provenance @> '{\"audit\": true}' AND observed_at < $1::timestamptz LIMIT $2)",
        "DELETE FROM attr_versions WHERE ctid IN (
             SELECT ctid FROM attr_versions
             WHERE provenance @> '{\"audit\": true}' AND observed_at < $1::timestamptz LIMIT $2)",
    ];
    let before = antares_store::filter::canonical_datetime(before).into_owned();
    let mut total = 0u64;
    for sql in AUDIT_SWEEP {
        let mut tx = pool.begin().await?;
        crate::store::pg::set_service(&mut tx).await?;
        sqlx::query("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *tx)
            .await?;
        let n = sqlx::query(sql)
            .bind(&before)
            .bind(i64::try_from(batch).unwrap_or(i64::MAX))
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        total = total.saturating_add(n);
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LO: &str = "2026-08-17T00:00:00+00";
    const HI: &str = "2026-08-24T00:00:00+00";

    /// The partition name is an identifier the catalog hands back, not SQL.
    /// A bare interpolation drops any partition whose name needs quoting and
    /// resolves through `search_path` instead of the schema the row is in.
    #[test]
    fn a_partition_is_dropped_by_the_name_the_catalog_spells() {
        assert_eq!(
            drop_partition_sql("attr_instances_2026w34"),
            "DROP TABLE public.\"attr_instances_2026w34\""
        );
        assert_eq!(
            drop_partition_sql("Weird Name"),
            "DROP TABLE public.\"Weird Name\"",
            "a name that is not a bare identifier still names one table"
        );
        assert_eq!(
            drop_partition_sql("a\"b"),
            "DROP TABLE public.\"a\"\"b\"",
            "a quote in the name closes nothing"
        );
    }

    #[test]
    fn a_new_partition_gets_the_parents_policies() {
        assert_eq!(
            partition_rls_sql(false, "2026w34"),
            "SELECT public.antares_partition_rls('public.attr_instances_2026w34'::regclass)"
        );
        assert_eq!(
            partition_rls_sql(true, "2026w34"),
            "SELECT public.antares_partition_rls('public.attr_rows_2026w34'::regclass)"
        );
    }

    #[test]
    fn create_partition_covers_exactly_the_week() {
        let sql = create_partition_sql(false, "2026w34", LO, HI);
        assert!(sql.contains("attr_instances_2026w34 PARTITION OF attr_instances"));
        assert!(sql.contains(&format!("FROM ('{LO}') TO ('{HI}')")));
    }

    /// The week's table must be built standalone: `CREATE ... PARTITION OF`
    /// is the statement that just failed, so repeating it cannot recover the
    /// range.
    #[test]
    fn adopt_builds_the_table_standalone_then_attaches_it() {
        let [create, _move, attach] = adopt_default_rows_sql(false, "2026w34", LO, HI);
        assert!(create.contains("LIKE attr_instances"), "{create}");
        assert!(!create.contains("PARTITION OF"), "{create}");
        assert!(
            attach
                .starts_with("ALTER TABLE attr_instances ATTACH PARTITION attr_instances_2026w34"),
            "{attach}"
        );
        assert!(
            attach.contains(&format!("FROM ('{LO}') TO ('{HI}')")),
            "{attach}"
        );
    }

    /// The move is bounded by the partition range on BOTH sides. Unbounded, it
    /// would empty the DEFAULT partition of every historic-backfill row in the
    /// deployment and stuff them into one week.
    #[test]
    fn adopt_moves_only_the_partition_range_out_of_default() {
        let [_create, mv, _attach] = adopt_default_rows_sql(false, "2026w34", LO, HI);
        assert!(mv.contains("DELETE FROM attr_instances_default"), "{mv}");
        assert!(mv.contains(&format!("observed_at >= '{LO}'")), "{mv}");
        assert!(mv.contains(&format!("observed_at < '{HI}'")), "{mv}");
        assert!(
            mv.contains("INSERT INTO attr_instances_2026w34"),
            "moved rows must land in the week's table: {mv}"
        );
    }

    /// A week from the cut-over on (T-5200) is a week of `attr_rows`, built,
    /// moved and attached under its logged child `attr_rows_long` (0023,
    /// T-5249) only.
    #[test]
    fn a_keyed_week_goes_to_attr_rows() {
        let sql = create_partition_sql(true, "2026w34", LO, HI);
        assert!(
            sql.contains("attr_rows_2026w34 PARTITION OF attr_rows_long "),
            "{sql}"
        );
        let [create, mv, attach] = adopt_default_rows_sql(true, "2026w34", LO, HI);
        assert!(create.contains("(LIKE attr_rows_long "), "{create}");
        assert!(mv.contains("DELETE FROM attr_rows_default"), "{mv}");
        assert!(mv.contains("INSERT INTO attr_rows_2026w34"), "{mv}");
        assert!(mv.contains(&format!("observed_at < '{HI}'")), "{mv}");
        assert!(
            attach.starts_with("ALTER TABLE attr_rows_long ATTACH PARTITION attr_rows_2026w34"),
            "{attach}"
        );
        assert!(!format!("{sql}{create}{mv}{attach}").contains("attr_instances"));
    }
}
