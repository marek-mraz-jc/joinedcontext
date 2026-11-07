// SPDX-License-Identifier: EUPL-1.2
//! Input bounds wall: every request-shaped resource has
//! a configured cap, rejected with the spec-shaped error. One middleware
//! enforces the transport-level caps (URI length 414, body size 413, JSON
//! nesting 400) — size and depth are checked BEFORE any parse. The
//! per-feature caps (batch count, joinLevel, @context fetch
//! count, q= complexity, result ceiling) live at their parse points.
//! Rejections are counted and exported via /q/health.

use axum::body::{Body, Bytes};
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::atomic::{AtomicU64, Ordering};
// Clock rule: std Instant panics on wasm32 (T-4937).
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant as Clock;
#[cfg(target_arch = "wasm32")]
use web_time::Instant as Clock;

/// Hard caps (v1: compile-time constants — a config file is a later knob;
/// every value is spec-shaped on rejection).
/// → bare 413 (6.3.4). Deployment knob (ANTARES_MAX_BODY_BYTES): the spec
/// names no ceiling; 4 MiB is the DoS bound, raised where a trusted
/// producer legitimately sends bigger batches. Read once at first use.
pub static MAX_BODY_BYTES: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    std::env::var("ANTARES_MAX_BODY_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(4 * 1024 * 1024)
});
pub const MAX_URI_BYTES: usize = 8 * 1024; // → bare 414
pub const MAX_JSON_DEPTH: usize = 64; // → 400 BadRequestData
/// → 403 TooManyResults (5.5.6) on the NGSI-LD API, 413 on a surface
/// (`antares_ops::Refusal::Limit`). The most bytes of one in-process answer
/// a handler buffers before reading it (a temporal candidate set, a
/// snapshot fill page, a surface's query): the page size bounds the Entity
/// count, not the bytes, and every concurrent request holds its buffer.
/// `AppState::max_answer_bytes` carries it, so a test can lower it.
pub const MAX_ANSWER_BYTES: usize = 64 * 1024 * 1024;
/// The fixed detail of an answer past [`MAX_ANSWER_BYTES`].
pub const ANSWER_TOO_LARGE: &str = "the answer is larger than the broker buffers for one request";
/// The fixed detail of a 503 for a buffered answer the [`AnswerPool`] has no
/// room for (T-5895): the request is fine, the broker is holding as many
/// answer bytes as its memory budget allows. It is the marker the HTTP
/// binding maps to 503 + `Retry-After`, as it does for the database pool.
pub use antares_model::error::MEMORY_OVERLOADED;
/// `Retry-After` of a 503 for [`MEMORY_OVERLOADED`]: the pool frees as the
/// concurrent buffered answers finish, which takes well under a second.
pub const ANSWER_POOL_RETRY_SECONDS: u64 = 1;
/// How long a buffered read waits for room in a full [`AnswerPool`] before
/// it is refused with [`MEMORY_OVERLOADED`] (T-5898). The answers ahead of it
/// free their share as soon as they are written out, well under a second
/// each, so a brief burst of large reads queues instead of failing; a client
/// still sees a 503 long before its own timeout. One deadline covers the
/// whole read, however many frames wait.
pub const ANSWER_POOL_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// The process-wide memory budget (`ANTARES_MEMORY_BUDGET`, T-5895) and its
/// split over every long-lived cache and the buffered-answer pool. Each share
/// is `parts` / 200 of the total, so the default 200 MiB gives each the MiB
/// its parts say:
///
/// | share | parts | at 200 MiB |
/// |---|---|---|
/// | merged @contexts (`antares_jsonld`) | 44 | 44 MiB |
/// | fetched @context documents (`antares_jsonld`) | 8 | 8 MiB |
/// | OData models (`antares_odata`) | 16 | 16 MiB |
/// | LinkML models (`linkml_model`) | 8 | 8 MiB |
/// | regex programs (`antares_ql`) | 16 | 16 MiB |
/// | rendered LinkML schemas (`antares_linkml`) | 8 | 8 MiB |
/// | ImplicitlyCreated @contexts per Tenant | 8 | 8 MiB |
/// | buffered answers ([`AnswerPool`]) | 64 | 64 MiB |
///
/// The 28 parts left are the process itself: code, runtime, connection
/// buffers and the stores' own state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryBudget {
    /// Bytes the whole process plans for.
    pub total: u64,
}

impl Default for MemoryBudget {
    fn default() -> Self {
        Self {
            total: Self::DEFAULT,
        }
    }
}

impl MemoryBudget {
    /// The budget when `ANTARES_MEMORY_BUDGET` is unset.
    pub const DEFAULT: u64 = 200 << 20;
    /// The smallest budget accepted: below it the answer pool is smaller
    /// than one ordinary page of 1000 Entities.
    pub const MIN: u64 = 64 << 20;

    /// Reads `ANTARES_MEMORY_BUDGET`: bytes, or a number with `Ki`, `Mi` or
    /// `Gi` (`512Mi`).
    ///
    /// # Errors
    /// A message naming the key and an example, for a value that is not a
    /// size or is below [`Self::MIN`].
    pub fn parse(v: &str) -> Result<Self, String> {
        let v = v.trim();
        let (digits, unit) = match v.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
            Some((i, _)) => v.split_at(i),
            None => (v, ""),
        };
        let shift = match unit {
            "" => 0,
            "Ki" => 10,
            "Mi" => 20,
            "Gi" => 30,
            _ => {
                return Err(format!(
                    "ANTARES_MEMORY_BUDGET={v}: a size in bytes or with Ki, Mi or Gi, e.g. 200Mi"
                ))
            }
        };
        let total = digits
            .parse::<u64>()
            .ok()
            .and_then(|n| n.checked_mul(1 << shift))
            .ok_or_else(|| {
                format!(
                    "ANTARES_MEMORY_BUDGET={v}: a size in bytes or with Ki, Mi or Gi, e.g. 200Mi"
                )
            })?;
        if total < Self::MIN {
            return Err(format!(
                "ANTARES_MEMORY_BUDGET={v} is below the 64Mi the broker needs, e.g. 200Mi"
            ));
        }
        Ok(Self { total })
    }

    fn parts(self, n: u64) -> u64 {
        (self.total / 200).saturating_mul(n)
    }
    /// Heap bytes of the merged-@context cache.
    #[must_use]
    pub fn merged_contexts(self) -> u64 {
        self.parts(44)
    }
    /// Bytes of the fetched-@context document cache.
    #[must_use]
    pub fn fetched_contexts(self) -> u64 {
        self.parts(8)
    }
    /// Bytes of built OData models.
    #[must_use]
    pub fn odata_models(self) -> u64 {
        self.parts(16)
    }
    /// Bytes of LinkML schema source behind the model cache.
    #[must_use]
    pub fn linkml_models(self) -> u64 {
        self.parts(8)
    }
    /// Charged bytes of retained regex programs.
    #[must_use]
    pub fn regex_programs(self) -> u64 {
        self.parts(16)
    }
    /// Bytes of rendered LinkML schemas.
    #[must_use]
    pub fn linkml_renders(self) -> u64 {
        self.parts(8)
    }
    /// Charged bytes of the ImplicitlyCreated @contexts one Tenant holds.
    #[must_use]
    pub fn implicit_contexts(self) -> u64 {
        self.parts(8)
    }
    /// Bytes every concurrently buffered answer holds together.
    #[must_use]
    pub fn answers(self) -> u64 {
        self.parts(64)
    }

    /// Sets the shares of the caches this crate's dependencies keep (the
    /// @context caches, the regex programs). Called before
    /// the state is built: a [`antares_jsonld::Loader`] sizes its caches
    /// when it is made.
    pub fn apply(self) {
        antares_jsonld::set_cache_bytes(self.merged_contexts(), self.fetched_contexts());
        antares_ql::regex::set_cache_bytes(
            usize::try_from(self.regex_programs()).unwrap_or(usize::MAX),
        );
    }

    /// Sets the budget's state fields on `st`: the answer pool, the
    /// per-answer bound (never above [`MAX_ANSWER_BYTES`] nor the pool) and
    /// the ImplicitlyCreated byte cap.
    pub fn set_on(self, st: &mut crate::AppState) {
        let pool = usize::try_from(self.answers()).unwrap_or(usize::MAX);
        st.answer_pool = AnswerPool::new(pool);
        st.max_answer_bytes = MAX_ANSWER_BYTES.min(pool);
        st.implicit_context_bytes = self.implicit_contexts();
    }
}

/// The bytes every in-process answer a handler buffers holds together
/// (T-5895): [`MAX_ANSWER_BYTES`] bounds one answer, this bounds N of them,
/// so concurrency cannot multiply the per-request cap past the memory budget.
/// A read the pool has no room for waits up to [`ANSWER_POOL_WAIT`], then
/// is refused with [`MEMORY_OVERLOADED`] (503 + `Retry-After`), never cut
/// short.
#[derive(Debug)]
pub struct AnswerPool {
    cap: usize,
    held: std::sync::atomic::AtomicUsize,
    /// The most `held` reached since [`AnswerPool::take_peak`] last read it.
    peak: std::sync::atomic::AtomicUsize,
    /// Wakes the reads waiting for room whenever an answer gives its share back.
    freed: tokio::sync::Notify,
    wait: std::time::Duration,
}

impl AnswerPool {
    /// A pool of `cap` bytes whose reads wait up to [`ANSWER_POOL_WAIT`].
    #[must_use]
    pub fn new(cap: usize) -> std::sync::Arc<Self> {
        Self::with_wait(cap, ANSWER_POOL_WAIT)
    }
    /// A pool of `cap` bytes whose reads wait up to `wait` for room.
    #[must_use]
    pub fn with_wait(cap: usize, wait: std::time::Duration) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            cap,
            held: std::sync::atomic::AtomicUsize::new(0),
            peak: std::sync::atomic::AtomicUsize::new(0),
            freed: tokio::sync::Notify::new(),
            wait,
        })
    }
    /// The most bytes held at once since the last call, then starts over
    /// from what is held now: the metric sampler reads the peak between two
    /// samples, which a gauge of `held` alone misses.
    pub fn take_peak(&self) -> usize {
        self.peak.swap(self.held(), Ordering::AcqRel)
    }
    /// The bytes the pool holds at most.
    #[must_use]
    pub fn cap(&self) -> usize {
        self.cap
    }
    /// The bytes buffered answers hold now.
    #[must_use]
    pub fn held(&self) -> usize {
        self.held.load(Ordering::Acquire)
    }
    fn take(&self, n: usize) -> bool {
        match self
            .held
            .try_update(Ordering::AcqRel, Ordering::Acquire, |h| {
                h.checked_add(n).filter(|t| *t <= self.cap)
            }) {
            Ok(before) => {
                self.peak
                    .fetch_max(before.saturating_add(n), Ordering::AcqRel);
                true
            }
            Err(_) => false,
        }
    }
    /// [`AnswerPool::take`], waiting until `until` for an answer to give
    /// room back; a read that already holds `own` and would not fit even an
    /// otherwise empty pool is refused at once. The wasm build has no tokio
    /// timer: there a read takes room only when it is free now.
    async fn take_until(&self, n: usize, own: usize, until: Clock) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        loop {
            // registered before the try, so a share freed in between wakes it
            let freed = self.freed.notified();
            tokio::pin!(freed);
            freed.as_mut().enable();
            if self.take(n) {
                return true;
            }
            let left = until.saturating_duration_since(Clock::now());
            if own.saturating_add(n) > self.cap || tokio::time::timeout(left, freed).await.is_err()
            {
                return self.take(n);
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (own, until);
            self.take(n)
        }
    }
}

/// An answer read whole, holding its bytes of the [`AnswerPool`] until it
/// is dropped.
#[derive(Debug)]
pub struct HeldAnswer {
    bytes: Bytes,
    /// Returned to the pool when the answer drops.
    _charge: Charge,
}

impl HeldAnswer {
    /// The bytes of the answer.
    #[must_use]
    pub fn bytes(&self) -> &Bytes {
        &self.bytes
    }
    /// The bytes, handed on as a response body: the pool share is returned
    /// here, as the server writes them out and frees them.
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.bytes
    }
}

impl std::ops::Deref for HeldAnswer {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}

/// What a read through the pool holds of it, returned on drop.
#[derive(Debug)]
struct Charge {
    pool: std::sync::Arc<AnswerPool>,
    bytes: usize,
}

impl Drop for Charge {
    fn drop(&mut self) {
        if self.bytes > 0 {
            self.pool.held.fetch_sub(self.bytes, Ordering::AcqRel);
            self.pool.freed.notify_waiters();
        }
    }
}

/// Why a buffered read was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerRefusal {
    /// This answer alone is over the per-request bound
    /// ([`ANSWER_TOO_LARGE`]).
    TooLarge,
    /// The pool has no room within its wait ([`MEMORY_OVERLOADED`]).
    OverBudget,
    /// The inner body failed while it was read.
    Body,
}

/// The NGSI-LD API's answer to each refusal: 403 TooManyResults past the
/// per-answer bound (5.5.6), 503 + `Retry-After` while the pool is full.
impl From<AnswerRefusal> for antares_model::NgsiError {
    fn from(r: AnswerRefusal) -> Self {
        match r {
            AnswerRefusal::TooLarge => Self::TooManyResults(ANSWER_TOO_LARGE.into()),
            AnswerRefusal::OverBudget => Self::InternalError(MEMORY_OVERLOADED.into()),
            AnswerRefusal::Body => Self::InternalError("buffered answer body failed".into()),
        }
    }
}

impl From<AnswerRefusal> for crate::negotiate::ApiError {
    fn from(r: AnswerRefusal) -> Self {
        antares_model::NgsiError::from(r).into()
    }
}

/// Reads `body` whole, at most `max` bytes, charging every frame to `pool`
/// as it arrives; a frame the pool has no room for waits for it up to the
/// pool's wait, counted from the start of the read (T-5898). A refusal
/// releases what the read held. `surface` labels the metrics: the size of
/// each answer read whole (`antares_answer_bytes`), the time spent waiting
/// for room and each refusal for a full pool.
///
/// # Errors
/// [`AnswerRefusal`] when the answer is over `max`, the pool had no room
/// within its wait or the body fails.
pub async fn read_answer(
    pool: &std::sync::Arc<AnswerPool>,
    body: Body,
    max: usize,
    surface: &'static str,
) -> Result<HeldAnswer, AnswerRefusal> {
    let started = Clock::now();
    let read = read_charged(pool, body, max, started).await;
    match &read {
        Ok(answer) => {
            metrics::histogram!("antares_answer_bytes", "surface" => surface)
                .record(answer.len() as f64);
        }
        Err(AnswerRefusal::OverBudget) => {
            metrics::counter!("antares_answer_pool_refusals_total", "surface" => surface)
                .increment(1);
        }
        Err(_) => {}
    }
    metrics::histogram!("antares_answer_pool_wait_seconds", "surface" => surface)
        .record(started.elapsed().as_secs_f64());
    read
}

async fn read_charged(
    pool: &std::sync::Arc<AnswerPool>,
    body: Body,
    max: usize,
    started: Clock,
) -> Result<HeldAnswer, AnswerRefusal> {
    use futures_util::StreamExt;
    let until = started.checked_add(pool.wait).unwrap_or(started);
    let mut charge = Charge {
        pool: std::sync::Arc::clone(pool),
        bytes: 0,
    };
    let mut buf = Vec::new();
    let mut frames = body.into_data_stream();
    while let Some(frame) = frames.next().await {
        let frame = frame.map_err(|_| AnswerRefusal::Body)?;
        let total = buf.len().saturating_add(frame.len());
        if total > max {
            return Err(AnswerRefusal::TooLarge);
        }
        if !pool.take_until(frame.len(), charge.bytes, until).await {
            return Err(AnswerRefusal::OverBudget);
        }
        charge.bytes = charge.bytes.saturating_add(frame.len());
        buf.extend_from_slice(&frame);
    }
    Ok(HeldAnswer {
        bytes: Bytes::from(buf),
        _charge: charge,
    })
}
/// → 400 BadRequestData. Maximum coordinate positions in a QUERY geometry
/// (4.10 geoQ, 4.23 ordering reference). The spec sets no ceiling, and the
/// geometry is not bounded by the URI length on the POST query path — the
/// body carries it. Every position is an edge the DE-9IM relate walks once
/// per candidate entity, so the work a single request can buy is capped
/// here: 1024 positions describe an administrative boundary at street
/// resolution, and are already more than the 8 KiB URI ceiling can carry.
pub use antares_ql::geo::MAX_GEO_VERTICES;
/// → 400 BadRequestData. Deployment knob (ANTARES_MAX_BATCH_ITEMS): the
/// spec sets no batch ceiling — 1000 is this broker's DoS-bounds default,
/// raised where a trusted producer legitimately batches larger (e.g. a
/// full-fleet upsert). Read once at first use.
pub static MAX_BATCH_ITEMS: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    std::env::var("ANTARES_MAX_BATCH_ITEMS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1_000)
});
/// Deployment knob (ANTARES_MAX_LIMIT): the most Entities one query page
/// answers (`limit`, 6.3.9; past it 403 TooManyResults). 1,000 unless set;
/// read once at first use, and advertised by the MCP tools as their `limit`
/// maximum (T-4800).
pub static MAX_LIMIT: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    std::env::var("ANTARES_MAX_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1_000)
});
/// Deployment knob (ANTARES_DEFAULT_LIMIT): the Entities a query page answers
/// when the request names no `limit` (6.3.9 leaves it to the broker); the
/// next page is Linked. 100 unless set, never above [`MAX_LIMIT`] (owner,
/// T-4804); advertised by the MCP tools as their `limit` default.
pub static DEFAULT_LIMIT: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    std::env::var("ANTARES_DEFAULT_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(100)
        .min(*MAX_LIMIT)
});
/// Deployment knob (ANTARES_TEMPORAL_INSTANCE_LIMIT): NGSI-LD 6.3.10, the
/// most instances of one Attribute a temporal response serves; past it the
/// representation is cut and answered 206 with a Content-Range. 1,000 unless
/// set: a week of a 10-minute sensor in one answer (T-4800). The ETSI suite
/// creates 20 instances and expects the cut, so its brokers run with 9. The
/// answer stays bounded by [`MAX_ANSWER_BYTES`]; the MCP tools advertise it
/// as their `lastN` maximum. Read once at first use into
/// `AppState::temporal_instance_limit`, which a test or the wasm broker
/// (no process env; it reads `globalThis`) sets per state.
pub static TEMPORAL_INSTANCE_LIMIT: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    std::env::var("ANTARES_TEMPORAL_INSTANCE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1_000)
});

/// [`TEMPORAL_INSTANCE_LIMIT`].
pub fn temporal_instance_limit() -> usize {
    *TEMPORAL_INSTANCE_LIMIT
}

/// Deployment knob (ANTARES_TEMPORAL_DEFAULT_INSTANCES): the instances of one
/// Attribute a temporal response serves when the request names no
/// lastN/firstN; past it the answer is cut and 206 like the ceiling's.
/// 100 unless set, never above [`temporal_instance_limit`], so the ETSI
/// brokers (ceiling 9) keep 9 (owner, T-4804). Read once at first use into
/// `AppState::temporal_default_instances`.
pub static TEMPORAL_DEFAULT_INSTANCES: std::sync::LazyLock<usize> =
    std::sync::LazyLock::new(|| {
        std::env::var("ANTARES_TEMPORAL_DEFAULT_INSTANCES")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|n| *n > 0)
            .unwrap_or(100)
            .min(temporal_instance_limit())
    });

/// [`TEMPORAL_DEFAULT_INSTANCES`].
pub fn temporal_default_instances() -> usize {
    *TEMPORAL_DEFAULT_INSTANCES
}
// Deployment knob (ANTARES_MAX_FED_RESPONSE_BYTES): ceiling on one forwarded
// (4.3.6) response body. The spec sets no ceiling; an over-cap peer part
// fails like an unparseable payload (Table 6.3.17-1, warning 111) instead of
// ballooning broker memory — one misbehaving peer must not break the
// 500 MB RSS budget. Read once at first use.
pub static MAX_FED_RESPONSE_BYTES: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    std::env::var("ANTARES_MAX_FED_RESPONSE_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(16 * 1024 * 1024)
});
// Deployment knob (ANTARES_FED_INFLIGHT): forwarded requests in flight for
// the whole process. Per-request fan-out is bounded below; across requests
// nothing was, and 6 000 open federated queries × 34 sources each held
// 7.7 GB of buffers and connections. Callers over the cap wait their turn.
pub static MAX_FED_INFLIGHT: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    std::env::var("ANTARES_FED_INFLIGHT")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(256)
});
pub static FED_INFLIGHT: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(*MAX_FED_INFLIGHT));
// Deployment knob (ANTARES_FED_FANOUT): concurrent forwards per distributed
// read. 4.3.6.1 orders the MERGE (4.5.5), never the requests, so forwards
// run concurrently; this bounds how many at once per request.
pub static MAX_FED_FANOUT: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    std::env::var("ANTARES_FED_FANOUT")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(8)
});
/// 6.3.17: `NGSILD-Warning` values relayed from ONE Context Source. The
/// clause makes a peer's warnings part of this broker's answer (4.3.6.4 puts
/// a deeper hop's abnormality on the response that survived it), so the list
/// is written by the peer but sent by this broker to a client that never
/// addressed it. Table 6.3.17-1 defines four codes and a cascade adds a few
/// per hop, so eight carries a real cascade; past it a source would make the
/// response grow faster than the fan-out does, and would crowd out the
/// warnings the clause obliges this broker to raise about the other sources.
pub const MAX_PEER_WARNINGS: usize = 8;

/// → 500 InternalError. How many [`crate::AppState::call`] frames one
/// request may be inside. The in-process handle is the façade seam, and a
/// façade legitimately calls the broker while another façade legitimately
/// calls it, so the ceiling is a depth rather than a refusal of the second
/// call. Without one a `/x/` route that translates into a request its own
/// surface serves recurses until the stack ends, and each frame builds a
/// router of its own. Eight carries a façade over a façade over the broker
/// several times and still ends the loop in milliseconds, an order of
/// magnitude below what ends the process: with the guard removed, a route
/// asking for thirty-two hops overflows a 2 MiB thread stack in a debug
/// build. The count is per task — work a handler spawns starts a new chain,
/// which is right, since a notification is not inside the request that
/// caused it.
pub const MAX_IN_PROCESS_CALL_DEPTH: usize = 8;

pub const MAX_JOIN_LEVEL: usize = 10; // → 400 BadRequestData
/// → 400 BadRequestData. Documents one @context resolution may fetch, owned
/// by the loader that enforces it (`antares_jsonld`), and the ceiling on how
/// many DISTINCT @contexts one batch may name — without the second, the item
/// count multiplies the first.
pub use antares_jsonld::MAX_CONTEXT_URLS as MAX_CONTEXT_FETCHES;
/// Linked-entity lookup budget per `q=`, owned by the shared evaluator.
pub use antares_ql::eval::MAX_Q_LINK_LOOKUPS;
/// Regex compile ceiling and retention caps, owned by the shared cache
/// (`antares_ql::regex`).
pub use antares_ql::regex::{MAX_REGEX_CACHE, MAX_REGEX_PROGRAM_BYTES};
/// → 403 TooComplexQuery. The AST size cap, owned by the parser that
/// enforces it (`antares_ql::parse_q`).
pub use antares_ql::MAX_Q_NODES;

/// → 403 TooManyResults (5.5.6). Deployment knob
/// (ANTARES_DISCOVERY_SCAN_MAX): documents ONE unpaginated whole-tenant fold
/// may read or hold. The folds that carry it are the ones the spec gives no
/// window to push into the store: `/types` and `/attributes` (5.7.5-5.7.10
/// define no pagination) and the registration query (5.10.2.4 filters first
/// and pages second, so the page cannot be pushed down). Without it one
/// request over a tenant at the 100 000-registration target holds every match
/// at once. Read once at first use.
pub static MAX_FOLD_DOCS: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    std::env::var("ANTARES_DISCOVERY_SCAN_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(100_000)
});

/// The ceilings on the notification pipeline. Not input bounds: no request
/// names them and none is rejected against them. They are published beside
/// the input caps for the same reason the regex-cache caps above are — an
/// operator reading `/q/health` has to be able to tell a ceiling from a
/// coincidence, and reaching one of these is what a dropped change or a
/// stalled fan-out looks like from outside.
///
/// Depth of the change→matcher ring, the same size the local bus uses. A
/// full ring drops the batch and counts it
/// (`antares_notification_changes_dropped_total`), so this number is the
/// back-pressure a deployment has before delivery loss starts.
pub const CHANGE_QUEUE: usize = 1024;
/// Groups one subscription may have waiting for delivery. Each subscription
/// is served on a lane of its own, so a slow or dead endpoint backs up its
/// lane and never the matching of every other change (T-4086); a full lane
/// drops the group for that subscription alone and counts it
/// (`antares_notification_lane_dropped_total`).
pub const DELIVERY_LANE: usize = 64;
/// What every delivery lane together may hold, in estimated heap bytes of
/// the entities waiting in them. [`DELIVERY_LANE`] counts groups, and a
/// group is up to a drain's worth of entities copied for that subscription
/// alone, so twenty subscriptions behind slow receivers under a batch upsert
/// flood held 64 groups of 100 3 KB entities each and took the process past
/// its 1536 MiB limit in seconds (T-4549). Past this the group is dropped
/// for that subscription and counted, as a full lane drops it. 128 MiB
/// leaves room for a running batch, which costs as much again (T-4562).
/// Deployment knob (ANTARES_DELIVERY_LANE_BYTES), read once at first use.
pub static DELIVERY_LANE_BYTES: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    count_from(
        std::env::var("ANTARES_DELIVERY_LANE_BYTES").ok().as_deref(),
        128 * 1024 * 1024,
        usize::MAX,
    )
});
/// Batch write bodies in flight at once, in bytes. One batch costs about a
/// hundred times its body while it runs: the parsed items, their expansion
/// for the policy check, the stored pre-images, the forward copy, the rows
/// and the change events, so a 1000-entity upsert of 3 MB held about 300 MB
/// and eight of them at once took the process past its 1536 MiB limit
/// (T-4562); two at once were 480 MB live, so the default runs one batch of
/// that size beside small ones. A batch waits for its share and is answered 503 with
/// `Retry-After` when none frees inside [`BATCH_ADMIT_WAIT`]; one batch is
/// always admitted alone, however large. Deployment knob
/// (ANTARES_BATCH_INFLIGHT_BYTES), read once at first use.
pub static BATCH_INFLIGHT_BYTES: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    count_from(
        std::env::var("ANTARES_BATCH_INFLIGHT_BYTES")
            .ok()
            .as_deref(),
        4 * 1024 * 1024,
        u32::MAX as usize,
    )
});
/// How long a batch waits for its share of [`BATCH_INFLIGHT_BYTES`]. A waiting
/// batch holds only its body, and the batches ahead of it take well under a
/// second each, so it waits long enough for a queue of dozens to drain; a
/// client timeout of 30 s still sees the 503 first.
pub const BATCH_ADMIT_WAIT: std::time::Duration = std::time::Duration::from_secs(20);
static BATCH_SLOTS: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(*BATCH_INFLIGHT_BYTES));

/// Batch writes running at once, whatever their size. Each holds a database
/// connection for as long as it runs, so a stream of small batches has to
/// leave the rest of the pool to the reads: 811 vehicle upserts in two
/// minutes held all 20 of a dev broker's and a visitor's read waited 4 s for
/// one (T-4624). Deployment knob (ANTARES_BATCH_INFLIGHT), read once at first
/// use; keep it under `ANTARES_PG_POOL`.
pub static BATCH_INFLIGHT: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    count_from(
        std::env::var("ANTARES_BATCH_INFLIGHT").ok().as_deref(),
        8,
        u32::MAX as usize,
    )
});
static BATCH_RUNNING: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(*BATCH_INFLIGHT));

/// The admission of a batch body of `bytes`, held for as long as the batch
/// runs: its share of [`BATCH_INFLIGHT_BYTES`] and one of [`BATCH_INFLIGHT`]
/// slots; `None` when they did not free inside [`BATCH_ADMIT_WAIT`].
pub async fn admit_batch(bytes: usize) -> Option<Admitted> {
    admit_both(
        (&BATCH_SLOTS, *BATCH_INFLIGHT_BYTES),
        (&BATCH_RUNNING, *BATCH_INFLIGHT),
        bytes,
        BATCH_ADMIT_WAIT,
    )
    .await
}

/// A running batch's share of the byte budget and its slot.
pub type Admitted = (
    tokio::sync::SemaphorePermit<'static>,
    tokio::sync::SemaphorePermit<'static>,
);

async fn admit_both<'a>(
    bytes_budget: (&'a tokio::sync::Semaphore, usize),
    running: (&'a tokio::sync::Semaphore, usize),
    bytes: usize,
    wait: std::time::Duration,
) -> Option<(
    tokio::sync::SemaphorePermit<'a>,
    tokio::sync::SemaphorePermit<'a>,
)> {
    // One wait for both: the slot gets what the byte share left of it.
    // Clock rule: std Instant panics on wasm32 (every batch write aborted the
    // wasm broker, T-4937).
    #[cfg(not(target_arch = "wasm32"))]
    let started = std::time::Instant::now();
    #[cfg(target_arch = "wasm32")]
    let started = web_time::Instant::now();
    let share = admit(bytes_budget.0, bytes_budget.1, bytes, wait).await?;
    let slot = admit(
        running.0,
        running.1,
        1,
        wait.saturating_sub(started.elapsed()),
    )
    .await?;
    Some((share, slot))
}

async fn admit(
    slots: &tokio::sync::Semaphore,
    budget: usize,
    bytes: usize,
    wait: std::time::Duration,
) -> Option<tokio::sync::SemaphorePermit<'_>> {
    // capped at the whole budget, so the largest body is admitted once it runs alone
    let weight = bytes.clamp(1, budget) as u32;
    #[cfg(not(target_arch = "wasm32"))]
    let admitted = tokio::time::timeout(wait, slots.acquire_many(weight))
        .await
        .ok()
        .and_then(Result::ok);
    // the wasm build has no tokio timer: a batch is admitted only when its share is free now
    #[cfg(target_arch = "wasm32")]
    let admitted = {
        let _ = wait;
        slots.try_acquire_many(weight).ok()
    };
    match admitted {
        Some(permit) => Some(permit),
        _ => {
            metrics::counter!("antares_batch_overloaded_total").increment(1);
            None
        }
    }
}
/// Notifications in flight at once per drain: one serial POST at a time
/// capped a 9-subscription fan-out at ~600 POST/s and overflowed the ring.
/// Deployment knob (ANTARES_DELIVERY_WIDTH): what a width is worth is a
/// property of the subscribers, not of this broker — a slot is held for as
/// long as the endpoint takes to answer, so a fleet of local sinks and a
/// fleet of remote endpoints sitting at their 30 s timeout (Table 5.2.15-1)
/// are served by different numbers, and the one that fits is measured
/// against a deployment rather than compiled in. Read once at first use.
pub static DELIVERY_WIDTH: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    count_from(
        std::env::var("ANTARES_DELIVERY_WIDTH").ok().as_deref(),
        64,
        usize::MAX,
    )
});
/// Of that width, what one tenant may hold. A Subscription belongs to one
/// tenant (5.2.12), and a delivery to an endpoint that accepts and never
/// answers holds its slot for the endpoint's whole timeout — up to 30 s
/// (Table 5.2.15-1). Sharing the width with no per-tenant bound, a single
/// tenant with enough dead endpoints holds every slot and nothing leaves the
/// broker for anyone else. The share is a fraction of the width and not a
/// fair split of it: a tenant delivering alone still gets several slots, and
/// eight of them is what a full width of 64 divides into before the
/// per-tenant queue becomes the bottleneck for an ordinary fan-out.
/// Deployment knob (ANTARES_DELIVERY_WIDTH_PER_TENANT), ceilinged at the
/// width above: a share larger than the width is not a share, and
/// publishing one would name a ceiling that can never fire. Read once at
/// first use.
pub static DELIVERY_WIDTH_PER_TENANT: std::sync::LazyLock<usize> = std::sync::LazyLock::new(|| {
    count_from(
        std::env::var("ANTARES_DELIVERY_WIDTH_PER_TENANT")
            .ok()
            .as_deref(),
        8,
        *DELIVERY_WIDTH,
    )
});

/// One configured count: the default stands in for anything unset,
/// unparseable or zero — a zero would mint a semaphore that admits nobody
/// and stop delivery altogether — and the ceiling bounds what a deployment
/// can ask for where a larger number would be meaningless.
fn count_from(raw: Option<&str>, default: usize, ceiling: usize) -> usize {
    raw.and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
        .min(ceiling)
}
/// Destinations and registrations the egress breaker tracks. Both maps are
/// keyed by client-supplied strings, so they need a bound: at the ceiling the
/// least recently recorded entry is dropped, which costs at most a forgotten
/// failure count for a destination nobody has touched in a while.
pub const MAX_TRACKED_DESTINATIONS: usize = 4096;

/// Rejection counters, exported by /q/health.
#[derive(Default)]
pub struct LimitStats {
    pub uri_too_long: AtomicU64,
    pub body_too_large: AtomicU64,
    pub body_too_deep: AtomicU64,
}

impl LimitStats {
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "maxBodyBytes": *MAX_BODY_BYTES,
            "maxUriBytes": MAX_URI_BYTES,
            "maxJsonDepth": MAX_JSON_DEPTH,
            "maxGeoVertices": MAX_GEO_VERTICES,
            "maxBatchItems": *MAX_BATCH_ITEMS,
            "maxLimit": *MAX_LIMIT,
            "defaultLimit": *DEFAULT_LIMIT,
            "temporalInstanceLimit": temporal_instance_limit(),
            "temporalDefaultInstances": temporal_default_instances(),
            "maxAnswerBytes": MAX_ANSWER_BYTES,
            "maxFoldDocs": *MAX_FOLD_DOCS,
            "maxFedResponseBytes": *MAX_FED_RESPONSE_BYTES,
            "maxFedFanout": *MAX_FED_FANOUT,
            "maxFedInflight": *MAX_FED_INFLIGHT,
            "maxJoinLevel": MAX_JOIN_LEVEL,
            "maxInProcessCallDepth": MAX_IN_PROCESS_CALL_DEPTH,
            "maxPeerWarnings": MAX_PEER_WARNINGS,
            "maxContextFetches": MAX_CONTEXT_FETCHES,
            "maxQNodes": MAX_Q_NODES,
            "maxQLinkLookups": MAX_Q_LINK_LOOKUPS,
            "maxRegexCache": MAX_REGEX_CACHE,
            "maxRegexCacheBytes": antares_ql::regex::cache_bytes(),
            "maxRegexProgramBytes": MAX_REGEX_PROGRAM_BYTES,
            "changeQueue": CHANGE_QUEUE,
            "deliveryLane": DELIVERY_LANE,
            "batchInflight": *BATCH_INFLIGHT,
            "batchInflightBytes": *BATCH_INFLIGHT_BYTES,
            "deliveryLaneBytes": *DELIVERY_LANE_BYTES,
            "deliveryWidth": *DELIVERY_WIDTH,
            "deliveryWidthPerTenant": *DELIVERY_WIDTH_PER_TENANT,
            "maxTrackedDestinations": MAX_TRACKED_DESTINATIONS,
            "rejectedUriTooLong": self.uri_too_long.load(Ordering::Relaxed),
            "rejectedBodyTooLarge": self.body_too_large.load(Ordering::Relaxed),
            "rejectedBodyTooDeep": self.body_too_deep.load(Ordering::Relaxed),
        })
    }
}

/// Maximum brace/bracket nesting of a JSON byte stream, string-aware.
/// A scan, not a parse — depth is checked before serde ever runs.
pub fn json_depth(bytes: &[u8]) -> usize {
    let (mut depth, mut max, mut in_str, mut esc) = (0usize, 0usize, false, false);
    for &b in bytes {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                max = max.max(depth);
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

pub(crate) async fn bounds_layer(
    axum::extract::State(st): axum::extract::State<crate::AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if req.uri().to_string().len() > MAX_URI_BYTES {
        st.limits.uri_too_long.fetch_add(1, Ordering::Relaxed);
        return StatusCode::URI_TOO_LONG.into_response(); // bare, like 6.3.4
    }
    // 6.3.4: "For HTTP POST, PATCH and PUT HTTP requests implementations shall
    // check … Content-Length header shall include the length of the request
    // payload body", and its absence "shall result in just a 411 HTTP status
    // code (without any payload body)" — restated in 6.3.2. Scoped to HTTP/1.x:
    // HTTP/2 and later carry length in the framing layer and legitimately omit
    // the header, so demanding it there would reject conformant clients.
    // The clause grants NO exemption for `Transfer-Encoding: chunked` — a
    // chunked POST without Content-Length is exactly the case 411 covers, so it
    // is deliberately not carved out here.
    //
    // The ONE deviation, made explicit rather than implied: the check is scoped
    // to HTTP/1.x. 6.3.4 is written against RFC 7230/7231 and predates any h2
    // consideration; HTTP/2 carries length in its framing and conformant h2
    // clients routinely omit the header, so applying it there would reject
    // requests the spec never meant to describe. Recorded in docs/ics.yaml.
    if matches!(req.method().as_str(), "POST" | "PATCH" | "PUT")
        && req.version() <= axum::http::Version::HTTP_11
        && !req
            .headers()
            .contains_key(axum::http::header::CONTENT_LENGTH)
    {
        return StatusCode::LENGTH_REQUIRED.into_response(); // bare 411
    }
    let has_body = matches!(req.method().as_str(), "POST" | "PATCH" | "PUT" | "DELETE");
    if !has_body {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    // What the client says it is sending. The read below still decides — a
    // declared length is a claim, and answering on the claim alone would cut
    // the client off mid-body, where the RST that follows can take the
    // response with it.
    let fits_its_claim = parts
        .headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|n| n <= *MAX_BODY_BYTES);
    let bytes: Bytes = match axum::body::to_bytes(body, *MAX_BODY_BYTES).await {
        Ok(b) => b,
        // `to_bytes` reports the length limit and any transport failure the
        // same way. A body whose declared length fits the cap cannot have
        // exceeded it, so what failed was the delivery: that is a bad request
        // and NOT a size rejection — counting it as one would make
        // `rejectedBodyTooLarge` read client aborts as clients hitting the
        // cap. A body that declared more than the cap, or declared nothing
        // (chunked), can only have hit the cap.
        Err(_) if fits_its_claim => {
            return crate::negotiate::ApiError::from(antares_model::NgsiError::InvalidRequest(
                "request body was not delivered completely".into(),
            ))
            .into_response();
        }
        Err(_) => {
            st.limits.body_too_large.fetch_add(1, Ordering::Relaxed);
            return StatusCode::PAYLOAD_TOO_LARGE.into_response(); // bare 413
        }
    };
    // An absent (or unreadable) Content-Type is parsed as JSON downstream —
    // 6.3.4 mandates Content-Length, not Content-Type — so it is scanned
    // here too, or the nesting cap has a hole exactly where the parser has
    // none. A header that names another media type keeps its 415.
    let is_json = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .is_none_or(|v| match v.to_str() {
            Ok(ct) => ct.contains("json"),
            // A header the parser cannot read is not a header naming another
            // media type: `negotiate::content_type` reports it as the empty
            // string, which every route that tolerates an absent
            // Content-Type reads as absent and parses. Scanning it is what
            // keeps the cap ahead of the parser on those routes.
            Err(_) => true,
        });
    if is_json && json_depth(&bytes) > MAX_JSON_DEPTH {
        st.limits.body_too_deep.fetch_add(1, Ordering::Relaxed);
        return crate::negotiate::ApiError::from(antares_model::NgsiError::BadRequestData(
            format!("JSON nesting exceeds the {MAX_JSON_DEPTH}-level limit"),
        ))
        .into_response();
    }
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-5895: `ANTARES_MEMORY_BUDGET` takes bytes or Ki/Mi/Gi, refuses
    /// anything else and anything under 64 MiB with the key in the message.
    #[test]
    fn the_memory_budget_parses_sizes_and_refuses_the_rest() {
        assert_eq!(MemoryBudget::parse("200Mi").map(|b| b.total), Ok(200 << 20));
        assert_eq!(MemoryBudget::parse("1Gi").map(|b| b.total), Ok(1 << 30));
        assert_eq!(
            MemoryBudget::parse(" 134217728 ").map(|b| b.total),
            Ok(128 << 20)
        );
        assert_eq!(
            MemoryBudget::parse("65536Ki").map(|b| b.total),
            Ok(64 << 20)
        );
        for bad in [
            "",
            "200MB",
            "-1",
            "2.5Gi",
            "Mi",
            "63Mi",
            "99999999999999999999Gi",
        ] {
            let err = MemoryBudget::parse(bad).expect_err(bad);
            assert!(err.starts_with("ANTARES_MEMORY_BUDGET="), "{bad}: {err}");
        }
    }

    /// The default shares are the defaults the caches carry, so a broker
    /// without the key is the 200 MiB split, and the shares leave the
    /// process its 28 parts.
    #[test]
    fn the_default_split_is_what_the_caches_hold_without_the_key() {
        let b = MemoryBudget::default();
        assert_eq!(
            b.merged_contexts(),
            antares_jsonld::DEFAULT_MERGED_CACHE_BYTES
        );
        assert_eq!(
            b.fetched_contexts(),
            antares_jsonld::DEFAULT_FETCHED_CACHE_BYTES
        );
        assert_eq!(
            usize::try_from(b.regex_programs()).ok(),
            Some(antares_ql::regex::DEFAULT_REGEX_CACHE_BYTES)
        );
        assert_eq!(b.odata_models(), 16 << 20);
        assert_eq!(b.linkml_models(), 8 << 20);
        assert_eq!(b.linkml_renders(), 8 << 20);
        let shares = [
            b.merged_contexts(),
            b.fetched_contexts(),
            b.odata_models(),
            b.linkml_models(),
            b.regex_programs(),
            b.linkml_renders(),
            b.implicit_contexts(),
            b.answers(),
        ];
        assert_eq!(shares.iter().sum::<u64>(), 172 << 20);
        let st = &mut crate::AppState::new("antares-budget-split".into());
        MemoryBudget { total: 100 << 20 }.set_on(st);
        assert_eq!(st.answer_pool.cap(), 32 << 20);
        assert_eq!(st.max_answer_bytes, 32 << 20, "one answer fits the pool");
        MemoryBudget { total: 1 << 30 }.set_on(st);
        assert_eq!(
            st.max_answer_bytes, MAX_ANSWER_BYTES,
            "never above the per-answer cap"
        );
    }

    fn body_of(n: usize) -> Body {
        Body::from(vec![b'x'; n])
    }

    /// N concurrent buffered answers share one pool: past it a read is
    /// refused whole, and a dropped answer gives its bytes back.
    #[tokio::test]
    async fn the_answer_pool_refuses_past_its_cap_and_frees_on_drop() {
        let pool = AnswerPool::with_wait(1000, std::time::Duration::ZERO);
        let first = read_answer(&pool, body_of(600), 800, "test")
            .await
            .expect("fits");
        assert_eq!(first.len(), 600);
        assert_eq!(pool.held(), 600);
        assert_eq!(
            read_answer(&pool, body_of(600), 800, "test").await.err(),
            Some(AnswerRefusal::OverBudget)
        );
        assert_eq!(pool.held(), 600, "a refused read holds nothing");
        assert_eq!(
            read_answer(&pool, body_of(900), 800, "test").await.err(),
            Some(AnswerRefusal::TooLarge),
            "one answer stays under the per-answer bound"
        );
        drop(first);
        assert_eq!(pool.held(), 0);
        let again = read_answer(&pool, body_of(600), 800, "test")
            .await
            .expect("room again");
        assert_eq!(again.into_bytes().len(), 600);
        assert_eq!(pool.held(), 0, "a body handed on leaves the pool");
        assert_eq!(pool.take_peak(), 600, "the peak between two samples");
        assert_eq!(pool.take_peak(), 0, "starts over from what is held");
    }

    /// A read the full pool has no room for waits, and gets its room when an
    /// answer ahead of it is written out before the wait ends (T-5898).
    #[tokio::test(start_paused = true)]
    async fn a_reader_waits_for_room_and_gets_it_before_the_wait_ends() {
        let pool = AnswerPool::with_wait(1000, std::time::Duration::from_secs(2));
        let first = read_answer(&pool, body_of(600), 800, "test")
            .await
            .expect("fits");
        let started = tokio::time::Instant::now();
        let waiting = tokio::spawn({
            let pool = std::sync::Arc::clone(&pool);
            async move { read_answer(&pool, body_of(600), 800, "test").await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(!waiting.is_finished(), "it waits while the pool is full");
        drop(first);
        let second = waiting.await.expect("joined").expect("room freed in time");
        assert_eq!(second.len(), 600);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(pool.held(), 600);
    }

    /// Past the wait the read is refused whole and holds nothing.
    #[tokio::test(start_paused = true)]
    async fn a_reader_is_refused_when_no_room_frees_within_the_wait() {
        let pool = AnswerPool::with_wait(1000, std::time::Duration::from_secs(2));
        let _first = read_answer(&pool, body_of(600), 800, "test")
            .await
            .expect("fits");
        let started = tokio::time::Instant::now();
        assert_eq!(
            read_answer(&pool, body_of(600), 800, "test").await.err(),
            Some(AnswerRefusal::OverBudget)
        );
        assert!(started.elapsed() >= std::time::Duration::from_secs(2));
        assert_eq!(pool.held(), 600, "a refused read holds nothing");
    }

    /// A read that would not fit even an empty pool beside what it already
    /// holds is refused at once: no wait can make room for it.
    #[tokio::test(start_paused = true)]
    async fn a_read_too_big_for_the_whole_pool_does_not_wait() {
        let pool = AnswerPool::with_wait(1000, std::time::Duration::from_secs(2));
        let frames = futures_util::stream::iter([
            Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 600])),
            Ok(Bytes::from(vec![b'x'; 600])),
        ]);
        let started = tokio::time::Instant::now();
        assert_eq!(
            read_answer(&pool, Body::from_stream(frames), 4000, "test")
                .await
                .err(),
            Some(AnswerRefusal::OverBudget)
        );
        assert_eq!(started.elapsed(), std::time::Duration::ZERO);
        assert_eq!(pool.held(), 0, "a refused read holds nothing");
    }

    /// The API answers a full pool 503 with Retry-After, never a body cut
    /// short, and an answer past the per-answer bound 403 TooManyResults.
    #[test]
    fn a_full_pool_answers_503_with_retry_after() {
        let r = crate::negotiate::ApiError::from(AnswerRefusal::OverBudget).into_response();
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            r.headers()
                .get(axum::http::header::RETRY_AFTER)
                .map(|v| v.as_bytes()),
            Some(&b"1"[..])
        );
        let r = crate::negotiate::ApiError::from(AnswerRefusal::TooLarge).into_response();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    /// The scan is a bound, not a parser: unbalanced closers must not
    /// underflow, and the value it reports is the one the middleware compares
    /// against MAX_JSON_DEPTH, so the accept/reject boundary is exact.
    #[test]
    fn depth_scan_survives_unbalanced_and_boundary_input() {
        assert_eq!(json_depth(b"]]]]"), 0, "stray closers must not underflow");
        assert_eq!(json_depth(b"}}}{"), 1);
        assert_eq!(json_depth(b""), 0);
        assert_eq!(
            json_depth(br#""{{{{""#),
            0,
            "a bare string carries no depth"
        );
        assert_eq!(
            json_depth(br#"{"a": "\\"}"#),
            1,
            "an escaped backslash ends the escape"
        );
        let at_cap = "[".repeat(MAX_JSON_DEPTH) + &"]".repeat(MAX_JSON_DEPTH);
        assert_eq!(json_depth(at_cap.as_bytes()), MAX_JSON_DEPTH);
        assert!(
            json_depth(at_cap.as_bytes()) <= MAX_JSON_DEPTH,
            "exactly at the cap is accepted"
        );
        let over = "[".repeat(MAX_JSON_DEPTH + 1) + &"]".repeat(MAX_JSON_DEPTH + 1);
        assert!(
            json_depth(over.as_bytes()) > MAX_JSON_DEPTH,
            "one over is rejected"
        );
    }

    /// A cap `/q/health` publishes is the cap that fires: an OR chain of
    /// `MAX_Q_NODES - 1` terms (the chain node plus its terms) parses, one
    /// term more is 5.5.6 TooComplexQuery.
    #[test]
    fn the_published_q_node_cap_is_the_enforced_one() {
        let chain = |k: usize| vec!["a==1"; k].join("|");
        assert!(antares_ql::parse_q(&chain(MAX_Q_NODES - 1)).is_ok());
        assert!(matches!(
            antares_ql::parse_q(&chain(MAX_Q_NODES)),
            Err(antares_model::NgsiError::TooComplexQuery(_))
        ));
    }

    /// Every cap belongs in `/q/health` AND in the operator's chapter: the
    /// admin-API chapter prints the whole `limits` object as the answer a
    /// memory-store broker gives, and an operator reads a bound from there.
    /// The test above pins the key set in code; nothing pinned the chapter,
    /// and `maxFedInflight` was published for a release without ever being
    /// written down. A cap added here now fails until the chapter has it.
    #[test]
    fn the_documented_health_limits_are_the_published_ones() {
        let book = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/src/admin-api.md"
        ))
        .expect("the admin API chapter");
        let block = book
            .split_once("\"limits\": {")
            .expect("the limits object in the sample response")
            .1;
        let block = block.split_once('}').expect("the end of the object").0;
        let mut documented: Vec<&str> = block
            .lines()
            .filter_map(|l| l.trim().strip_prefix('"'))
            .filter_map(|l| l.split_once('"'))
            .map(|(k, _)| k)
            .collect();
        documented.sort_unstable();
        let snap = LimitStats::default().snapshot();
        let mut published: Vec<&str> = snap
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        published.sort_unstable();
        assert_eq!(
            documented, published,
            "the admin API chapter's limits object is not the one /q/health answers"
        );
    }

    /// /q/health publishes the caps and the rejection counters — and nothing
    /// else: no configuration paths, no environment variable values, no
    /// internal error text.
    #[test]
    fn health_snapshot_reports_the_caps_and_nothing_internal() {
        let stats = LimitStats::default();
        stats.uri_too_long.fetch_add(3, Ordering::Relaxed);
        let snap = stats.snapshot();
        let obj = snap.as_object().expect("object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "batchInflight",
                "batchInflightBytes",
                "changeQueue",
                "defaultLimit",
                "deliveryLane",
                "deliveryLaneBytes",
                "deliveryWidth",
                "deliveryWidthPerTenant",
                "maxAnswerBytes",
                "maxBatchItems",
                "maxBodyBytes",
                "maxContextFetches",
                "maxFedFanout",
                "maxFedInflight",
                "maxFedResponseBytes",
                "maxFoldDocs",
                "maxGeoVertices",
                "maxInProcessCallDepth",
                "maxJoinLevel",
                "maxJsonDepth",
                "maxLimit",
                "maxPeerWarnings",
                "maxQLinkLookups",
                "maxQNodes",
                "maxRegexCache",
                "maxRegexCacheBytes",
                "maxRegexProgramBytes",
                "maxTrackedDestinations",
                "maxUriBytes",
                "rejectedBodyTooDeep",
                "rejectedBodyTooLarge",
                "rejectedUriTooLong",
                "temporalDefaultInstances",
                "temporalInstanceLimit",
            ],
            "no member beyond the caps, the pipeline ceilings and the counters"
        );
        assert_eq!(snap["rejectedUriTooLong"], 3);
        assert!(
            obj.values().all(|v| v.is_number()),
            "every member is a number — no strings to leak paths through"
        );
    }

    /// The nesting cap is checked BEFORE any parse — including on the path
    /// that carries no Content-Type header at all, which the body parser
    /// accepts and parses as JSON (6.3.4 only mandates Content-Length).
    /// An unparseable Content-Type follows the same rule.
    #[tokio::test]
    async fn over_depth_body_without_content_type_is_still_rejected() {
        use tower::ServiceExt;
        let st = crate::AppState::new("http://localhost:0".into());
        let app = axum::Router::new()
            .route(
                "/x",
                axum::routing::post(|| async { StatusCode::NO_CONTENT }),
            )
            .layer(axum::middleware::from_fn_with_state(st, bounds_layer));
        let deep = "[".repeat(MAX_JSON_DEPTH + 5) + &"]".repeat(MAX_JSON_DEPTH + 5);
        // The third case is the one a header map can hold and `to_str`
        // cannot read: a Content-Type carrying a byte outside UTF-8. Every
        // route reads it as an absent Content-Type, so the scan must too.
        let unreadable = axum::http::HeaderValue::from_bytes(b"application/\xffjson")
            .expect("header value from raw bytes");
        assert!(
            unreadable.to_str().is_err(),
            "the case under test is a header value that cannot be read as text"
        );
        for ct in [
            None,
            Some(axum::http::HeaderValue::from_static("application/json")),
            Some(unreadable),
        ] {
            let mut req = Request::post("/x")
                .header(axum::http::header::CONTENT_LENGTH, deep.len().to_string());
            if let Some(ct) = ct.clone() {
                req = req.header(axum::http::header::CONTENT_TYPE, ct);
            }
            let resp = app
                .clone()
                .oneshot(req.body(Body::from(deep.clone())).expect("req"))
                .await
                .expect("resp");
            assert_eq!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "an over-deep body must not reach the handler (content-type {ct:?})"
            );
        }
    }

    /// 6.3.4's 413 is about size, and `/q/health` publishes how often it
    /// fired. A body the client abandoned mid-flight is a different event: it
    /// is not over the cap, it must not be counted as one, and a declared
    /// length over the cap must be refused before the broker buffers a byte
    /// of it.
    #[tokio::test]
    async fn a_broken_body_is_not_an_over_cap_body() {
        use tower::ServiceExt;
        let st = crate::AppState::new("http://localhost:0".into());
        let app = axum::Router::new()
            .route(
                "/x",
                axum::routing::post(|| async { StatusCode::NO_CONTENT }),
            )
            .layer(axum::middleware::from_fn_with_state(
                st.clone(),
                bounds_layer,
            ));

        // the transport gave up: a declared length the body never delivers
        let broken = Body::from_stream(futures_util::stream::once(async {
            Err::<axum::body::Bytes, std::io::Error>(std::io::Error::other("reset"))
        }));
        let resp = app
            .clone()
            .oneshot(
                Request::post("/x")
                    .header(axum::http::header::CONTENT_LENGTH, "100")
                    .body(broken)
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            st.limits.body_too_large.load(Ordering::Relaxed),
            0,
            "a client abort is not a size rejection"
        );

        // over the cap for real: 6.3.4's 413, counted
        let over = vec![b'x'; *MAX_BODY_BYTES + 1];
        let resp = app
            .oneshot(
                Request::post("/x")
                    .header(axum::http::header::CONTENT_LENGTH, over.len().to_string())
                    .body(Body::from(over))
                    .expect("req"),
            )
            .await
            .expect("resp");
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(st.limits.body_too_large.load(Ordering::Relaxed), 1);
    }

    /// Every cap this module declares is in the payload `/q/health` serves.
    /// A cap an operator cannot read is one they cannot tell from a
    /// coincidence when a request is refused or a change is dropped, so the
    /// list is read out of this file's own source rather than kept by hand:
    /// a cap added below without a member above fails here.
    #[test]
    fn every_declared_cap_is_published() {
        let src = include_str!("bounds.rs");
        let published = LimitStats::default().snapshot();
        let published = published.as_object().expect("an object");
        let mut missing = Vec::new();
        for line in src.lines() {
            let line = line.trim_start();
            // `pub use` re-exports name their cap in another crate; the
            // declarations here are the ones this file owns.
            for kw in ["pub const ", "pub static "] {
                let Some(rest) = line.strip_prefix(kw) else {
                    continue;
                };
                let Some((name, ty)) = rest.split_once(':') else {
                    continue;
                };
                let name = name.trim();
                // A cap is a count; the semaphore built from one is not a
                // second cap and has nothing of its own to publish.
                if !ty.contains("usize") {
                    continue;
                }
                // MAX_URI_BYTES → maxUriBytes
                let mut camel = String::new();
                for (i, word) in name.split('_').enumerate() {
                    let lower = word.to_lowercase();
                    if i == 0 {
                        camel.push_str(&lower);
                    } else {
                        let mut c = lower.chars();
                        if let Some(f) = c.next() {
                            camel.extend(f.to_uppercase());
                            camel.push_str(c.as_str());
                        }
                    }
                }
                if !published.contains_key(&camel) {
                    missing.push(format!("{name} (expected {camel:?})"));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "caps declared here and absent from /q/health: {missing:?}"
        );
    }

    /// A configured count is read once and never re-read, so a value that
    /// would stop delivery has no second chance to be corrected: zero mints
    /// a semaphore that admits nobody, and a per-tenant share above the
    /// width names a ceiling that can never fire. Both fall back rather
    /// than take the number as given.
    #[test]
    fn a_configured_count_falls_back_rather_than_disabling_delivery() {
        assert_eq!(count_from(None, 64, usize::MAX), 64, "unset is the default");
        assert_eq!(count_from(Some("weeks"), 64, usize::MAX), 64);
        assert_eq!(count_from(Some(""), 64, usize::MAX), 64);
        assert_eq!(
            count_from(Some("0"), 64, usize::MAX),
            64,
            "zero would admit nobody"
        );
        assert_eq!(count_from(Some("-8"), 64, usize::MAX), 64);
        assert_eq!(count_from(Some("256"), 64, usize::MAX), 256);
        assert_eq!(
            count_from(Some("512"), 8, 64),
            64,
            "a share over the width is the width"
        );
        assert_eq!(
            count_from(None, 8, 4),
            4,
            "the ceiling binds the default too"
        );
    }

    /// A guard on the compiled defaults rather than on the clamp: both
    /// counts are read once per process, so a test cannot set one without
    /// racing every other test in the binary for the read. What it holds is
    /// that the two numbers shipped stay a share and a total, and that
    /// neither default is ever lowered to zero — which would admit nobody.
    #[test]
    fn the_published_tenant_share_fits_inside_the_published_width() {
        let snap = LimitStats::default().snapshot();
        let width = snap["deliveryWidth"].as_u64().expect("a number");
        let share = snap["deliveryWidthPerTenant"].as_u64().expect("a number");
        assert!(share <= width, "share {share} exceeds width {width}");
        assert!(width > 0 && share > 0, "neither may be zero");
    }

    #[test]
    fn depth_scan_is_string_aware() {
        assert_eq!(json_depth(br#"{"a": [1, {"b": 2}]}"#), 3);
        assert_eq!(
            json_depth(br#"{"a": "}]}]}]{[{["}"#),
            1,
            "braces in strings don't count"
        );
        assert_eq!(
            json_depth(br#"{"a": "\"}"}"#),
            1,
            "escaped quotes stay in-string"
        );
        let deep = "[".repeat(100) + &"]".repeat(100);
        assert_eq!(json_depth(deep.as_bytes()), 100);
    }

    /// T-4562: batches run inside their byte budget. A second one that does
    /// not fit waits and is refused when nothing frees; a body larger than
    /// the whole budget still runs alone; a finished batch frees its share.
    #[tokio::test]
    async fn batches_wait_for_their_share_of_the_in_flight_budget() {
        let slots = tokio::sync::Semaphore::new(10);
        let wait = std::time::Duration::from_millis(50);
        let first = admit(&slots, 10, 6, wait).await.expect("fits");
        assert!(
            admit(&slots, 10, 6, wait).await.is_none(),
            "6 + 6 is past 10"
        );
        let small = admit(&slots, 10, 4, wait).await.expect("6 + 4 fits");
        drop((first, small));
        let huge = admit(&slots, 10, 1 << 30, wait)
            .await
            .expect("alone, a huge body runs");
        assert!(
            admit(&slots, 10, 0, wait).await.is_none(),
            "an empty body still counts one"
        );
        drop(huge);
        assert!(admit(&slots, 10, 0, wait).await.is_some());
        assert_eq!(slots.available_permits(), 10, "every share came back");
    }

    /// T-4624: small batches each fit the byte budget, yet only as many run
    /// as there are slots, so the rest of the pool stays with the reads; a
    /// batch refused a slot gives its byte share back.
    #[tokio::test]
    async fn only_so_many_batches_run_at_once_however_small() {
        let (bytes, running) = (
            tokio::sync::Semaphore::new(1000),
            tokio::sync::Semaphore::new(2),
        );
        let wait = std::time::Duration::from_millis(50);
        let admit = || admit_both((&bytes, 1000), (&running, 2), 10, wait);
        let first = admit().await.expect("a slot");
        let second = admit().await.expect("a slot");
        assert!(
            admit().await.is_none(),
            "two run, the third waits and is refused"
        );
        assert_eq!(
            bytes.available_permits(),
            980,
            "the refused one's share came back"
        );
        drop(first);
        let third = admit().await.expect("a freed slot");
        drop((second, third));
        assert_eq!(
            (bytes.available_permits(), running.available_permits()),
            (1000, 2)
        );
    }
}
