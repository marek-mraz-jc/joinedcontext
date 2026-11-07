// SPDX-License-Identifier: EUPL-1.2
//! The store seam: ONE closed set of backends behind the
//! memory store's 12-method surface. An enum, not a trait — `mutate<T, E>` is
//! generic (dyn-incompatible), the backend set is closed by design
//! (exactly the implementations the product needs), and match
//! exhaustiveness forces every backend to answer every method.
//!
//! Every method returns `Result<_, NgsiError>`: the memory/file backend never
//! errors; the postgres backend maps sqlx failures to `InternalError` (500) —
//! a DB outage must be a visible 5xx, never a silent 404/409.

use antares_model::{NgsiError, TenantId};
use serde_json::Value;

#[cfg(feature = "postgres")]
use super::pg::doc::{DocKind, PgDocStore};
#[cfg(feature = "postgres")]
use super::pg::entity::PgEntityStore;
#[cfg(feature = "postgres")]
use super::pg::temporal::PgTemporalStore;
use super::{ChangeHook, Kind, Store};

/// 5.5.6: unexpected failures (database errors, timeouts) surface as
/// InternalError. The client-visible detail is deliberately generic —
/// driver internals (SQL text, constraint names, connection state) go to
/// the server log only.
#[cfg(feature = "postgres")]
pub(crate) fn db(e: sqlx::Error) -> NgsiError {
    // The store's own spec errors travel out through the same sqlx channel
    // (the signature is fixed by the callers), so recover them — by VALUE, so
    // the variant and therefore the status survive — before the generic
    // mapping turns them all into a 500.
    if let sqlx::Error::Configuration(b) = e {
        return match b.downcast::<NgsiError>() {
            Ok(n) => *n,
            Err(b) => {
                tracing::error!("database error: {b}");
                NgsiError::InternalError("database error".into())
            }
        };
    }
    // The pool handed out no connection inside its acquire timeout: the
    // broker is holding every connection it may hold and the caller waited
    // the whole wall. That is overload, not a fault — the binding answers it
    // 503 with Retry-After (`negotiate::ApiError::Overloaded`), so the detail
    // is the constant both ends name.
    if matches!(e, sqlx::Error::PoolTimedOut) {
        metrics::counter!("antares_pg_pool_timeouts_total").increment(1);
        tracing::warn!(
            "connection pool exhausted: no connection within the acquire timeout \
             (raise ANTARES_PG_POOL, or the request rate is above what this \
             database can serve)"
        );
        return NgsiError::InternalError(antares_model::error::DB_OVERLOADED.into());
    }
    // 5.5.2: "database timeouts" are InternalError. SQLSTATE 57014 is the
    // session's statement_timeout firing — named in the detail so a wall hit
    // reads differently from a broken query in the operator's log.
    if let sqlx::Error::Database(d) = &e {
        if d.code().as_deref() == Some("57014") {
            tracing::warn!("database statement timeout: {d}");
            return NgsiError::InternalError("database statement timeout".into());
        }
    }
    tracing::error!("database error: {e}");
    NgsiError::InternalError("database error".into())
}

/// 4.22: the read-boundary "now" — every entity read strips expired docs and
/// instances against this stamp (UTC Z, millisecond precision, the same form
/// the broker writes into system timestamps).
pub(crate) fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A document as its JSON text ([`AnyStore::query_entities_text`]).
fn raw(doc: &Value) -> Result<Box<serde_json::value::RawValue>, NgsiError> {
    serde_json::value::to_raw_value(doc).map_err(|e| NgsiError::InternalError(e.to_string()))
}

#[cfg(feature = "postgres")]
fn doc_kind(kind: Kind) -> Result<DocKind, NgsiError> {
    match kind {
        Kind::Subscription => Ok(DocKind::Subscription),
        Kind::Registration => Ok(DocKind::Registration),
        Kind::CSourceSubscription => Ok(DocKind::CSourceSubscription),
        Kind::Snapshot => Ok(DocKind::Snapshot),
        Kind::EntityMap => Ok(DocKind::EntityMap),
        Kind::DistSub => Ok(DocKind::DistSub),
        Kind::DeadLetter => Ok(DocKind::DeadLetter),
        Kind::View => Ok(DocKind::View),
        // Entities and Temporal Representations have their own tables and
        // never reach the document store. Every caller routes them to their
        // own match arm first; one that does not has mismatched its arms,
        // and the mistake stays inside the one request.
        Kind::Entity | Kind::Temporal => Err(NgsiError::InternalError(format!(
            "{kind:?} is not a document-store kind"
        ))),
    }
}

/// A transaction scoped to one tenant: begun, then RLS set for it first.
#[cfg(feature = "postgres")]
async fn tenant_tx(
    p: &PgBackend,
    tenant: &TenantId,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, NgsiError> {
    let mut tx = p.docs.pool().begin().await.map_err(db)?;
    crate::store::pg::set_tenant(&mut tx, tenant)
        .await
        .map_err(db)?;
    Ok(tx)
}

/// Postgres backend bundle: one pool, three table-family stores, plus the
/// change hook (the memory store emits its own; here the seam emits).
#[cfg(feature = "postgres")]
pub struct PgBackend {
    pub entities: PgEntityStore,
    pub temporal: PgTemporalStore,
    pub docs: PgDocStore,
    hook: std::sync::RwLock<Option<ChangeHook>>,
    /// Server and extension versions, read once at startup
    /// (`with_version`); `/q/health` serves this copy rather than querying
    /// the database on a polled endpoint.
    version: Value,
    /// Since when every readiness ping found every pool slot taken, `None`
    /// once one finds a slot (T-6182, see [`AnyStore::wedged`]).
    exhausted_since: std::sync::Mutex<Option<std::time::Instant>>,
    /// How long the pool may stay exhausted before liveness fails.
    wedged_after: std::time::Duration,
}

#[cfg(feature = "postgres")]
impl PgBackend {
    pub fn new(pool: sqlx::postgres::PgPool) -> Self {
        Self {
            entities: PgEntityStore::new(pool.clone()),
            temporal: PgTemporalStore::new(pool.clone()),
            docs: PgDocStore::new(pool),
            hook: std::sync::RwLock::new(None),
            version: Value::Object(serde_json::Map::new()),
            exhausted_since: std::sync::Mutex::new(None),
            wedged_after: POOL_WEDGED,
        }
    }

    /// Shorten [`POOL_WEDGED`], for tests that cannot wait five minutes.
    pub fn with_wedged_after(mut self, after: std::time::Duration) -> Self {
        self.wedged_after = after;
        self
    }

    /// Record a readiness ping: a timeout with every slot checked out
    /// starts (or keeps) the exhausted clock; any other answer stops it.
    fn note_ping(&self, res: &Result<sqlx::postgres::PgQueryResult, sqlx::Error>) {
        let pool = self.docs.pool();
        let all_held = matches!(res, Err(sqlx::Error::PoolTimedOut))
            && pool.num_idle() == 0
            && pool.size() >= pool.options().get_max_connections();
        let mut since = self
            .exhausted_since
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *since = match (*since, all_held) {
            (Some(t), true) => Some(t),
            (None, true) => Some(std::time::Instant::now()),
            (_, false) => None,
        };
    }

    async fn wedged(&self) -> bool {
        let since = *self
            .exhausted_since
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !since.is_some_and(|t| t.elapsed() >= self.wedged_after) {
            return false;
        }
        let opts = self.docs.pool().connect_options();
        let probe = async {
            use sqlx::Connection;
            let mut c = sqlx::PgConnection::connect_with(&opts).await?;
            // Dropped unclosed: close() over TLS can wait on a read (T-6000).
            sqlx::query("SELECT 1").execute(&mut c).await.map(|_| ())
        };
        let serves = matches!(tokio::time::timeout(WEDGE_PROBE, probe).await, Ok(Ok(())));
        if serves {
            tracing::error!(
                after_secs = self.wedged_after.as_secs(),
                "every pool slot has been held past the bound while the database \
                 answers a fresh connection: the pool leaked; failing liveness so \
                 the pod restarts"
            );
        }
        serves
    }

    /// Seal the notification credentials under `ANTARES_SECRET_KEY`
    /// (T-5838, [`super::pg::seal`]).
    pub fn with_secret_key(mut self, key: super::pg::seal::SecretKey) -> Self {
        self.docs = self.docs.with_secret_key(key);
        self
    }

    /// Give the batch writes their own pool (`ANTARES_PG_WRITE_RESERVE`,
    /// T-5150): reads keep the main pool, so a flood of them cannot queue
    /// ahead of a pipeline's batch upsert.
    pub fn with_write_reserve(mut self, reserve: sqlx::postgres::PgPool) -> Self {
        self.entities = self.entities.with_batch_pool(reserve);
        self
    }

    /// Attach what the startup probe read from the server
    /// (`pg::version_info`), so `/q/health` can answer it without a query.
    pub fn with_version(mut self, version: Value) -> Self {
        self.version = version;
        self
    }

    /// Poison recovery (`into_inner`) is deliberate, the same choice the
    /// memory arm records: the hook runs real code over attacker-shaped JSON,
    /// and a panic inside it must unwind one request, not poison this lock
    /// and panic every later entity write until the process restarts.
    async fn emit(&self, tenant: &TenantId, before: Option<Value>, after: Option<Value>) {
        // cloned out of the lock first: a read guard cannot be held across
        // the await the hook now costs.
        let hook = self
            .hook
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(h) = hook {
            h(tenant, before, after).await;
        }
    }
}

/// What `antares-api` sees. No core crate names redb or sqlx.
// One AnyStore exists per process — variant size difference is irrelevant.
#[allow(clippy::large_enum_variant)]
pub enum AnyStore {
    Mem(Store),
    #[cfg(feature = "postgres")]
    Pg(PgBackend),
}

/// A row's `id`, or `""` for one without: an id-less row sorts first and is
/// never skipped past, so a keyset walk over it cannot lose it. Only the
/// Postgres arm sorts rows it read whole; the memory arm's maps are already
/// keyed by id.
#[cfg(feature = "postgres")]
fn row_id(v: &Value) -> &str {
    v.get("id").and_then(Value::as_str).unwrap_or_default()
}

/// The bound on one size scan of a tenant (`GET /q/tenants/{tenant}`,
/// T-2890): the history table runs to tens of GB, and a size is not worth a
/// longer hold on a connection.
#[cfg(feature = "postgres")]
const SIZE_SCAN_TIMEOUT: &str = "SET LOCAL statement_timeout = '5s'";

/// Every table holding history rows (the partitions of both history
/// parents, a hypertable's chunks: all inherit), with the planner's row
/// count, its heap and TOAST bytes and the `tenant_id` statistics ANALYZE
/// left: `(rows, bytes, most common tenants, their frequencies, n_distinct)`.
/// Catalog reads only (`pg_class` pages, no relation opened), so it answers
/// in milliseconds at any history size and behind any lock.
#[cfg(feature = "postgres")]
const HISTORY_STATS: &str = "WITH RECURSIVE tree(oid) AS (
        SELECT 'attr_instances'::regclass::oid
        UNION ALL SELECT 'attr_rows'::regclass::oid
        UNION ALL SELECT i.inhrelid FROM pg_inherits i JOIN tree t ON i.inhparent = t.oid)
    SELECT greatest(c.reltuples, 0)::float8,
           ((c.relpages + coalesce(toast.relpages, 0))::bigint
             * current_setting('block_size')::bigint),
           s.most_common_vals::text::text[],
           s.most_common_freqs::float8[],
           s.n_distinct::float8
      FROM tree t
      JOIN pg_class c ON c.oid = t.oid
      JOIN pg_namespace n ON n.oid = c.relnamespace
      LEFT JOIN pg_class toast ON toast.oid = c.reltoastrelid
      LEFT JOIN pg_stats s ON s.schemaname = n.nspname AND s.tablename = c.relname
                          AND s.attname = 'tenant_id' AND NOT s.inherited
     WHERE c.relkind = 'r'";

/// The `entities` table's planner row count and its heap and TOAST bytes as
/// the last VACUUM or ANALYZE left them in `pg_class`: catalog reads only,
/// so no lock on the table holds it up (T-6159, T-6204).
#[cfg(feature = "postgres")]
const ENTITY_TABLE_STATS: &str = "SELECT greatest(c.reltuples, 0)::float8,
           ((c.relpages + coalesce(t.relpages, 0))::bigint
             * current_setting('block_size')::bigint)
      FROM pg_class c LEFT JOIN pg_class t ON t.oid = c.reltoastrelid
     WHERE c.oid = 'entities'::regclass";

/// A tenant's Entity bytes by first type from [`ENTITY_TABLE_STATS`] and the
/// tenant's row count per type: each row weighs the table's average row,
/// TOAST included; `None` while ANALYZE never counted the table. Without the
/// per-type counts (`None`: their count passed its bound) the tenant's
/// `entities` rows weigh the same and `by_type` stays empty (T-6204).
/// ponytail: a tenant whose rows are heavier than the table's average reads
/// low; a per-type sample would close that.
#[cfg(feature = "postgres")]
fn estimate_entities(
    tuples: f64,
    size: i64,
    counts: Option<&[(String, i64)]>,
    entities: u64,
) -> Option<antares_store::TenantBytes> {
    if tuples < 1.0 {
        return None;
    }
    let per_row = size.max(0) as f64 / tuples;
    let mut out = antares_store::TenantBytes::default();
    let Some(counts) = counts else {
        out.entities = (per_row * entities as f64).round() as u64;
        return Some(out);
    };
    for (ty, n) in counts {
        let b = (per_row * (*n).max(0) as f64).round() as u64;
        out.entities = out.entities.saturating_add(b);
        out.by_type.insert(ty.clone(), b);
    }
    Some(out)
}

/// One row of [`HISTORY_STATS`].
#[cfg(feature = "postgres")]
type HistoryStats = (f64, i64, Option<Vec<String>>, Option<Vec<f64>>, Option<f64>);

/// `tenant`'s `(instances, bytes)` from [`HISTORY_STATS`]: each table's rows
/// and bytes times the tenant's share of its rows. The share is the tenant's
/// most-common-value frequency, or else the rest spread evenly over the
/// distinct values ANALYZE did not list (a negative `n_distinct` is a
/// fraction of the rows). ponytail: a table ANALYZE never saw counts 0 rows;
/// autovacuum analyzes a partition once a tenth of it changed.
#[cfg(feature = "postgres")]
fn estimate_history(tenant: &str, rows: &[HistoryStats]) -> (u64, u64) {
    let (mut count, mut bytes) = (0.0_f64, 0.0_f64);
    for (tuples, size, vals, freqs, distinct) in rows {
        let (Some(vals), Some(freqs), Some(distinct)) = (vals, freqs, distinct) else {
            continue;
        };
        let share = match vals.iter().position(|v| v == tenant) {
            Some(i) => freqs.get(i).copied().unwrap_or(0.0),
            None => {
                let distinct = if *distinct < 0.0 {
                    -distinct * tuples
                } else {
                    *distinct
                };
                let others = distinct - vals.len() as f64;
                let rest = 1.0 - freqs.iter().sum::<f64>();
                if others >= 1.0 && rest > 0.0 {
                    rest / others
                } else {
                    0.0
                }
            }
        };
        count += share * tuples;
        bytes += share * (*size).max(0) as f64;
    }
    (count.round() as u64, bytes.round() as u64)
}

/// The attribute instances of one tenant, both history tables.
#[cfg(feature = "postgres")]
async fn count_instances(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    tenant: &TenantId,
) -> Result<u64, NgsiError> {
    let n: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM attr_instances WHERE tenant_id = $1)
              + (SELECT count(*) FROM attr_rows WHERE tenant_id = $1)",
    )
    .bind(tenant.as_str())
    .fetch_one(&mut **tx)
    .await
    .map_err(db)?;
    Ok(n.max(0) as u64)
}

/// How often an upsert runs its replace-then-create pair while concurrent
/// creates and deletes of the same id keep taking the row from under it.
#[cfg(feature = "postgres")]
const UPSERT_ROUNDS: usize = 3;

/// The refusal of an upsert that lost [`UPSERT_ROUNDS`] races: nothing was
/// stored, so it never answers success.
#[cfg(feature = "postgres")]
fn upsert_raced() -> NgsiError {
    NgsiError::InternalError(
        "the entity was created and deleted concurrently; nothing was stored, try again".into(),
    )
}

/// An upsert's two steps, `first` then `second` (each an expression that
/// awaits whether it took the row), until one of them took it: whether
/// `first` did. A create that lost its race to a writer whose row a delete
/// took again finds nothing to replace either, so the pair runs again, and
/// after [`UPSERT_ROUNDS`] the upsert is refused: it never answers success
/// for a write that stored nothing (F-R2ANT3-003). A macro, not an async
/// fn over closures: the steps' futures stay concrete, so the caller's
/// stays `Send`.
#[cfg(feature = "postgres")]
macro_rules! upsert_rounds {
    ($first:expr, $second:expr) => {{
        let mut took = Err(upsert_raced());
        for _ in 0..UPSERT_ROUNDS {
            if $first {
                took = Ok(true);
                break;
            }
            if $second {
                took = Ok(false);
                break;
            }
        }
        took
    }};
}

/// The longest the readiness ping waits for the database (see
/// [`AnyStore::ping`]).
#[cfg(feature = "postgres")]
pub const READY_PING: std::time::Duration = std::time::Duration::from_secs(1);

/// How long every readiness ping may find every pool slot taken before
/// `/q/health` fails liveness (T-6182: prod held 13 of 13 slots for 16 h,
/// ready failing and liveness green, so nothing restarted it). A loaded
/// pool still hands a slot to some ping within five minutes; a leaked one
/// never does.
#[cfg(feature = "postgres")]
pub const POOL_WEDGED: std::time::Duration = std::time::Duration::from_secs(300);

/// The bound on the fresh connection that tells a leaked pool from a lost
/// database, inside the liveness probe's 3 s.
#[cfg(feature = "postgres")]
const WEDGE_PROBE: std::time::Duration = std::time::Duration::from_secs(2);

impl AnyStore {
    /// Readiness ping: can the store answer a trivial request
    /// RIGHT NOW? Memory/file are in-process (always ready); the Pg arm runs
    /// `SELECT 1` so a lost database (failover, network partition) flips
    /// /q/ready to 503 and the Service stops routing to this pod.
    ///
    /// Bounded by [`READY_PING`]: with every pool slot taken the acquire
    /// waits its whole 5 s, past the probe's 3 s, and the kubelet logged a
    /// timeout instead of reading the 503 (prod 2026-10-28, T-6000).
    pub async fn ping(&self) -> Result<(), NgsiError> {
        match self {
            AnyStore::Mem(_) => Ok(()),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                let res = tokio::time::timeout(
                    READY_PING,
                    sqlx::query("SELECT 1").execute(p.docs.pool()),
                )
                .await
                .unwrap_or(Err(sqlx::Error::PoolTimedOut));
                p.note_ping(&res);
                res.map(|_| ()).map_err(db)
            }
        }
    }

    /// Liveness: has the pool stopped serving for good? True only when
    /// every readiness ping for [`POOL_WEDGED`] found every slot held AND a
    /// fresh connection beside the pool still reaches the database: then
    /// the slots leaked and only a restart frees them. A lost database
    /// answers false here, since a restart fixes nothing (T-6182).
    pub async fn wedged(&self) -> bool {
        match self {
            AnyStore::Mem(_) => false,
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.wedged().await,
        }
    }

    /// What this store runs on, for `/q/health`. The memory and file modes
    /// are one backend with two durability shapes and must not read as the
    /// same thing; the Postgres arm serves what the startup probe read.
    pub fn version_info(&self) -> Value {
        match self {
            AnyStore::Mem(s) => serde_json::json!({
                "engine": if s.shadowed() { "redb" } else { "memory" },
            }),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.version.clone(),
        }
    }

    /// (Queued writers, peak) behind the single redb committer, reported
    /// only by a store that has one: `None` for the Pg arm (Postgres has no
    /// single-writer commit queue) and for a pure in-memory store, whose
    /// lock depth is not the fsync queue this number stands for.
    pub fn commit_queue(&self) -> Option<(usize, usize)> {
        match self {
            AnyStore::Mem(s) => s.shadowed().then(|| s.commit_queue()),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(_) => None,
        }
    }

    /// The last step of the drain: close the connection pool so in-flight
    /// transactions finish and the server sees a clean disconnect instead of
    /// N abandoned backends. A no-op for the memory/file arm, whose durability
    /// is already commit-before-ack — there is nothing buffered to lose.
    pub async fn close(&self) {
        match self {
            AnyStore::Mem(_) => {}
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.docs.pool().close().await,
        }
    }

    pub fn set_change_hook(&self, h: ChangeHook) {
        match self {
            AnyStore::Mem(s) => s.set_change_hook(h),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                *p.hook
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(h)
            }
        }
    }

    /// Turn the same-tx outbox producer on (bus=nats). The memory arm has
    /// no outbox — the broker's wiring rejects bus=nats without a Pg store,
    /// so this is unreachable there by construction.
    pub fn set_outbox(
        &self,
        #[cfg_attr(not(feature = "postgres"), allow(unused_variables))] on: bool,
    ) {
        match self {
            AnyStore::Mem(_) => {}
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.entities.set_outbox(on),
        }
    }

    /// Seal the notification credentials a release before
    /// `ANTARES_SECRET_KEY` stored in plaintext (T-5838); the count of rows
    /// sealed. The memory arm seals nothing.
    pub async fn seal_plaintext_credentials(&self) -> Result<u64, NgsiError> {
        match self {
            AnyStore::Mem(_) => Ok(0),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.docs.seal_plaintext().await.map_err(db),
        }
    }

    /// Asynchronous commit for the batch writes (ANTARES_PG_BATCH_ASYNC_COMMIT,
    /// T-5050). The memory arm has no commit to relax.
    pub fn set_async_batch_commit(
        &self,
        #[cfg_attr(not(feature = "postgres"), allow(unused_variables))] on: bool,
    ) {
        match self {
            AnyStore::Mem(_) => {}
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.entities.set_async_batch_commit(on),
        }
    }

    /// Outbox drain: oldest-first page of pending rows `(seq, tenant, event)`.
    pub async fn outbox_peek(
        &self,
        #[cfg_attr(not(feature = "postgres"), allow(unused_variables))] limit: i64,
    ) -> Result<Vec<(i64, String, Value)>, NgsiError> {
        match self {
            AnyStore::Mem(_) => Ok(Vec::new()),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => super::pg::outbox::peek(p.docs.pool(), limit)
                .await
                .map_err(db),
        }
    }

    /// Outbox drain: delete EXACTLY the published rows (never a blanket
    /// `seq <= max`, which loses a row committing between peek and ack).
    pub async fn outbox_ack(
        &self,
        #[cfg_attr(not(feature = "postgres"), allow(unused_variables))] seqs: &[i64],
    ) -> Result<u64, NgsiError> {
        match self {
            AnyStore::Mem(_) => Ok(0),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => super::pg::outbox::ack(p.docs.pool(), seqs)
                .await
                .map_err(db),
        }
    }

    /// Outbox drain: keep the rows of events the bus could not carry whole.
    /// The memory arm has no outbox, so nothing is ever retained there.
    pub async fn outbox_retain(
        &self,
        #[cfg_attr(not(feature = "postgres"), allow(unused_variables))] tenant: &TenantId,
        #[cfg_attr(not(feature = "postgres"), allow(unused_variables))] seqs: &[i64],
    ) -> Result<u64, NgsiError> {
        match self {
            AnyStore::Mem(_) => Ok(0),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => super::pg::outbox::retain(p.docs.pool(), tenant, seqs)
                .await
                .map_err(db),
        }
    }

    /// The whole event behind a claim-check reference, `None` once reaped.
    pub async fn outbox_event(
        &self,
        #[cfg_attr(not(feature = "postgres"), allow(unused_variables))] seq: i64,
        #[cfg_attr(not(feature = "postgres"), allow(unused_variables))] tenant: &TenantId,
    ) -> Result<Option<Value>, NgsiError> {
        match self {
            AnyStore::Mem(_) => Ok(None),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => super::pg::outbox::event(p.docs.pool(), seq, tenant)
                .await
                .map_err(db),
        }
    }

    /// Create one document of `kind`; `false` when that id is already taken.
    pub async fn create(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        doc: Value,
    ) -> Result<bool, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.create(tenant, kind, id, doc).await),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                let created = match kind {
                    Kind::Entity => p.entities.create(tenant, id, &doc).await.map_err(db)?,
                    Kind::Temporal => p.temporal.create(tenant, id, &doc).await.map_err(db)?,
                    _ => {
                        let dk = doc_kind(kind)?;
                        p.docs.create(tenant, dk, id, &doc).await.map_err(db)?
                    }
                };
                if created && kind == Kind::Entity {
                    p.emit(tenant, None, Some(doc)).await;
                }
                Ok(created)
            }
        }
    }

    /// Batch create (entities only): one multi-row statement on the Pg
    /// arm, per-item loop on the memory arm. Created-flags in input order.
    pub async fn batch_create(
        &self,
        tenant: &TenantId,
        items: Vec<(String, Value)>,
    ) -> Result<Vec<bool>, NgsiError> {
        match self {
            AnyStore::Mem(s) => {
                let mut out = Vec::with_capacity(items.len());
                for (id, doc) in items {
                    out.push(s.create(tenant, Kind::Entity, &id, doc).await);
                }
                Ok(out)
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                let flags = p.entities.batch_create(tenant, &items).await.map_err(db)?;
                for ((_, doc), created) in items.into_iter().zip(&flags) {
                    if *created {
                        p.emit(tenant, None, Some(doc)).await;
                    }
                }
                Ok(flags)
            }
        }
    }

    /// Batch delete (entities only): deleted-flags in input order; a
    /// duplicate id in the input deletes once and 404s the second time,
    /// matching the per-item loop's semantics (5.5.11.4).
    pub async fn batch_delete(
        &self,
        tenant: &TenantId,
        ids: &[String],
    ) -> Result<Vec<bool>, NgsiError> {
        match self {
            AnyStore::Mem(s) => {
                let mut out = Vec::with_capacity(ids.len());
                for id in ids {
                    out.push(s.delete(tenant, Kind::Entity, id).await);
                }
                Ok(out)
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                let deleted = p.entities.batch_delete(tenant, ids).await.map_err(db)?;
                let mut prev: std::collections::HashMap<String, Value> =
                    deleted.into_iter().collect();
                let mut out = Vec::with_capacity(ids.len());
                for id in ids {
                    match prev.remove(id) {
                        Some(before) => {
                            p.emit(tenant, Some(before), None).await;
                            out.push(true);
                        }
                        None => out.push(false),
                    }
                }
                Ok(out)
            }
        }
    }

    pub async fn upsert(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        doc: Value,
    ) -> Result<bool, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.upsert(tenant, kind, id, doc).await),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => match kind {
                Kind::Entity => {
                    // replace-or-create without a pre-read: the replace first
                    // (it captures the true before-image under the row lock),
                    // else the create
                    let mut prev: Option<Value> = None;
                    let existed = upsert_rounds!(
                        p.entities
                            .mutate(tenant, id, |d| {
                                prev = Some(d.clone());
                                *d = doc.clone();
                                Ok::<(), std::convert::Infallible>(())
                            })
                            .await
                            .map_err(db)?
                            .is_some(),
                        p.entities.create(tenant, id, &doc).await.map_err(db)?
                    )?;
                    p.emit(tenant, prev, Some(doc)).await;
                    Ok(existed)
                }
                Kind::Temporal => upsert_rounds!(
                    p.temporal.create(tenant, id, &doc).await.map_err(db)?,
                    p.temporal
                        .mutate(tenant, id, |d| {
                            *d = doc.clone();
                            Ok::<(), std::convert::Infallible>(())
                        })
                        .await
                        .map_err(db)?
                        .is_some()
                )
                .map(|created: bool| !created),
                _ => {
                    let dk = doc_kind(kind)?;
                    p.docs.upsert(tenant, dk, id, &doc).await.map_err(db)
                }
            },
        }
    }

    pub async fn get(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
    ) -> Result<Option<Value>, NgsiError> {
        let doc = match self {
            AnyStore::Mem(s) => s.get(tenant, kind, id),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => match kind {
                Kind::Entity => p.entities.get(tenant, id).await.map_err(db)?,
                Kind::Temporal => p.temporal.get(tenant, id).await.map_err(db)?,
                _ => p.docs.get(tenant, doc_kind(kind)?, id).await.map_err(db)?,
            },
        };
        // 4.22: an expired entity is invalid context — a read serves it to
        // no one, whichever arm stored it (the Pg sweep lags by design).
        if kind == Kind::Entity {
            let now = now_utc();
            if let Some(mut d) = doc {
                if crate::store::filter::strip_expired(&mut d, &now) {
                    return Ok(None);
                }
                return Ok(Some(d));
            }
            return Ok(None);
        }
        Ok(doc)
    }

    /// [`Self::get`] of several Entities, in one transaction on Postgres
    /// (T-4085); an absent or expired id has no entry.
    pub async fn get_entities(
        &self,
        tenant: &TenantId,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, Value>, NgsiError> {
        let found: std::collections::HashMap<String, Value> = match self {
            AnyStore::Mem(s) => ids
                .iter()
                .filter_map(|id| Some(((*id).to_owned(), s.get(tenant, Kind::Entity, id)?)))
                .collect(),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.entities.get_many(tenant, ids).await.map_err(db)?,
        };
        let now = now_utc();
        Ok(found
            .into_iter()
            .filter_map(|(id, mut d)| {
                (!crate::store::filter::strip_expired(&mut d, &now)).then_some((id, d))
            })
            .collect())
    }

    pub async fn delete(&self, tenant: &TenantId, kind: Kind, id: &str) -> Result<bool, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.delete(tenant, kind, id).await),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => match kind {
                Kind::Entity => {
                    // the before-image comes from the DELETE's own RETURNING —
                    // same transaction, never a separate racy read
                    let prev = p.entities.delete(tenant, id).await.map_err(db)?;
                    let hit = prev.is_some();
                    if hit {
                        p.emit(tenant, prev, None).await;
                    }
                    Ok(hit)
                }
                Kind::Temporal => p.temporal.delete(tenant, id).await.map_err(db),
                _ => p.docs.delete(tenant, doc_kind(kind)?, id).await.map_err(db),
            },
        }
    }

    /// Delete one entity only if `keep` accepts the stored document, read
    /// and removed under one lock (see the driver trait).
    pub async fn delete_entity_if(
        &self,
        tenant: &TenantId,
        id: &str,
        keep: &(dyn for<'v> Fn(&'v Value) -> bool + Sync),
    ) -> Result<bool, NgsiError> {
        // 4.22 is applied to the document `keep` judges, exactly as `get`
        // applies it: an expired instance is not there to be matched on.
        let now = now_utc();
        let judge = |d: &Value| {
            let mut d = d.clone();
            !crate::store::filter::strip_expired(&mut d, &now) && keep(&d)
        };
        match self {
            AnyStore::Mem(s) => Ok(s.delete_if(tenant, id, &judge).await.is_some()),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                // the before-image comes from the DELETE's own RETURNING —
                // same transaction, never a separate racy read
                let prev = p.entities.delete_if(tenant, id, &judge).await.map_err(db)?;
                let hit = prev.is_some();
                if hit {
                    p.emit(tenant, prev, None).await;
                }
                Ok(hit)
            }
        }
    }

    pub async fn list(&self, tenant: &TenantId, kind: Kind) -> Result<Vec<Value>, NgsiError> {
        let mut rows = match self {
            AnyStore::Mem(s) => s.list(tenant, kind),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => match kind {
                Kind::Entity => p.entities.list(tenant).await.map_err(db)?,
                Kind::Temporal => p.temporal.list(tenant).await.map_err(db)?,
                _ => p.docs.list(tenant, doc_kind(kind)?).await.map_err(db)?,
            },
        };
        if kind == Kind::Entity {
            let now = now_utc();
            rows.retain_mut(|d| !crate::store::filter::strip_expired(d, &now));
        }
        Ok(rows)
    }

    /// Every document's id and digest (see `CurrentStateDriver::list_digests`):
    /// a Postgres doc kind digests in the database, so no body leaves it.
    pub async fn list_digests(
        &self,
        tenant: &TenantId,
        kind: Kind,
    ) -> Result<Vec<(String, u64)>, NgsiError> {
        #[cfg(feature = "postgres")]
        if let AnyStore::Pg(p) = self {
            if !matches!(kind, Kind::Entity | Kind::Temporal) {
                return p
                    .docs
                    .list_digests(tenant, doc_kind(kind)?)
                    .await
                    .map_err(db);
            }
        }
        Ok(antares_store::digests(self.list(tenant, kind).await?))
    }

    /// One id-ordered page of documents (see `CurrentStateDriver::list_page`).
    pub async fn list_page(
        &self,
        tenant: &TenantId,
        kind: Kind,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Value>, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.list_page(tenant, kind, after, limit)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => match kind {
                // Not doc kinds: they live in their own tables, behind their
                // own readers, and `doc_kind` refuses them.
                Kind::Entity => {
                    let mut rows = p
                        .entities
                        .list_page(tenant, after, i64::try_from(limit).unwrap_or(i64::MAX))
                        .await
                        .map_err(db)?;
                    // 4.22: the statement excluded expired ENTITIES, so no
                    // document leaves this page and its length still means
                    // what the caller reads it as; expired INSTANCES are
                    // stripped here, at the read boundary, as everywhere else.
                    let now = now_utc();
                    rows.retain_mut(|d| !crate::store::filter::strip_expired(d, &now));
                    Ok(rows)
                }
                // ponytail: temporal is sliced from the whole list, so a
                // tenant large enough still refuses this read for volume; the
                // upgrade is a keyset reader over the instance rows.
                // Temporal has no keyset reader of its own: its documents are
                // reconstructed from the instance rows, where the volume that
                // needs bounding is instances and not entities — the one kind
                // that does not yet honour `list_page`'s contract, and the
                // reason nothing internal walks temporal state.
                Kind::Temporal => {
                    let mut rows = AnyStore::list(self, tenant, kind).await?;
                    rows.sort_by(|a, b| row_id(a).cmp(row_id(b)));
                    Ok(rows
                        .into_iter()
                        .skip_while(|r| after.is_some_and(|a| row_id(r) <= a))
                        .take(limit)
                        .collect())
                }
                _ => p
                    .docs
                    .list_page(
                        tenant,
                        doc_kind(kind)?,
                        after,
                        i64::try_from(limit).unwrap_or(i64::MAX),
                    )
                    .await
                    .map_err(db),
            },
        }
    }

    /// One id-ordered window of documents and the total (see
    /// `CurrentStateDriver::list_slice`).
    pub async fn list_slice(
        &self,
        tenant: &TenantId,
        kind: Kind,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<Value>, usize), NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.list_slice(tenant, kind, offset, limit)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => match kind {
                // Not doc kinds: each reads its own window in SQL, so the
                // whole-list ceiling never refuses a page (F-ANT1-011).
                Kind::Entity => {
                    let (mut rows, total) = p
                        .entities
                        .list_slice(
                            tenant,
                            i64::try_from(offset).unwrap_or(i64::MAX),
                            i64::try_from(limit).unwrap_or(i64::MAX),
                        )
                        .await
                        .map_err(db)?;
                    // 4.22 at the read boundary, as `list_page` does
                    let now = now_utc();
                    rows.retain_mut(|d| !crate::store::filter::strip_expired(d, &now));
                    Ok((rows, usize::try_from(total).unwrap_or(usize::MAX)))
                }
                // ponytail: temporal is sliced from the whole list, as in
                // `list_page`: its documents are rebuilt from instance rows
                // and the paged temporal query skips shells without history;
                // the upgrade is a keyset reader over temporal_entities.
                Kind::Temporal => {
                    let mut rows = AnyStore::list(self, tenant, kind).await?;
                    rows.sort_by(|a, b| row_id(a).cmp(row_id(b)));
                    let total = rows.len();
                    Ok((rows.into_iter().skip(offset).take(limit).collect(), total))
                }
                _ => {
                    let (page, total) = p
                        .docs
                        .list_slice(
                            tenant,
                            doc_kind(kind)?,
                            i64::try_from(offset).unwrap_or(i64::MAX),
                            i64::try_from(limit).unwrap_or(i64::MAX),
                        )
                        .await
                        .map_err(db)?;
                    Ok((page, usize::try_from(total).unwrap_or(usize::MAX)))
                }
            },
        }
    }

    /// 5.12 registration candidates for these entity ids / types. The Pg arm
    /// reads the `csource_index` rows (an indexed narrowing); `memory`/`file`
    /// have nothing to push into and return the same snapshot `list` does.
    /// Either way the result is a SUPERSET — the caller's matcher decides
    /// every 5.12 condition, this only avoids reading registrations that
    /// cannot match on id or type. `types` must be expanded plain type IRIs
    /// (what the registration write stored); a caller holding terms or a 4.17
    /// selection expression passes `None` and narrows on ids alone.
    pub async fn matching_registrations(
        &self,
        tenant: &TenantId,
        #[cfg_attr(not(feature = "postgres"), allow(unused_variables))] ids: Option<&[String]>,
        #[cfg_attr(not(feature = "postgres"), allow(unused_variables))] types: Option<&[String]>,
    ) -> Result<Vec<Value>, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.list(tenant, Kind::Registration)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p
                .docs
                .matching_registrations(tenant, ids, types)
                .await
                .map_err(db),
        }
    }

    /// Query Entities with the filter pushed down where the backend
    /// can take it: SQL for Postgres, the borrowed documents for
    /// `memory`/`file` (`Store::query_entities`). Either way the caller
    /// applies the exact filter afterwards unless the outcome says the store
    /// already did.
    pub async fn query_entities(
        &self,
        tenant: &TenantId,
        f: &crate::store::filter::EntityFilter<'_>,
    ) -> Result<crate::store::filter::QueryOutcome, NgsiError> {
        self.query_paged(tenant, f, false).await
    }

    /// `Store::query_candidates`: the Pg arm pages the candidates it cannot
    /// decide; the memory arm answers as `query_entities`.
    pub async fn query_candidates(
        &self,
        tenant: &TenantId,
        f: &crate::store::filter::EntityFilter<'_>,
    ) -> Result<crate::store::filter::QueryOutcome, NgsiError> {
        self.query_paged(tenant, f, true).await
    }

    /// `query_entities` with each row as its JSON text (T-6055): the Pg arm
    /// decodes no `Value` tree, and only a row that names `expiresAt` is
    /// parsed for 4.22 stripping; the memory arm writes its documents out.
    pub async fn query_entities_text(
        &self,
        tenant: &TenantId,
        f: &crate::store::filter::EntityFilter<'_>,
    ) -> Result<crate::store::filter::QueryOutcome<Box<serde_json::value::RawValue>>, NgsiError>
    {
        match self {
            AnyStore::Mem(_) => {
                let o = self.query_paged(tenant, f, false).await?;
                Ok(crate::store::filter::QueryOutcome {
                    rows: o
                        .rows
                        .into_iter()
                        .map(|d| raw(&d))
                        .collect::<Result<_, _>>()?,
                    decided: o.decided,
                    paged: o.paged,
                    total: o.total,
                })
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                let mut outcome = p.entities.query_text(tenant, f).await.map_err(db)?;
                let now = now_utc();
                let mut kept = Vec::with_capacity(outcome.rows.len());
                for row in outcome.rows {
                    if !row.get().contains("expiresAt") {
                        kept.push(row);
                        continue;
                    }
                    let mut doc: Value = serde_json::from_str(row.get())
                        .map_err(|e| NgsiError::InternalError(e.to_string()))?;
                    if !crate::store::filter::strip_expired(&mut doc, &now) {
                        kept.push(raw(&doc)?);
                    }
                }
                outcome.rows = kept;
                Ok(outcome)
            }
        }
    }

    async fn query_paged(
        &self,
        tenant: &TenantId,
        f: &crate::store::filter::EntityFilter<'_>,
        candidates: bool,
    ) -> Result<crate::store::filter::QueryOutcome, NgsiError> {
        let mut outcome = match self {
            AnyStore::Mem(s) => {
                let _ = candidates;
                s.query_entities(tenant, f)
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => match candidates {
                true => p.entities.query_candidates(tenant, f).await,
                false => p.entities.query(tenant, f).await,
            }
            .map_err(db)?,
        };
        // 4.22: the Pg arm already excludes expired ENTITIES in SQL (so
        // paging/totals stay exact); instance stripping — and the whole job
        // on the memory arm — happens here at the read boundary.
        let now = now_utc();
        outcome
            .rows
            .retain_mut(|d| !crate::store::filter::strip_expired(d, &now));
        Ok(outcome)
    }

    /// The Entity types of the Tenant's live Entities, sorted, read without
    /// the documents (T-4610): the `types` column on Postgres, the held
    /// documents' `type` on the memory arm.
    pub async fn entity_type_names(&self, tenant: &TenantId) -> Result<Vec<String>, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.entity_type_names(tenant, &now_utc())),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.entities.type_names(tenant).await.map_err(db),
        }
    }

    /// The (type, member name) pairs of the Tenant's attribute catalogue,
    /// sorted (T-4934): table `entity_attrs` on Postgres, which may still
    /// list a pair no live Entity carries; the live documents in memory.
    pub async fn entity_attr_pairs(
        &self,
        tenant: &TenantId,
    ) -> Result<Vec<(String, String)>, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.entity_attr_pairs(tenant, &now_utc())),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.entities.attr_pairs(tenant).await.map_err(db),
        }
    }

    /// The types of the Tenant's live Entities that carry one of `any_of`,
    /// sorted (T-4683): the types index on Postgres, one pass in memory.
    pub async fn entity_types_beside(
        &self,
        tenant: &TenantId,
        any_of: &[String],
    ) -> Result<Vec<String>, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.entity_types_beside(tenant, any_of, &now_utc())),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.entities.types_beside(tenant, any_of).await.map_err(db),
        }
    }

    /// The id segments the Pg arm reads by its C-collated id index
    /// (T-5661); the memory arm keeps none and its probes narrow by literal.
    pub async fn entity_id_segments(
        &self,
        tenant: &TenantId,
        lead: &str,
        sep: char,
        max: usize,
    ) -> Result<Option<Vec<String>>, NgsiError> {
        match self {
            AnyStore::Mem(_) => {
                let _ = (tenant, lead, sep, max);
                Ok(None)
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p
                .entities
                .id_segments(tenant, lead, sep, max)
                .await
                .map_err(db),
        }
    }

    /// Query Temporal Evolution with entity narrowing, instance-window
    /// pruning AND entity paging pushed down. Same
    /// contract: the API's window() is the arbiter; the memory arm returns
    /// the full snapshot, never paged.
    pub async fn query_temporal(
        &self,
        tenant: &TenantId,
        #[cfg_attr(not(feature = "postgres"), allow(unused_variables))]
        f: &crate::store::filter::TemporalFilter<'_>,
    ) -> Result<crate::store::filter::TemporalOutcome, NgsiError> {
        let mut outcome = match self {
            AnyStore::Mem(s) => crate::store::filter::TemporalOutcome {
                rows: s.list(tenant, Kind::Temporal),
                paged: false,
                total: None,
                aggregated: false,
            },
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.temporal.query(tenant, f).await.map_err(db)?,
        };
        // 4.22 on temporal reads: the Pg arm already dropped expired ENTITIES in
        // SQL (paging exact); this strips expired attribute INSTANCES, and does
        // the whole job on the memory arm (no SQL to push into).
        let now = now_utc();
        outcome
            .rows
            .retain_mut(|d| !crate::store::filter::strip_expired(d, &now));
        Ok(outcome)
    }

    /// Auto-recording fast path: append instances to an
    /// entity's temporal evolution, creating the meta shell on first touch.
    /// Pg: pure multi-row INSERT — no history read, no doc rewrite. Memory:
    /// the create-or-extend the mirror always did, under the store lock.
    /// `shell` carries the meta members; `additions` maps attr IRI →
    /// instance array (instanceIds already stamped by the caller).
    ///
    /// Both arms record only for an entity that still exists: 5.6.6 deletes
    /// the entity and then the temporal evolution recorded for it, so an
    /// append overlapping the delete must not recreate history nothing will
    /// ever clean again. An instance marked `temporal_only` never holds the
    /// entities (they live in another backend) and records unconditionally;
    /// there the delete overlap stays a window the temporal delete closes.
    pub async fn temporal_append(
        &self,
        tenant: &TenantId,
        id: &str,
        shell: &Value,
        additions: &Value,
    ) -> Result<(), NgsiError> {
        match self {
            AnyStore::Mem(s) => {
                if !s.temporal_only && s.get(tenant, Kind::Entity, id).is_none() {
                    return Ok(());
                }
                if s.get(tenant, Kind::Temporal, id).is_none() {
                    // loser of a concurrent create race just extends below
                    let _ = s.create(tenant, Kind::Temporal, id, shell.clone()).await;
                }
                s.mutate(tenant, Kind::Temporal, id, |doc| {
                    let target = doc.as_object_mut().ok_or(())?;
                    // 4.8: the Entity's modifiedAt is its last write, an
                    // Attribute's update included; never moved backwards
                    if let Some(m) = shell.get("modifiedAt").and_then(Value::as_str) {
                        let stale = target
                            .get("modifiedAt")
                            .and_then(Value::as_str)
                            .is_none_or(|cur| super::mem::earlier(cur, m));
                        if stale {
                            target.insert("modifiedAt".into(), Value::String(m.to_owned()));
                        }
                    }
                    if let Some(adds) = additions.as_object() {
                        for (k, v) in adds {
                            let mut incoming: Vec<Value> =
                                v.as_array().cloned().unwrap_or_default();
                            // ponytail: the memory store keeps no provenance and no
                            // versions (T-4665); Postgres does.
                            incoming.iter_mut().for_each(|i| {
                                antares_store::take_provenance(i);
                            });
                            match target.get_mut(k).and_then(Value::as_array_mut) {
                                // same instanceId = the same instance corrected
                                // (the pg arm's ON CONFLICT DO UPDATE)
                                Some(cur) => {
                                    for inst in incoming {
                                        let iid = inst.get("instanceId");
                                        match cur.iter_mut().find(|c| c.get("instanceId") == iid) {
                                            Some(slot) => *slot = inst,
                                            None => cur.push(inst),
                                        }
                                    }
                                }
                                None => {
                                    target.insert(k.clone(), Value::Array(incoming));
                                }
                            }
                        }
                    }
                    Ok::<(), ()>(())
                })
                .await;
                Ok(())
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p
                .temporal
                .append(tenant, id, shell, additions)
                .await
                .map_err(db),
        }
    }

    /// Mark this instance as the temporal half of a mixed deployment: the
    /// entities live in another backend, so appends never look for them here.
    pub fn temporal_only(mut self) -> Self {
        match &mut self {
            AnyStore::Mem(s) => s.temporal_only = true,
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.temporal.temporal_only = true,
        }
        self
    }

    /// Retrieve Temporal Evolution with the same instance pruning.
    pub async fn get_temporal(
        &self,
        tenant: &TenantId,
        id: &str,
        #[cfg_attr(not(feature = "postgres"), allow(unused_variables))]
        f: &crate::store::filter::TemporalFilter<'_>,
    ) -> Result<Option<Value>, NgsiError> {
        let mut doc = match self {
            AnyStore::Mem(s) => s.get(tenant, Kind::Temporal, id),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.temporal.get_range(tenant, id, f).await.map_err(db)?,
        };
        // 4.22: expired entity → None (Pg already did this in SQL); otherwise
        // strip expired instances.
        if let Some(d) = &mut doc {
            let now = now_utc();
            if crate::store::filter::strip_expired(d, &now) {
                return Ok(None);
            }
        }
        Ok(doc)
    }

    pub async fn mutate<T, E>(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        f: impl FnOnce(&mut Value) -> Result<T, E>,
    ) -> Result<Option<Result<T, E>>, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.mutate(tenant, kind, id, f).await),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => match kind {
                Kind::Entity => {
                    // before/after captured for the change hook's
                    // prev_payload INSIDE the row lock — a before-image read
                    // in its own transaction can belong to a different version
                    // than the one the lock serialized on.
                    let mut before: Option<Value> = None;
                    let mut after: Option<Value> = None;
                    let r = p
                        .entities
                        .mutate(tenant, id, |d| {
                            before = Some(d.clone());
                            let r = f(d);
                            if r.is_ok() {
                                after = Some(d.clone());
                            }
                            r
                        })
                        .await
                        .map_err(db)?;
                    if let (Some(Ok(_)), Some(a)) = (&r, after) {
                        if before.as_ref() != Some(&a) {
                            p.emit(tenant, before, Some(a)).await;
                        }
                    }
                    Ok(r)
                }
                Kind::Temporal => p.temporal.mutate(tenant, id, f).await.map_err(db),
                _ => {
                    // FOR UPDATE + UPDATE in one tx: a bookkeeping writeback
                    // racing a DELETE must never resurrect the row (047_06).
                    let dk = doc_kind(kind)?;
                    match p.docs.mutate(tenant, dk, id, f).await.map_err(db)? {
                        Some(r) => Ok(Some(r)),
                        None => Ok(None),
                    }
                }
            },
        }
    }

    /// Batch upsert with REPLACE semantics for entities:
    /// one statement + one transaction on the Pg arm, per-item loop on the
    /// memory arm. Created-flags in input order.
    pub async fn batch_upsert(
        &self,
        tenant: &TenantId,
        items: Vec<(String, Value)>,
    ) -> Result<Vec<bool>, NgsiError> {
        // Each id once per call, on every arm: Postgres refuses an ON CONFLICT
        // row affected twice and the memory arm would keep the last, so a
        // repeat is the caller's bug (the API splits repeats into rounds).
        let mut seen = std::collections::HashSet::with_capacity(items.len());
        if let Some((id, _)) = items.iter().find(|(id, _)| !seen.insert(id.as_str())) {
            return Err(NgsiError::InternalError(format!(
                "batch_upsert names {id} twice"
            )));
        }
        match self {
            AnyStore::Mem(s) => {
                let mut out = Vec::with_capacity(items.len());
                for (id, doc) in items {
                    out.push(!s.upsert(tenant, Kind::Entity, &id, doc).await);
                }
                Ok(out)
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                let out = p
                    .entities
                    .batch_upsert_replace(tenant, &items)
                    .await
                    .map_err(db)?;
                for ((_, doc), (_, prev)) in items.iter().zip(&out) {
                    p.emit(tenant, prev.clone(), Some(doc.clone())).await;
                }
                Ok(out.into_iter().map(|(created, _)| created).collect())
            }
        }
    }

    /// Batch read-modify-write for entities: one transaction + one ordered
    /// lock set + one multi-row writeback on the Pg arm; per-item mutate on
    /// the memory arm. Results align with `ids` (`None` = absent).
    pub async fn batch_mutate<E>(
        &self,
        tenant: &TenantId,
        ids: &[String],
        mut f: impl FnMut(&str, &mut Value) -> Result<(), E>,
    ) -> Result<Vec<Option<Result<(), E>>>, NgsiError> {
        match self {
            AnyStore::Mem(s) => {
                let mut out = Vec::with_capacity(ids.len());
                for id in ids {
                    out.push(s.mutate(tenant, Kind::Entity, id, |d| f(id, d)).await);
                }
                Ok(out)
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                // hook images captured inside the lock, same as single mutate
                let mut images: Vec<(Value, Value)> = Vec::new();
                let r = p
                    .entities
                    .batch_mutate(tenant, ids, |id, d| {
                        let before = d.clone();
                        let r = f(id, d);
                        if r.is_ok() && *d != before {
                            images.push((before, d.clone()));
                        }
                        r
                    })
                    .await
                    .map_err(db)?;
                for (before, after) in images {
                    p.emit(tenant, Some(before), Some(after)).await;
                }
                Ok(r)
            }
        }
    }

    /// 4.22 GC for the memory/file arm (the Pg arm's sweep lives in the
    /// maintenance job, mode-switched in the broker). Returns reaped count.
    pub fn sweep_expired(&self) -> usize {
        match self {
            AnyStore::Mem(s) => s.sweep_expired(&now_utc()),
            // 4.22 reaping on the Pg arm is `pg::maintenance`'s job
            // (`reap_expired_entities` / `reap_expired_instances`), which
            // deletes in one indexed statement instead of walking the store;
            // reads refuse an expired document either way.
            #[cfg(feature = "postgres")]
            AnyStore::Pg(_) => 0,
        }
    }

    /// 5.5.10: does the Tenant exist? The default Tenant "implicitly exists";
    /// others exist once implicitly created by a create operation.
    ///
    /// It answers for CLIENT tenants. A tenant the broker minted for itself
    /// does hold a row — every write claims one, and that row is what keeps
    /// the tenant enumerable to `subscription_tenants` across a restart — but
    /// nothing asks this about one: the wall that asks sits outside the 6.3.22
    /// snapshot scoping and reads the header the caller sent, which cannot
    /// name an internal tenant. What a Snapshot's synthetic tenant holds is
    /// asserted by the Snapshot document; the row only accounts for it, and
    /// `tenant_ids` keeps it out of the customer-account listing.
    pub async fn tenant_exists(&self, tenant: &TenantId) -> Result<bool, NgsiError> {
        if tenant.as_str() == TenantId::DEFAULT {
            return Ok(true);
        }
        match self {
            AnyStore::Mem(s) => Ok(s.tenant_exists(tenant)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                async {
                    let row =
                        sqlx::query_scalar::<_, i32>("SELECT 1 FROM tenants WHERE tenant_id = $1")
                            .bind(tenant.as_str())
                            .fetch_optional(p.docs.pool())
                            .await
                            .map_err(db)?;
                    Ok(row.is_some())
                }
                .await
            }
        }
    }

    /// Every tenant name the backend knows, sorted. One cheap query: at the
    /// 10 000-tenant target (ADR-0001) an inventory that counted every kind
    /// of every tenant would be 7 counts per tenant on one transaction, so
    /// the counts live in `tenant_stats_one` and are paid per lookup.
    pub async fn tenant_ids(&self) -> Result<Vec<String>, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.tenant_ids()),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                async {
                    let mut rows: Vec<String> =
                        sqlx::query_scalar("SELECT tenant_id FROM tenants ORDER BY 1")
                            .fetch_all(p.docs.pool())
                            .await
                            .map_err(db)?;
                    // The broker's own tenants claim a row like any other
                    // write, because that row is also the enumeration the
                    // notification paths walk. The inventory is the list of
                    // customer accounts, so they are filtered on the way out.
                    rows.retain(|t| !TenantId::is_reserved_str(t));
                    // 5.5.10: the default Tenant implicitly exists, whether or
                    // not a row was ever written for it.
                    if !rows.iter().any(|t| t == TenantId::DEFAULT) {
                        rows.push(TenantId::DEFAULT.to_owned());
                        rows.sort();
                    }
                    Ok(rows)
                }
                .await
            }
        }
    }

    /// What one tenant holds; `None` when it does not exist (5.5.10 keeps
    /// the default Tenant existing even when empty).
    pub async fn tenant_stats_one(
        &self,
        tenant: &TenantId,
    ) -> Result<Option<antares_store::TenantStats>, NgsiError> {
        if !self.tenant_exists(tenant).await? {
            return Ok(None);
        }
        match self {
            AnyStore::Mem(s) => Ok(Some(s.tenant_stats_one(tenant))),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                async {
                    let mut tx = tenant_tx(p, tenant).await?;
                    // the tenant setting is what the RLS-guarded counts see
                    let created_at: Option<String> = sqlx::query_scalar(
                        "SELECT created_at::text FROM tenants WHERE tenant_id = $1",
                    )
                    .bind(tenant.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db)?;
                    let c: (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
                        "SELECT (SELECT count(*) FROM entities WHERE tenant_id = $1),
                            (SELECT count(*) FROM subscriptions WHERE tenant_id = $1),
                            (SELECT count(*) FROM csource_registrations WHERE tenant_id = $1),
                            (SELECT count(*) FROM csource_subscriptions WHERE tenant_id = $1),
                            (SELECT count(*) FROM snapshots WHERE tenant_id = $1),
                            (SELECT count(*) FROM entity_map_docs WHERE tenant_id = $1),
                            (SELECT count(*) FROM dist_subs WHERE tenant_id = $1)",
                    )
                    .bind(tenant.as_str())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db)?;
                    tx.commit().await.map_err(db)?;
                    Ok(Some(antares_store::TenantStats {
                        tenant: tenant.as_str().to_owned(),
                        created_at,
                        entities: c.0 as u64,
                        subscriptions: c.1 as u64,
                        registrations: c.2 as u64,
                        csource_subscriptions: c.3 as u64,
                        snapshots: c.4 as u64,
                        entity_maps: c.5 as u64,
                        dist_subs: c.6 as u64,
                    }))
                }
                .await
            }
        }
    }

    /// Purge the current-state half of one tenant in one transaction;
    /// `false` when the tenant did not exist. The default tenant's row stays.
    pub async fn purge_tenant(&self, tenant: &TenantId) -> Result<bool, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.purge_tenant(tenant)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                async {
                    let mut tx = tenant_tx(p, tenant).await?;
                    let known = sqlx::query_scalar::<_, i32>(
                        "SELECT 1 FROM tenants WHERE tenant_id = $1 FOR UPDATE",
                    )
                    .bind(tenant.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db)?
                    .is_some();
                    if !known {
                        return Ok(false);
                    }
                    for table in [
                        "entities",
                        // emptied by the delete trigger of `entities` (0024)
                        "entity_values",
                        "subscriptions",
                        "csource_subscriptions",
                        "csource_registrations",
                        "csource_index",
                        "outbox",
                        "snapshots",
                        "entity_map_docs",
                        "dist_subs",
                        "dead_letters",
                        "views",
                        "entity_types",
                        "entity_attrs",
                        // ADR-0021: `tenant_id` is GENERATED from the row's kind
                        // and owner, so a Cached row's is NULL and this DELETE
                        // takes only what the Tenant stored itself.
                        "jsonld_contexts",
                    ] {
                        sqlx::query(sqlx::AssertSqlSafe(format!(
                            "DELETE FROM {table} WHERE tenant_id = $1"
                        )))
                        .bind(tenant.as_str())
                        .execute(&mut *tx)
                        .await
                        .map_err(db)?;
                    }
                    if tenant.as_str() != TenantId::DEFAULT {
                        sqlx::query("DELETE FROM tenants WHERE tenant_id = $1")
                            .bind(tenant.as_str())
                            .execute(&mut *tx)
                            .await
                            .map_err(db)?;
                    }
                    tx.commit().await.map_err(db)?;
                    Ok(true)
                }
                .await
            }
        }
    }

    pub async fn subscription_tenants(&self) -> Result<Vec<String>, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.subscription_tenants()),
            // Pg answers the superset the contract allows: every known
            // tenant. Narrowing it would need a cross-tenant read of
            // `subscriptions`, and that table is deliberately outside the
            // `antares.service` escape — a subscription document carries
            // endpoint.receiverInfo, i.e. the credentials the notification
            // is sent with. The callers list per tenant under set_tenant
            // afterwards, so the extra names cost one empty list each.
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                async {
                    let rows =
                        sqlx::query_scalar::<_, String>("SELECT tenant_id FROM tenants ORDER BY 1")
                            .fetch_all(p.docs.pool())
                            .await
                            .map_err(db)?;
                    Ok(rows)
                }
                .await
            }
        }
    }

    pub async fn context_put(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
        doc: Value,
    ) -> Result<(), NgsiError> {
        // ADR-0021, the write half no `WHERE` on an existing row can state:
        // the row that ARRIVES must belong to the caller too, or a Tenant
        // could store term mappings under another Tenant's name and decide
        // what that Tenant's payloads mean (5.5.7). Postgres says the same
        // thing as the policy's WITH CHECK; this holds under the roles that
        // bypass RLS.
        if !antares_store::context_row_visible(&doc, tenant) {
            return Err(NgsiError::InternalError(
                "@context belongs to another tenant".into(),
            ));
        }
        // 5.13.1 names three kinds and the row's kind decides who reaches it
        // (ADR-0021): a row without one would be Cached to Postgres and
        // nothing at all to the memory arm.
        if doc.get("kind").and_then(Value::as_str).is_none() {
            return Err(NgsiError::InternalError(
                "@context row without a kind".into(),
            ));
        }
        match self {
            AnyStore::Mem(s) => s.context_put(tenant, id, doc),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                let kind = doc
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("Cached")
                    .to_owned();
                p.docs
                    .context_put(tenant, id, &doc, &kind)
                    .await
                    .map_err(db)
            }
        }
    }

    pub async fn context_get(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
    ) -> Result<Option<Value>, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.context_get(tenant, id)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.docs.context_get(tenant, id).await.map_err(db),
        }
    }

    pub async fn context_mutate(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
        f: antares_store::MutateFn<'_>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        // the row a closure leaves must still be one this caller may store,
        // and keep its kind: the same two checks context_put makes
        let guarded = move |doc: &mut Value| {
            let kind = doc.get("kind").cloned();
            f(doc)?;
            if doc.get("kind") != kind.as_ref() || !antares_store::context_row_visible(doc, tenant)
            {
                return Err(());
            }
            Ok(())
        };
        match self {
            AnyStore::Mem(s) => Ok(s.context_mutate(tenant, id, guarded)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.docs.context_mutate(tenant, id, guarded).await.map_err(db),
        }
    }

    pub async fn context_delete(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
    ) -> Result<bool, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.context_delete(tenant, id)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.docs.context_delete(tenant, id).await.map_err(db),
        }
    }

    pub async fn context_list_meta(
        &self,
        tenant: Option<&TenantId>,
    ) -> Result<Vec<Value>, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.context_list_meta(tenant)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.docs.context_list_meta(tenant).await.map_err(db),
        }
    }
}

#[cfg(all(test, feature = "postgres"))]
mod db_error_tests {
    #[allow(unused_imports)]
    use antares_model::NgsiError;

    /// An acquire timeout is overload, not a fault, and the HTTP binding
    /// recognises it by this exact detail. If the two ends ever spell it
    /// differently the broker answers 500 to a condition a client could
    /// have retried, so the constant is asserted rather than the words.
    #[test]
    fn a_pool_timeout_is_marked_as_overload() {
        let pd = super::db(sqlx::Error::PoolTimedOut).to_problem_details();
        assert_eq!(pd.detail, antares_model::error::DB_OVERLOADED);
        assert_eq!(pd.title, "InternalError");
        // and no other sqlx error borrows the mark
        assert_ne!(
            super::db(sqlx::Error::RowNotFound)
                .to_problem_details()
                .detail,
            antares_model::error::DB_OVERLOADED
        );
    }

    /// 5.5.6 InternalError: the RFC 7807 `detail` a client sees must be
    /// generic — driver internals (SQL text, row counts, connection
    /// strings) belong in the server log, never in the response body.
    #[test]
    fn db_error_detail_is_generic() {
        let pd = super::db(sqlx::Error::RowNotFound).to_problem_details();
        assert_eq!(pd.detail, "database error");
        assert_eq!(pd.title, "InternalError");
        assert!(
            !pd.detail.contains("no rows"),
            "sqlx internals leaked into the client-visible detail: {}",
            pd.detail
        );
        // a configuration error that is NOT one of ours stays generic too
        let pd = super::db(sqlx::Error::Configuration("boom".into())).to_problem_details();
        assert_eq!(pd.detail, "database error");
        assert!(!pd.detail.contains("boom"), "{}", pd.detail);
    }

    /// A spec error the store raised itself travels out through the driver
    /// error channel. Rebuilding it as a fixed variant forces every one of
    /// them to that variant's status — a 400 BadRequestData would reach the
    /// client as a 403.
    #[test]
    fn a_store_raised_spec_error_keeps_its_own_status() {
        for (err, kind, status) in [
            (NgsiError::BadRequestData("x".into()), "BadRequestData", 400),
            (NgsiError::TooManyResults("x".into()), "TooManyResults", 403),
            (NgsiError::AlreadyExists("x".into()), "AlreadyExists", 409),
        ] {
            let out = super::db(sqlx::Error::Configuration(Box::new(err)));
            assert_eq!(out.kind(), kind);
            assert_eq!(out.status(), status, "{kind} lost its status");
        }
    }
}

// The driver seam: `AnyStore` carries both driver interfaces, delegating to
// the inherent methods above. New backends implement the traits directly —
// this enum stays an implementation detail of the built-in backends, no
// longer the API surface.
/// The most Entities one history transaction appends (T-4085). Each append
/// holds `FOR KEY SHARE` on its Entity until the commit, which only a DELETE
/// of that Entity waits for (writes lock `FOR NO KEY UPDATE`, T-4669); a
/// 1000-member page in one transaction would hold such a delete for the whole
/// append (T-4123). Small requests, the HSL stream's, are one transaction.
// ponytail: a fixed chunk; a measured one if lock waits still show on dev.
#[cfg(feature = "postgres")]
const HISTORY_CHUNK: usize = 64;

#[async_trait::async_trait]
impl antares_store::CurrentStateDriver for AnyStore {
    async fn ping(&self) -> Result<(), NgsiError> {
        AnyStore::ping(self).await
    }
    async fn wedged(&self) -> bool {
        AnyStore::wedged(self).await
    }
    fn commit_queue(&self) -> Option<(usize, usize)> {
        AnyStore::commit_queue(self)
    }
    fn version_info(&self) -> Value {
        AnyStore::version_info(self)
    }
    async fn close(&self) {
        AnyStore::close(self).await;
    }
    fn set_change_hook(&self, h: super::ChangeHook) {
        AnyStore::set_change_hook(self, h);
    }
    fn set_outbox(&self, on: bool) {
        AnyStore::set_outbox(self, on);
    }
    async fn outbox_peek(&self, limit: i64) -> Result<Vec<(i64, String, Value)>, NgsiError> {
        AnyStore::outbox_peek(self, limit).await
    }
    async fn outbox_ack(&self, seqs: &[i64]) -> Result<u64, NgsiError> {
        AnyStore::outbox_ack(self, seqs).await
    }
    async fn outbox_retain(&self, tenant: &TenantId, seqs: &[i64]) -> Result<u64, NgsiError> {
        AnyStore::outbox_retain(self, tenant, seqs).await
    }
    async fn outbox_event(&self, seq: i64, tenant: &TenantId) -> Result<Option<Value>, NgsiError> {
        AnyStore::outbox_event(self, seq, tenant).await
    }
    async fn create(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        doc: Value,
    ) -> Result<bool, NgsiError> {
        AnyStore::create(self, tenant, kind, id, doc).await
    }
    async fn batch_create(
        &self,
        tenant: &TenantId,
        items: Vec<(String, Value)>,
    ) -> Result<Vec<bool>, NgsiError> {
        AnyStore::batch_create(self, tenant, items).await
    }
    async fn batch_delete(
        &self,
        tenant: &TenantId,
        ids: &[String],
    ) -> Result<Vec<bool>, NgsiError> {
        AnyStore::batch_delete(self, tenant, ids).await
    }
    async fn batch_upsert(
        &self,
        tenant: &TenantId,
        items: Vec<(String, Value)>,
    ) -> Result<Vec<bool>, NgsiError> {
        AnyStore::batch_upsert(self, tenant, items).await
    }
    async fn upsert(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        doc: Value,
    ) -> Result<bool, NgsiError> {
        AnyStore::upsert(self, tenant, kind, id, doc).await
    }
    async fn get(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
    ) -> Result<Option<Value>, NgsiError> {
        AnyStore::get(self, tenant, kind, id).await
    }
    async fn get_entities(
        &self,
        tenant: &TenantId,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, Value>, NgsiError> {
        AnyStore::get_entities(self, tenant, ids).await
    }
    async fn delete(&self, tenant: &TenantId, kind: Kind, id: &str) -> Result<bool, NgsiError> {
        AnyStore::delete(self, tenant, kind, id).await
    }
    async fn delete_entity_if(
        &self,
        tenant: &TenantId,
        id: &str,
        keep: &(dyn for<'v> Fn(&'v Value) -> bool + Sync),
    ) -> Result<bool, NgsiError> {
        AnyStore::delete_entity_if(self, tenant, id, keep).await
    }
    async fn list(&self, tenant: &TenantId, kind: Kind) -> Result<Vec<Value>, NgsiError> {
        AnyStore::list(self, tenant, kind).await
    }
    async fn list_digests(
        &self,
        tenant: &TenantId,
        kind: Kind,
    ) -> Result<Vec<(String, u64)>, NgsiError> {
        AnyStore::list_digests(self, tenant, kind).await
    }
    async fn list_page(
        &self,
        tenant: &TenantId,
        kind: Kind,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Value>, NgsiError> {
        AnyStore::list_page(self, tenant, kind, after, limit).await
    }
    async fn list_slice(
        &self,
        tenant: &TenantId,
        kind: Kind,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<Value>, usize), NgsiError> {
        AnyStore::list_slice(self, tenant, kind, offset, limit).await
    }
    async fn matching_registrations(
        &self,
        tenant: &TenantId,
        ids: Option<&[String]>,
        types: Option<&[String]>,
    ) -> Result<Vec<Value>, NgsiError> {
        AnyStore::matching_registrations(self, tenant, ids, types).await
    }
    async fn query_entities(
        &self,
        tenant: &TenantId,
        f: &crate::store::filter::EntityFilter<'_>,
    ) -> Result<crate::store::filter::QueryOutcome, NgsiError> {
        AnyStore::query_entities(self, tenant, f).await
    }
    async fn query_candidates(
        &self,
        tenant: &TenantId,
        f: &crate::store::filter::EntityFilter<'_>,
    ) -> Result<crate::store::filter::QueryOutcome, NgsiError> {
        AnyStore::query_candidates(self, tenant, f).await
    }
    async fn query_entities_text(
        &self,
        tenant: &TenantId,
        f: &crate::store::filter::EntityFilter<'_>,
    ) -> Result<crate::store::filter::QueryOutcome<Box<serde_json::value::RawValue>>, NgsiError>
    {
        AnyStore::query_entities_text(self, tenant, f).await
    }
    async fn entity_type_names(&self, tenant: &TenantId) -> Result<Option<Vec<String>>, NgsiError> {
        AnyStore::entity_type_names(self, tenant).await.map(Some)
    }
    async fn entity_attr_pairs(
        &self,
        tenant: &TenantId,
    ) -> Result<Option<Vec<(String, String)>>, NgsiError> {
        AnyStore::entity_attr_pairs(self, tenant).await.map(Some)
    }
    async fn entity_types_beside(
        &self,
        tenant: &TenantId,
        any_of: &[String],
    ) -> Result<Option<Vec<String>>, NgsiError> {
        AnyStore::entity_types_beside(self, tenant, any_of)
            .await
            .map(Some)
    }
    async fn entity_id_segments(
        &self,
        tenant: &TenantId,
        lead: &str,
        sep: char,
        max: usize,
    ) -> Result<Option<Vec<String>>, NgsiError> {
        AnyStore::entity_id_segments(self, tenant, lead, sep, max).await
    }
    async fn mutate_boxed<'a>(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        f: antares_store::MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        AnyStore::mutate(self, tenant, kind, id, f).await
    }
    async fn batch_mutate_boxed<'a>(
        &self,
        tenant: &TenantId,
        ids: &[String],
        mut f: antares_store::BatchMutateFn<'a>,
    ) -> Result<Vec<Option<Result<(), ()>>>, NgsiError> {
        AnyStore::batch_mutate(self, tenant, ids, |id, v| f(id, v)).await
    }
    /// The Postgres arm answers this in ONE statement (see
    /// `pg::doc::record_delivery`); every other arm runs the shared rule as a
    /// `mutate`, which is what the one statement is a translation of.
    async fn record_delivery(
        &self,
        tenant: &TenantId,
        kind: Kind,
        id: &str,
        now: &str,
    ) -> Result<Option<antares_store::Delivery>, NgsiError> {
        // `doc_kind` exists only with the Postgres arm, so its call lives
        // inside the gate with it: outside the feature this function is the
        // shared rule and nothing else.
        #[cfg(feature = "postgres")]
        {
            if let (AnyStore::Pg(p), Ok(dk)) = (self, doc_kind(kind)) {
                let found = p
                    .docs
                    .record_delivery(tenant, dk, id, now)
                    .await
                    .map_err(db)?;
                return Ok(
                    found.map(|(doc, prev_success)| antares_store::Delivery { doc, prev_success })
                );
            }
        }
        antares_store::record_delivery_via_mutate(self, tenant, kind, id, now).await
    }
    /// The Postgres arm books every attempt in ONE statement (see
    /// `pg::doc::record_deliveries`); every other arm books them one by one.
    async fn record_deliveries(
        &self,
        tenant: &TenantId,
        kind: Kind,
        attempts: &[(&str, &str, i64)],
    ) -> Result<Vec<Option<antares_store::Delivery>>, NgsiError> {
        #[cfg(feature = "postgres")]
        {
            if let (AnyStore::Pg(p), Ok(dk)) = (self, doc_kind(kind)) {
                let mut found = p
                    .docs
                    .record_deliveries(tenant, dk, attempts)
                    .await
                    .map_err(db)?;
                return Ok(attempts
                    .iter()
                    .map(|(id, _, _)| {
                        found
                            .remove(*id)
                            .map(|(doc, prev_success)| antares_store::Delivery {
                                doc,
                                prev_success,
                            })
                    })
                    .collect());
            }
        }
        let mut out = Vec::with_capacity(attempts.len());
        for (id, now, times) in attempts {
            let mut booked = None;
            for _ in 0..*times {
                booked =
                    antares_store::record_delivery_via_mutate(self, tenant, kind, id, now).await?;
            }
            out.push(booked);
        }
        Ok(out)
    }
    async fn sweep_expired(&self) -> usize {
        AnyStore::sweep_expired(self)
    }
    async fn retain_current(
        &self,
        tenant: &TenantId,
        type_iri: &str,
        before: &str,
        batch: usize,
        dry_run: bool,
    ) -> Result<u64, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.retain_current(tenant, type_iri, before, batch, dry_run)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => crate::store::pg::maintenance::retain_current(
                p.docs.pool(),
                tenant.as_str(),
                type_iri,
                before,
                batch,
                dry_run,
            )
            .await
            .map_err(db),
        }
    }
    async fn tenant_exists(&self, tenant: &TenantId) -> Result<bool, NgsiError> {
        AnyStore::tenant_exists(self, tenant).await
    }
    async fn subscription_tenants(&self) -> Result<Vec<String>, NgsiError> {
        AnyStore::subscription_tenants(self).await
    }
    async fn tenant_ids(&self) -> Result<Vec<String>, NgsiError> {
        AnyStore::tenant_ids(self).await
    }
    async fn history_overrides(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, Value)>, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.history_overrides(limit)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                crate::store::pg::maintenance::history_overrides(p.docs.pool(), limit)
                    .await
                    .map_err(db)
            }
        }
    }
    async fn tenant_stats_one(
        &self,
        tenant: &TenantId,
    ) -> Result<Option<antares_store::TenantStats>, NgsiError> {
        AnyStore::tenant_stats_one(self, tenant).await
    }
    async fn tenant_bytes(
        &self,
        tenant: &TenantId,
    ) -> Result<antares_store::TenantBytes, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.tenant_bytes(tenant)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                async {
                    let mut tx = tenant_tx(p, tenant).await?;
                    // a scan of the tenant: bounded, a slow one answers no size
                    sqlx::query(SIZE_SCAN_TIMEOUT)
                        .execute(&mut *tx)
                        .await
                        .map_err(db)?;
                    // the stored row, TOAST included, by the first type the
                    // write recorded (T-2890)
                    let rows: Vec<(String, i64)> = sqlx::query_as(
                        "SELECT coalesce(types[1], ''), sum(pg_column_size(e.*))::bigint
                           FROM entities e WHERE tenant_id = $1 GROUP BY 1",
                    )
                    .bind(tenant.as_str())
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(db)?;
                    let mut out = antares_store::TenantBytes::default();
                    for (ty, n) in rows {
                        let n = n.max(0) as u64;
                        out.entities = out.entities.saturating_add(n);
                        out.by_type.insert(ty, n);
                    }
                    Ok(out)
                }
                .await
            }
        }
    }
    async fn tenant_bytes_estimate(
        &self,
        tenant: &TenantId,
        entities: u64,
    ) -> Result<antares_store::TenantBytes, NgsiError> {
        // only the Postgres arm sizes from the count; the memory store is exact
        #[cfg(not(feature = "postgres"))]
        let _ = entities;
        match self {
            // exact, and as fast as a guess would be
            AnyStore::Mem(s) => Ok(s.tenant_bytes(tenant)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                let (tuples, size): (f64, i64) = sqlx::query_as(ENTITY_TABLE_STATS)
                    .fetch_one(p.docs.pool())
                    .await
                    .map_err(db)?;
                // the split by first type, bounded: under forced RLS no
                // index-only count, so a big tenant's passes the bound and
                // the size comes from `entities` alone (T-6204)
                let counts = async {
                    let mut tx = tenant_tx(p, tenant).await?;
                    sqlx::query(SIZE_SCAN_TIMEOUT)
                        .execute(&mut *tx)
                        .await
                        .map_err(db)?;
                    sqlx::query_as::<_, (String, i64)>(
                        "SELECT coalesce(types[1], ''), count(*)::bigint
                           FROM entities WHERE tenant_id = $1 GROUP BY 1",
                    )
                    .bind(tenant.as_str())
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(db)
                }
                .await
                .inspect_err(|e| {
                    tracing::warn!(tenant = tenant.as_str(), error = %e,
                        "tenant size by type not counted, sizing the whole tenant");
                })
                .ok();
                estimate_entities(tuples, size, counts.as_deref(), entities).ok_or_else(|| {
                    NgsiError::InternalError("the entities table was never analyzed".into())
                })
            }
        }
    }
    async fn purge_tenant(&self, tenant: &TenantId) -> Result<bool, NgsiError> {
        AnyStore::purge_tenant(self, tenant).await
    }
    async fn context_put(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
        doc: Value,
    ) -> Result<(), NgsiError> {
        AnyStore::context_put(self, tenant, id, doc).await
    }
    async fn context_get(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
    ) -> Result<Option<Value>, NgsiError> {
        AnyStore::context_get(self, tenant, id).await
    }
    async fn context_mutate<'a>(
        &self,
        tenant: Option<&TenantId>,
        id: &str,
        f: antares_store::MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        AnyStore::context_mutate(self, tenant, id, f).await
    }
    async fn context_delete(&self, tenant: Option<&TenantId>, id: &str) -> Result<bool, NgsiError> {
        AnyStore::context_delete(self, tenant, id).await
    }
    async fn context_list_meta(&self, tenant: Option<&TenantId>) -> Result<Vec<Value>, NgsiError> {
        AnyStore::context_list_meta(self, tenant).await
    }
}

#[async_trait::async_trait]
impl antares_store::TemporalDriver for AnyStore {
    async fn close(&self) {
        AnyStore::close(self).await;
    }
    fn set_short_history(&self, short: antares_store::ShortHistory) {
        match self {
            // ponytail: memory has no WAL to save
            AnyStore::Mem(_) => drop(short),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.temporal.set_short_history(short),
        }
    }
    async fn versions(
        &self,
        tenant: &TenantId,
        entity_id: &str,
    ) -> Result<serde_json::Map<String, Value>, NgsiError> {
        match self {
            // ponytail: memory keeps no versions or provenance
            AnyStore::Mem(_) => {
                let _ = (tenant, entity_id);
                Ok(serde_json::Map::new())
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p.temporal.versions(tenant, entity_id).await.map_err(db),
        }
    }
    async fn erase(
        &self,
        tenant: &TenantId,
        entity_id: &str,
        attrs: Option<&[String]>,
        before: Option<&str>,
    ) -> Result<u64, NgsiError> {
        match self {
            AnyStore::Mem(_) => {
                let instant = |t: &str| {
                    chrono::DateTime::parse_from_rfc3339(
                        &antares_store::filter::canonical_datetime(t),
                    )
                    .ok()
                };
                // an instance goes when it was observed at or before the cut,
                // timed as the Postgres arm's `observed_at` column times it
                // (F-R2ANT3-006: one recorded without observedAt survived the
                // start-up re-apply here); a cut that is no DateTime erases
                // nothing
                let cut = before.map(instant);
                let goes = |i: &Value| match cut {
                    None => true,
                    Some(cut) => {
                        let at = instant(
                            crate::store::mem::instance_instant(i)
                                .unwrap_or(crate::store::mem::EPOCH),
                        );
                        cut.zip(at).is_some_and(|(cut, at)| at <= cut)
                    }
                };
                let erased = AnyStore::mutate(self, tenant, Kind::Temporal, entity_id, |doc| {
                    let o = doc.as_object_mut().ok_or(())?;
                    let mut n = 0u64;
                    for (k, v) in o.iter_mut() {
                        let named = attrs.is_none_or(|a| a.contains(k));
                        let Some(list) = v.as_array_mut().filter(|_| named) else {
                            continue;
                        };
                        if !list.iter().all(Value::is_object) {
                            continue; // the Entity's own `type` list
                        }
                        let was = list.len();
                        list.retain(|i| !goes(i));
                        let gone = was.saturating_sub(list.len());
                        n = n.saturating_add(u64::try_from(gone).unwrap_or(u64::MAX));
                    }
                    let emptied = o
                        .values()
                        .filter_map(Value::as_array)
                        .all(|l| l.is_empty() || !l.iter().all(Value::is_object));
                    Ok::<_, ()>((n, emptied))
                })
                .await?;
                let Some(Ok((n, emptied))) = erased else {
                    return Ok(0);
                };
                if attrs.is_none() && emptied {
                    AnyStore::delete(self, tenant, Kind::Temporal, entity_id).await?;
                }
                Ok(n)
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => p
                .temporal
                .erase(tenant, entity_id, attrs, before)
                .await
                .map_err(db),
        }
    }
    async fn retain_history(
        &self,
        tenant: &TenantId,
        scope: antares_store::HistoryScope<'_>,
        before: &str,
        batch: usize,
        dry_run: bool,
    ) -> Result<u64, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.retain_history(tenant, scope, before, batch, dry_run)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => crate::store::pg::maintenance::retain_history(
                p.docs.pool(),
                tenant.as_str(),
                scope,
                before,
                batch,
                dry_run,
            )
            .await
            .map_err(db),
        }
    }
    async fn retain_audit(&self, before: &str, batch: usize) -> Result<u64, NgsiError> {
        match self {
            // memory keeps no provenance, so it holds no audit rows
            AnyStore::Mem(_) => {
                let _ = (before, batch);
                Ok(0)
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                crate::store::pg::maintenance::retain_audit(p.docs.pool(), before, batch)
                    .await
                    .map_err(db)
            }
        }
    }
    async fn drop_retained_history(
        &self,
        floors: &[(String, String)],
    ) -> Result<Vec<String>, NgsiError> {
        match self {
            // memory keeps no partitions
            AnyStore::Mem(_) => {
                let _ = floors;
                Ok(Vec::new())
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                crate::store::pg::maintenance::drop_retained_partitions(p.docs.pool(), floors)
                    .await
                    .map_err(db)
            }
        }
    }
    fn version_info(&self) -> Value {
        AnyStore::version_info(self)
    }
    async fn attr_instance_count(&self, tenant: &TenantId) -> Result<u64, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.attr_instance_count(tenant)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                async {
                    let mut tx = tenant_tx(p, tenant).await?;
                    count_instances(&mut tx, tenant).await
                }
                .await
            }
        }
    }
    async fn attr_instance_count_within(
        &self,
        tenant: &TenantId,
        bound: std::time::Duration,
    ) -> Result<u64, NgsiError> {
        match self {
            AnyStore::Mem(s) => {
                // the memory store counts at once: no bound to apply
                let _ = bound;
                Ok(s.attr_instance_count(tenant))
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                async {
                    let mut tx = tenant_tx(p, tenant).await?;
                    // a scan of the tenant's history: a slow one answers no count
                    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
                        .bind(format!("{}ms", bound.as_millis().max(1)))
                        .execute(&mut *tx)
                        .await
                        .map_err(db)?;
                    count_instances(&mut tx, tenant).await
                }
                .await
            }
        }
    }
    async fn attr_instance_usage(
        &self,
        tenant: &TenantId,
        bound: std::time::Duration,
    ) -> Result<(u64, u64), NgsiError> {
        match self {
            AnyStore::Mem(s) => {
                // the memory store counts at once: no bound to apply
                let _ = bound;
                Ok((s.attr_instance_count(tenant), s.attr_instance_bytes(tenant)))
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                async {
                    let mut tx = tenant_tx(p, tenant).await?;
                    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
                        .bind(format!("{}ms", bound.as_millis().max(1)))
                        .execute(&mut *tx)
                        .await
                        .map_err(db)?;
                    // one scan per history parent for both numbers
                    let (n, b): (i64, i64) = sqlx::query_as(
                        "SELECT (a.n + r.n)::bigint, (a.b + r.b)::bigint
                           FROM (SELECT count(*) AS n, coalesce(sum(pg_column_size(x.*)), 0) AS b
                                   FROM attr_instances x WHERE tenant_id = $1) a,
                                (SELECT count(*) AS n, coalesce(sum(pg_column_size(x.*)), 0) AS b
                                   FROM attr_rows x WHERE tenant_id = $1) r",
                    )
                    .bind(tenant.as_str())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db)?;
                    Ok((n.max(0) as u64, b.max(0) as u64))
                }
                .await
            }
        }
    }
    async fn attr_instance_estimate(&self, tenant: &TenantId) -> Result<(u64, u64), NgsiError> {
        match self {
            // exact, and as fast as a guess would be
            AnyStore::Mem(s) => Ok((s.attr_instance_count(tenant), s.attr_instance_bytes(tenant))),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                let rows: Vec<HistoryStats> = sqlx::query_as(HISTORY_STATS)
                    .fetch_all(p.docs.pool())
                    .await
                    .map_err(db)?;
                Ok(estimate_history(tenant.as_str(), &rows))
            }
        }
    }
    async fn attr_instance_bytes(&self, tenant: &TenantId) -> Result<u64, NgsiError> {
        match self {
            AnyStore::Mem(s) => Ok(s.attr_instance_bytes(tenant)),
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                async {
                    let mut tx = tenant_tx(p, tenant).await?;
                    sqlx::query(SIZE_SCAN_TIMEOUT)
                        .execute(&mut *tx)
                        .await
                        .map_err(db)?;
                    let n: i64 = sqlx::query_scalar(
                        "SELECT (SELECT coalesce(sum(pg_column_size(a.*)), 0)
                                   FROM attr_instances a WHERE tenant_id = $1)::bigint
                              + (SELECT coalesce(sum(pg_column_size(r.*)), 0)
                                   FROM attr_rows r WHERE tenant_id = $1)::bigint",
                    )
                    .bind(tenant.as_str())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db)?;
                    Ok(n.max(0) as u64)
                }
                .await
            }
        }
    }
    async fn purge_tenant(&self, tenant: &TenantId) -> Result<(), NgsiError> {
        match self {
            AnyStore::Mem(s) => {
                s.purge_kinds(tenant, &[Kind::Temporal]);
                Ok(())
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(p) => {
                async {
                    let mut tx = tenant_tx(p, tenant).await?;
                    // whole statements, not a spliced table name: the
                    // attr_instances tenant-predicate check reads literals
                    for sql in [
                        "DELETE FROM attr_instances WHERE tenant_id = $1",
                        "DELETE FROM attr_rows WHERE tenant_id = $1",
                        "DELETE FROM iri_ids WHERE tenant_id = $1",
                        "DELETE FROM attr_versions WHERE tenant_id = $1",
                        "DELETE FROM temporal_entities WHERE tenant_id = $1",
                    ] {
                        sqlx::query(sql)
                            .bind(tenant.as_str())
                            .execute(&mut *tx)
                            .await
                            .map_err(db)?;
                    }
                    tx.commit().await.map_err(db)?;
                    p.temporal.forget_tenant(tenant);
                    Ok(())
                }
                .await
            }
        }
    }
    async fn temporal_append(
        &self,
        tenant: &TenantId,
        id: &str,
        shell: &Value,
        additions: &Value,
    ) -> Result<(), NgsiError> {
        AnyStore::temporal_append(self, tenant, id, shell, additions).await
    }
    /// On Postgres a drained request is one transaction per
    /// `HISTORY_CHUNK` Entities (T-4085); a scope change, and the memory
    /// arm, take the default's append per Entity.
    async fn event_list(&self, evs: &[antares_store::TemporalEvent]) -> Result<(), NgsiError> {
        #[cfg(feature = "postgres")]
        if let (AnyStore::Pg(p), Some(appends)) = (self, antares_store::fold_appends(evs)) {
            // One chunk that fails (a lock timeout) loses its own Entities'
            // history, never the rest of the request's (T-4669).
            let mut first = None;
            for chunk in appends.chunks(HISTORY_CHUNK) {
                if let Err(e) = p.temporal.append_many(chunk).await {
                    first.get_or_insert(e);
                }
            }
            return first.map_or(Ok(()), |e| Err(db(e)));
        }
        antares_store::drain_events(self, evs).await
    }
    /// Only the Postgres arm pages in SQL, and only an exact prefilter
    /// makes that page the evaluator's page; the memory arm answers
    /// `paged: false` and the caller pages.
    fn q_pushdown_exact(
        &self,
        q: &antares_ql::QNode,
        range: Option<&crate::store::filter::InstanceRange<'_>>,
        expand: &dyn Fn(&str) -> String,
    ) -> bool {
        match self {
            // the memory arm pages nothing, so the filter is not its business
            AnyStore::Mem(_) => {
                let _ = (q, range, expand);
                false
            }
            #[cfg(feature = "postgres")]
            AnyStore::Pg(_) => crate::compile::qprefilter::prefilter_exact(q, range, expand),
        }
    }
    async fn query_temporal(
        &self,
        tenant: &TenantId,
        f: &crate::store::filter::TemporalFilter<'_>,
    ) -> Result<crate::store::filter::TemporalOutcome, NgsiError> {
        AnyStore::query_temporal(self, tenant, f).await
    }
    async fn get_temporal(
        &self,
        tenant: &TenantId,
        id: &str,
        f: &crate::store::filter::TemporalFilter<'_>,
    ) -> Result<Option<Value>, NgsiError> {
        AnyStore::get_temporal(self, tenant, id, f).await
    }
    async fn get(&self, tenant: &TenantId, id: &str) -> Result<Option<Value>, NgsiError> {
        AnyStore::get(self, tenant, Kind::Temporal, id).await
    }
    async fn create(&self, tenant: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError> {
        AnyStore::create(self, tenant, Kind::Temporal, id, doc).await
    }
    async fn upsert(&self, tenant: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError> {
        AnyStore::upsert(self, tenant, Kind::Temporal, id, doc).await
    }
    async fn delete(&self, tenant: &TenantId, id: &str) -> Result<bool, NgsiError> {
        AnyStore::delete(self, tenant, Kind::Temporal, id).await
    }
    async fn list(&self, tenant: &TenantId) -> Result<Vec<Value>, NgsiError> {
        AnyStore::list(self, tenant, Kind::Temporal).await
    }
    async fn mutate_boxed<'a>(
        &self,
        tenant: &TenantId,
        id: &str,
        f: antares_store::MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        // Postgres takes the provenance into its column in decompose; the
        // memory store keeps none, so neither arm answers it as data.
        let keeps_none = matches!(self, AnyStore::Mem(_));
        AnyStore::mutate(self, tenant, Kind::Temporal, id, move |doc: &mut Value| {
            let done = f(doc);
            if keeps_none {
                antares_store::strip_provenance(doc);
            }
            done
        })
        .await
    }
}

#[cfg(test)]
mod temporal_only_tests {
    use super::*;
    use antares_model::TenantId;

    async fn append(store: &AnyStore) -> Option<Value> {
        let t = TenantId::new("combo").expect("tenant");
        let shell = serde_json::json!({"id": "urn:a", "type": ["T"]});
        let adds = serde_json::json!({"speed": [{"instanceId": "urn:i:1", "value": 1}]});
        store
            .temporal_append(&t, "urn:a", &shell, &adds)
            .await
            .expect("append");
        store.get(&t, Kind::Temporal, "urn:a").await.expect("get")
    }

    /// A shared instance records only for an entity it holds (5.6.6 delete
    /// overlap); the temporal half of a mixed deployment records
    /// unconditionally, since the entities live elsewhere.
    #[tokio::test]
    async fn a_temporal_only_instance_records_without_the_entity() {
        assert!(append(&AnyStore::Mem(Store::default())).await.is_none());
        let doc = append(&AnyStore::Mem(Store::default()).temporal_only())
            .await
            .expect("recorded");
        assert_eq!(doc["speed"][0]["value"], 1);
    }

    /// F-R2ANT3-006: an erasure cut times an instance without observedAt
    /// by its other stamps, as the Postgres arm's `observed_at` does, and
    /// one with no stamp at all by the epoch.
    #[tokio::test]
    async fn an_erasure_cut_times_an_instance_without_observed_at() {
        use antares_store::TemporalDriver;
        let store = AnyStore::Mem(Store::default()).temporal_only();
        let t = TenantId::new("erase").expect("tenant");
        let shell = serde_json::json!({"id": "urn:a", "type": ["T"]});
        let adds = serde_json::json!({"speed": [
            {"instanceId": "urn:i:1", "value": 1, "createdAt": "2026-01-01T00:00:00Z"},
            {"instanceId": "urn:i:2", "value": 2, "modifiedAt": "2026-03-01T00:00:00Z"},
            {"instanceId": "urn:i:3", "value": 3},
            {"instanceId": "urn:i:4", "value": 4, "observedAt": "2026-03-01T00:00:00Z",
             "createdAt": "2026-01-01T00:00:00Z"},
        ]});
        store
            .temporal_append(&t, "urn:a", &shell, &adds)
            .await
            .expect("append");
        let gone = store
            .erase(&t, "urn:a", None, Some("2026-02-01T00:00:00Z"))
            .await
            .expect("erase");
        assert_eq!(gone, 2, "the createdAt one and the unstamped one");
        let doc = store.get(&t, Kind::Temporal, "urn:a").await.expect("get");
        let left: Vec<_> = doc
            .as_ref()
            .and_then(|d| d["speed"].as_array())
            .map_or(Vec::new(), |l| {
                l.iter().map(|i| i["value"].clone()).collect()
            });
        assert_eq!(
            left,
            [serde_json::json!(2), serde_json::json!(4)],
            "{doc:?}"
        );
    }
}

#[cfg(all(test, feature = "postgres"))]
mod estimate_tests {
    use super::{estimate_entities, estimate_history, HistoryStats};

    fn part(tuples: f64, bytes: i64, vals: &[&str], freqs: &[f64], distinct: f64) -> HistoryStats {
        (
            tuples,
            bytes,
            Some(vals.iter().map(|v| (*v).to_owned()).collect()),
            Some(freqs.to_vec()),
            Some(distinct),
        )
    }

    /// F-R2ANT3-003: replace None, create false (lost the race), replace
    /// None again (a delete took the row): the pair runs again, and a pair
    /// that never takes the row is refused, never answered as stored.
    #[test]
    fn an_upsert_racing_create_and_delete_runs_again_and_never_fakes_a_write() {
        use super::{upsert_raced, UPSERT_ROUNDS};
        // each step answers its script in turn, then false
        fn rounds(first: &[bool], second: &[bool]) -> Result<bool, antares_model::NgsiError> {
            let (mut f, mut s) = (first.iter().copied(), second.iter().copied());
            upsert_rounds!(f.next().unwrap_or(false), s.next().unwrap_or(false))
        }
        // replace, create, replace, create: the second create takes it
        assert_eq!(rounds(&[false, false], &[false, true]).ok(), Some(false));
        // the row came back between the steps: the second replace takes it
        assert_eq!(rounds(&[false, true], &[false]).ok(), Some(true));
        // never taken: refused, never answered as stored
        let got = rounds(&[], &[]);
        assert!(
            matches!(got, Err(antares_model::NgsiError::InternalError(_))),
            "{got:?}"
        );
    }

    #[test]
    fn a_listed_tenant_takes_its_frequency_of_each_partition() {
        let rows = [
            part(1000.0, 80_000, &["helsinki", "praha"], &[0.75, 0.25], 2.0),
            part(400.0, 40_000, &["praha"], &[1.0], 1.0),
        ];
        assert_eq!(estimate_history("helsinki", &rows), (750, 60_000));
        assert_eq!(estimate_history("praha", &rows), (650, 60_000));
    }

    #[test]
    fn an_unlisted_tenant_shares_the_rest_with_the_other_unlisted_ones() {
        // 0.4 of the rows over 4 unlisted tenants: 0.1 each
        let rows = [part(1000.0, 10_000, &["big"], &[0.6], 5.0)];
        assert_eq!(estimate_history("small", &rows), (100, 1_000));
        // a negative n_distinct is a fraction of the rows: 0.005 * 1000 = 5
        let frac = [part(1000.0, 10_000, &["big"], &[0.6], -0.005)];
        assert_eq!(estimate_history("small", &frac), (100, 1_000));
    }

    #[test]
    fn a_tenant_absent_from_a_fully_listed_or_unanalyzed_table_counts_nothing() {
        let listed = part(1000.0, 10_000, &["a", "b"], &[0.5, 0.5], 2.0);
        let never: HistoryStats = (5000.0, 99_000, None, None, None);
        assert_eq!(estimate_history("c", &[listed, never]), (0, 0));
    }

    /// T-6159: each of the tenant's rows weighs the table's average row, by
    /// first type, so the types sum to the whole; an unanalyzed table gives
    /// no guess rather than zero bytes.
    #[test]
    fn a_tenants_entities_weigh_the_tables_average_row_by_type() {
        let counts = [("Room".to_owned(), 3), ("Bridge".to_owned(), 1)];
        let b = estimate_entities(1000.0, 2_000_000, Some(&counts), 4).expect("analyzed");
        assert_eq!(b.by_type["Room"], 6_000);
        assert_eq!(b.by_type["Bridge"], 2_000);
        assert_eq!(b.entities, 8_000);
        assert!(estimate_entities(0.0, 2_000_000, Some(&counts), 4).is_none());
    }

    /// T-6204: a split by type that passed its bound still sizes the tenant
    /// from its Entity count, with no types.
    #[test]
    fn a_tenant_without_a_split_by_type_is_sized_from_its_entity_count() {
        let b = estimate_entities(1000.0, 2_000_000, None, 791_784).expect("analyzed");
        assert_eq!(b.entities, 1_583_568_000);
        assert!(b.by_type.is_empty());
        assert!(estimate_entities(0.0, 2_000_000, None, 5).is_none());
    }
}
