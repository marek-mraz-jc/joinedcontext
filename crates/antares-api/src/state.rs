// SPDX-License-Identifier: EUPL-1.2
//! Shared application state.

use antares_jsonld::Loader;
#[cfg(feature = "test-kit")]
use antares_sql::store::{any::AnyStore, Store};
use antares_store::{CurrentStateDriver, TemporalDriver};
use std::sync::Arc;
// Clock rule: std Instant panics on wasm32.
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

/// ANTARES_TEMPORAL_RECORD — the auto-recording gate on the write path.
/// Direct temporal-API writes (POST /temporal/entities…) are never gated:
/// this decides what the ENTITY endpoints leave behind as history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemporalRecord {
    /// `all` (default): every changed attribute instance.
    All,
    /// `observed`: only instances carrying `observedAt` — the spec's own
    /// measurement axis (4.5.7); metadata-shaped writes leave no history.
    Observed,
    /// `none`: nothing is auto-recorded; the temporal endpoints still serve
    /// what the temporal API was given directly (unlike ANTARES_TEMPORAL=none,
    /// which turns the temporal seam off entirely).
    None,
    /// `ruled` (T-5056, DM-97): history is opt-in. An Entity records only
    /// when its own `historyRetention`, the retention rule of one of its
    /// types or its Tenant's `*` rule gives a duration or `forever`; a type
    /// no rule names keeps its current state only.
    Ruled,
}

impl std::str::FromStr for TemporalRecord {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "all" => Ok(Self::All),
            "observed" => Ok(Self::Observed),
            "none" => Ok(Self::None),
            "ruled" => Ok(Self::Ruled),
            other => Err(format!(
                "ANTARES_TEMPORAL_RECORD: unknown mode {other} (all|observed|none|ruled)"
            )),
        }
    }
}

/// Behaviour profile (T-5497, docs/conformance/profiles.md): where the two
/// spec versions or the conformance suites accept two answers, the one this
/// deployment gives. Startup configuration only, never per request; the
/// default of every member is the behaviour the pinned ETSI suite asserts.
/// No member relaxes authorization, tenant isolation or an input limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    /// `ANTARES_NON_OBJECT_BODY=bad-request-data`: a JSON body that is no
    /// object (`[]`, `[1,2,3]`) where an object is expected answers
    /// BadRequestData (TS 104 175 8.2.3 second bullet, forge all-fixes,
    /// Coraine) instead of InvalidRequest (ETSI 001_02_02).
    pub non_object_bad_request_data: bool,
    /// `ANTARES_FED_WRITE_SOURCE_404=ignore`: on a distributed write, a
    /// source's 404 beside a success counts as "no data" (204, forge
    /// all-fixes, Coraine) instead of a part of the 207 (ETSI D002_02_*_inc).
    pub fed_write_ignore_404: bool,
    /// `ANTARES_IMPLICIT_TENANTS=listed`: only the operations TS 104 175
    /// 7.8.2 lists create a Tenant; Batch Upsert on an unknown Tenant
    /// answers NonexistentTenant (Coraine tenant_implicit_create).
    pub implicit_tenants_listed_only: bool,
}

impl Profile {
    /// Reads the profile keys; an unknown value is a startup error.
    pub fn from_env() -> Result<Self, String> {
        let get = |k: &str| std::env::var(k).ok();
        Self::from_vars(
            get("ANTARES_NON_OBJECT_BODY"),
            get("ANTARES_FED_WRITE_SOURCE_404"),
            get("ANTARES_IMPLICIT_TENANTS"),
        )
    }

    /// [`Profile::from_env`] over given values (unset = `None`).
    pub fn from_vars(
        non_object_body: Option<String>,
        fed_write_source_404: Option<String>,
        implicit_tenants: Option<String>,
    ) -> Result<Self, String> {
        Ok(Self {
            non_object_bad_request_data: profile_choice(
                "ANTARES_NON_OBJECT_BODY",
                non_object_body,
                ["invalid-request", "bad-request-data"],
            )?,
            fed_write_ignore_404: profile_choice(
                "ANTARES_FED_WRITE_SOURCE_404",
                fed_write_source_404,
                ["report", "ignore"],
            )?,
            implicit_tenants_listed_only: profile_choice(
                "ANTARES_IMPLICIT_TENANTS",
                implicit_tenants,
                ["with-upsert", "listed"],
            )?,
        })
    }
}

/// `false` for the default value (or unset/empty), `true` for the other.
fn profile_choice(
    key: &str,
    value: Option<String>,
    [default, other]: [&str; 2],
) -> Result<bool, String> {
    match value.as_deref().map(str::trim) {
        None | Some("") => Ok(false),
        Some(v) if v == default => Ok(false),
        Some(v) if v == other => Ok(true),
        Some(v) => Err(format!("{key}: unknown value {v:?} ({default}|{other})")),
    }
}

/// History gate 3's set (T-4275): the (Tenant, expanded type) pairs that
/// record no history, shared by every clone of the state.
pub type HistoryOff = Arc<std::sync::RwLock<HistoryRules>>;

/// When an attribute recorded `onChange` writes a row (T-4665, platform
/// ADR-N-054 2.2): when its value moved past `deadband` (a number) or
/// `min_distance` metres (a Point), or differs at all (anything else), or
/// `heartbeat` passed since its last row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OnChange {
    pub heartbeat: std::time::Duration,
    pub deadband: Option<f64>,
    pub min_distance: Option<f64>,
}

/// How an attribute records history. Unset, it is `every` (a row per
/// changed instance), and `observed` under `ANTARES_TEMPORAL_RECORD=ruled`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecordMode {
    /// `every`: a row per changed instance.
    Every,
    /// `never` (T-5056): no row, for a bookkeeping attribute such as
    /// `dateModified`.
    Never,
    /// `onChange` (T-4665): a row when the value moved or the heartbeat passed.
    OnChange(OnChange),
    /// `observed` (T-5056): a row only for an instance whose `observedAt` is
    /// newer than the attribute's last row, so a value re-sent with its old
    /// `observedAt`, or sent with none, adds nothing.
    Observed,
}

/// How one type records history, for the type and per expanded attribute:
/// `None` takes the broker's default ([`RecordMode`]).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Recording {
    pub default: Option<RecordMode>,
    pub attributes: std::collections::HashMap<String, Option<RecordMode>>,
}

impl Recording {
    /// How `attr` records: its own setting, else the type's.
    #[must_use]
    pub fn of(&self, attr: &str) -> Option<RecordMode> {
        self.attributes.get(attr).copied().unwrap_or(self.default)
    }

    /// Whether some attribute records `onChange`.
    #[must_use]
    pub fn has_on_change(&self) -> bool {
        std::iter::once(&self.default)
            .chain(self.attributes.values())
            .any(|m| matches!(m, Some(RecordMode::OnChange(_))))
    }
}

/// What the write path needs of the Tenants' history retention (T-4275,
/// DM-97): which writes record none.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct HistoryRules {
    /// (Tenant, type) pairs whose rule says `history: "off"`.
    pub off: std::collections::HashSet<(String, String)>,
    /// (Tenant, type) pairs with a history rule of their own (a duration,
    /// `off` or `forever`), which their Tenant's `*` rule does not reach.
    pub ruled: std::collections::HashSet<(String, String)>,
    /// Tenants whose `*` rule says `history: "off"`.
    pub space_off: std::collections::HashSet<String>,
    /// Tenants whose `*` rule gives a duration or `forever`: under
    /// `ANTARES_TEMPORAL_RECORD=ruled` every type without a rule of its own
    /// records (T-5056).
    pub space_on: std::collections::HashSet<String>,
    /// (Tenant, type) pairs whose rule records some attribute other than
    /// `every`.
    pub recording: std::collections::HashMap<(String, String), Recording>,
}

impl HistoryRules {
    /// No rule turns any history off.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.off.is_empty() && self.space_off.is_empty()
    }
}

/// The Tenants the store recently answered "exists" for (T-5500). The 5.5.10
/// wall asks about the request's Tenant ahead of every operation that
/// names one, and nearly every answer is "exists", so a name it holds skips
/// the store for [`KnownTenants::TTL`]. It keeps positive answers only: a
/// write can create a Tenant at any moment, so a "does not exist" is never
/// kept. A purge on this process forgets the name at once; a purge on
/// another process sharing the store is seen once the entry ages out.
#[derive(Debug, Default)]
pub struct KnownTenants(std::sync::Mutex<std::collections::HashMap<String, Instant>>);

impl KnownTenants {
    /// How long a remembered "exists" stands in for the store's answer.
    pub const TTL: std::time::Duration = std::time::Duration::from_secs(30);
    /// The most names held at once (ADR-0001's 10 000-tenant target): a
    /// full map takes no new name until its old entries age out.
    pub const CAP: usize = 10_000;

    fn map(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, Instant>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Did the store say this Tenant exists within the last [`Self::TTL`]?
    #[must_use]
    pub fn holds(&self, tenant: &antares_model::TenantId) -> bool {
        self.map()
            .get(tenant.as_str())
            .is_some_and(|at| at.elapsed() < Self::TTL)
    }

    /// The store just said this Tenant exists.
    pub fn remember(&self, tenant: &antares_model::TenantId) {
        let mut map = self.map();
        if map.len() >= Self::CAP && !map.contains_key(tenant.as_str()) {
            map.retain(|_, at| at.elapsed() < Self::TTL);
            if map.len() >= Self::CAP {
                return;
            }
        }
        map.insert(tenant.as_str().to_owned(), Instant::now());
    }

    /// The Tenant was purged: the next request asks the store again.
    pub fn forget(&self, tenant: &antares_model::TenantId) {
        self.map().remove(tenant.as_str());
    }
}

/// The handler `wire` installs for a notification that arrives on the
/// internal distributed-subscription endpoint (5.8.1.4).
pub type CsourceNotification = Arc<
    dyn for<'a> Fn(
            &'a AppState,
            &'a antares_model::TenantId,
            &'a str,
            Option<&'a str>,
            &'a [serde_json::Value],
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>
        + Send
        + Sync,
>;

/// The request headers an in-process call carries from its caller
/// ([`AppState::call`]): the two that select WHICH data the operation runs
/// against, and the one that says what its terms mean. Everything else is
/// the façade's own to set — an inner request is not the outer one.
static PROPAGATED: [axum::http::HeaderName; 3] = [
    axum::http::HeaderName::from_static("ngsild-tenant"),
    axum::http::HeaderName::from_static("ngsild-snapshot"),
    axum::http::HeaderName::from_static("link"),
];

#[cfg(feature = "views")]
tokio::task_local! {
    /// The @context an in-process caller names for its request
    /// ([`AppState::call_in_context`]). It takes the place of the `Link`
    /// header, which the outer client chose and the caller cannot vouch for.
    static INPROCESS_CONTEXT: Arc<serde_json::Value>;
}

/// The @context of the in-process call this task is inside, if its caller
/// named one.
#[cfg(feature = "views")]
pub(crate) fn inprocess_context() -> Option<Arc<serde_json::Value>> {
    INPROCESS_CONTEXT.try_with(Arc::clone).ok()
}

/// How many [`AppState::call`] frames the running task is inside: zero
/// outside an in-process call. A caller that must not reach the router past
/// [`crate::bounds::MAX_IN_PROCESS_CALL_DEPTH`] reads it to answer for
/// itself rather than take the router's 500 (ADR-0025 rule 3).
#[must_use]
pub fn in_process_depth() -> usize {
    INPROCESS_DEPTH.try_with(|d| *d).unwrap_or(0)
}

tokio::task_local! {
    /// How many [`AppState::call`] frames this task is already inside, held
    /// against [`crate::bounds::MAX_IN_PROCESS_CALL_DEPTH`]. Absent outside a
    /// façade call, which reads as zero.
    pub(crate) static INPROCESS_DEPTH: usize;
}

/// The broker's state as every handler and layer receives it. Cloned at
/// every middleware layer and extractor of every request, so a clone is
/// three reference counts: the per-process part sits behind one `Arc`
/// ([`Shared`], read through `Deref`), and only what a narrowed state
/// changes, the view and its in-process router memo, is held per state.
/// Writing a shared field (`DerefMut`) copies the shared part when another
/// state holds it, so a write made before serving is never seen by a state
/// cloned earlier, as it was when every field was copied.
#[derive(Clone)]
pub struct AppState {
    shared: Arc<Shared>,
    /// The endpoint view every operation through this state is narrowed to
    /// (ADR-0023); `None` serves the NGSI-LD API as it is.
    #[cfg(feature = "views")]
    pub view: Option<Arc<crate::views::CompiledView>>,
    /// The router [`AppState::call`] serves in-process requests through,
    /// built on first use. Empty until a façade actually calls: building it
    /// costs about 1.5 ms, which is worth memoizing per state and not worth
    /// paying in a host that never makes an in-process call.
    pub(crate) inbound: Arc<std::sync::OnceLock<axum::Router>>,
}

/// The per-process part of [`AppState`]: every handle and setting a state
/// and its narrowed copies share.
#[derive(Clone)]
pub struct Shared {
    pub store: Arc<dyn CurrentStateDriver>,
    /// The temporal driver — by default the same backend instance as
    /// `store`; a deployment may load a different one (or none).
    pub temporal: Arc<dyn TemporalDriver>,
    /// What the current-state driver is CALLED, reported by `/q/health` (NOT
    /// by `/info/sourceIdentity`, which is a spec resource) — a name, not an
    /// enumeration: a driver from outside this workspace mounts the same way
    /// as one from inside, and nothing here branches on it.
    pub store_name: String,
    /// The temporal backend `/q/health` names: the store's own mode when one
    /// instance serves both seams, `None` when history is off (`NoTemporal`);
    /// a second backend overwrites it after construction.
    pub temporal_name: Option<String>,
    pub loader: Arc<Loader>,
    pub started: Instant,
    /// Startup timestamp (createdAt of the built-in core @context entry).
    pub started_at: String,
    pub host_alias: String,
    /// Default page size for queries without an explicit limit.
    pub default_limit: usize,
    /// Hard ceiling on limit (TooManyResults guard).
    pub max_limit: usize,
    /// 6.3.10: the most instances of one Attribute a temporal response
    /// serves ([`crate::bounds::temporal_instance_limit`]); the ETSI suite's
    /// brokers and its unit cases run with 9.
    pub temporal_instance_limit: usize,
    /// The instances of one Attribute served when no lastN/firstN is asked
    /// ([`crate::bounds::temporal_default_instances`]), at most the ceiling.
    pub temporal_default_instances: usize,
    /// The most bytes of one in-process answer a handler buffers
    /// ([`crate::bounds::MAX_ANSWER_BYTES`]).
    pub max_answer_bytes: usize,
    /// The bytes every concurrently buffered answer holds together, from
    /// the memory budget ([`crate::bounds::MemoryBudget::answers`]).
    pub answer_pool: Arc<crate::bounds::AnswerPool>,
    /// The charged bytes of ImplicitlyCreated @contexts one Tenant may hold
    /// ([`crate::bounds::MemoryBudget::implicit_contexts`]).
    pub implicit_context_bytes: u64,
    /// One shared outbound client, timeouts set at construction.
    pub http: antares_jsonld::HttpClient,
    /// Federation-forwarding client — longer deadline: the ETSI mock replies
    /// to unstubbed forwards only when the robot side wakes (up to ~5 s).
    pub fed_http: antares_jsonld::HttpClient,
    /// Notification bindings by `endpoint.uri` scheme (6.3.8, 7.2). The
    /// only way a delivery transport is chosen; `with_sink` adds one.
    pub sinks: Arc<antares_notifier::SinkRegistry>,
    /// HTTP surfaces mounted outside the NGSI-LD API root, each under its
    /// own reserved prefix; `with_surface` adds one. The admin surface is
    /// here by default.
    pub surfaces: Arc<Vec<Box<dyn crate::ApiSurface>>>,
    /// The transport every federation forward asks before the socket
    /// (ADR-0025 rule 8): shared by every clone, so a state made before the
    /// transport was registered (the one an `Ops` holds) forwards through it
    /// too. Empty in vanilla: every forward is HTTP.
    pub(crate) forward_transport: Arc<std::sync::OnceLock<Arc<dyn crate::ForwardTransport>>>,
    /// The views this pod serves (ADR-0023 rule 10), shared by every clone.
    #[cfg(feature = "views")]
    pub views: Arc<crate::views::ViewTable>,
    /// Bounds-wall rejection counters (exported by /q/health).
    pub limits: Arc<crate::bounds::LimitStats>,
    /// `GET /q/tenants/{tenant}` sizes by tenant with the instant they were
    /// measured: a size is a scan of the tenant, so it is reused for
    /// [`crate::TENANT_BYTES_TTL`] (T-2890). One entry per existing tenant
    /// asked about; a purge drops its entry.
    pub tenant_bytes: Arc<std::sync::Mutex<std::collections::HashMap<String, crate::BytesEntry>>>,
    /// The 5.5.10 wall's memory of Tenants that exist (T-5500).
    pub known_tenants: Arc<KnownTenants>,
    /// Allocator stats provider (set by the broker; None in tests/wasm).
    pub mem_stats: Option<Arc<dyn Fn() -> serde_json::Value + Send + Sync>>,
    /// Bus state provider for /q/health (`bus: {mode, connected,
    /// reconnects}`) — installed by the nats wiring only, so the member is
    /// absent for bus=local.
    pub bus_stats: Option<Arc<dyn Fn() -> serde_json::Value + Send + Sync>>,
    /// One egress policy for notifications and federation forwards
    /// (scheme allowlist, private-range deny, per-destination breakers).
    pub egress: Arc<crate::egress::Egress>,
    /// The policy engine every operation is asked about (ADR-0020), or
    /// `None` when a deployment attached none with
    /// [`AppState::with_policy`] — the default, where the broker behaves
    /// exactly as it did before the seam existed. `None` is not an instance
    /// of `AllowAll`: the gate takes its answer without building a subject,
    /// boxing a future or arming a timer, so a broker with no engine pays
    /// nothing for the seam.
    pub policy: Option<Arc<dyn crate::policy::PolicyEngine>>,
    /// The `/q` admin credential ([`AppState::with_admin_auth`]); `None`
    /// serves `/q` without one, as the broker did before T-3569.
    pub admin_auth: Option<Arc<crate::admin_auth::AdminAuth>>,
    /// Set the moment SIGTERM arrives, BEFORE the listener stops
    /// accepting — `/q/health` then answers 503 DRAINING so the load balancer
    /// takes this instance out while its socket still works.
    pub draining: Arc<std::sync::atomic::AtomicBool>,
    /// Change batches the matcher queue accepted and has not finished
    /// delivering. A shutdown drain waits for zero before the pool closes.
    pub pending_changes: Arc<std::sync::atomic::AtomicUsize>,
    /// Called after every Subscription and Context Source Registration
    /// Subscription CUD, so the mirror the matcher reads follows the store.
    /// On the bus the wiring pushes the change into the KV mirror bucket;
    /// in local mode it applies to this process's own mirror.
    #[allow(clippy::type_complexity)] // a boxed callback, named where it is installed
    pub sub_sync: Option<
        Arc<
            dyn Fn(&antares_model::TenantId, antares_store::Kind, &str, Option<&serde_json::Value>)
                + Send
                + Sync,
        >,
    >,
    /// bus=nats: the KV-watched compiled-subscription mirror the matcher
    /// reads, so the hot path never touches Postgres. `None` in local
    /// mode (the matcher reads the store directly).
    pub sub_mirror: Option<Arc<crate::mirror::SubMirror>>,
    /// bus=local: takes the entity changes one request buffered (the history
    /// layer hands them over after the handler) so the matcher sees a batch
    /// request as one unit. `None` until the local pipeline is wired.
    pub change_flush: Option<Arc<dyn Fn(Vec<crate::mirror::Change>) + Send + Sync>>,
    /// Called after every Registration CUD: bus=nats publishes the delta on
    /// `ANTARES_REGISTRY`, bus=local applies it to `reg_mirror` in process.
    /// `None` while no mirror is wired.
    #[allow(clippy::type_complexity)] // a boxed callback, named where it is installed
    pub reg_sync: Option<
        Arc<dyn Fn(&antares_model::TenantId, &str, Option<&serde_json::Value>) + Send + Sync>,
    >,
    /// The ONE per-process compiled registration mirror, delta-fed from
    /// `ANTARES_REGISTRY` (bus=nats) or by `reg_sync` (bus=local); expiry
    /// stays filtered at the single yield point (`federation::matching_regs`).
    /// `None` until wired or when its hydrate failed: the store is read then.
    pub reg_mirror: Option<Arc<crate::mirror::DocMirror>>,
    /// 5.2.34 (bus=nats): shares a cooldown stamp with the other api pods —
    /// a per-process stamp re-dials a failed source from every pod behind
    /// the LB. Seconds-scale state: broadcast on the
    /// registry stream, deliberately not persisted. `None` in local mode.
    #[allow(clippy::type_complexity)] // a boxed callback, named where it is installed
    pub reg_fail_sync: Option<Arc<dyn Fn(&str, bool) + Send + Sync>>,
    /// Renders the Prometheus text format for /q/metrics. Installed by
    /// the broker (the only crate that knows an exporter exists);
    /// `None` = 404, the facade calls elsewhere stay no-ops.
    pub metrics_render: Option<Arc<dyn Fn() -> String + Send + Sync>>,
    /// 5.8.1.4 consumer half: a notification addressed to the internal
    /// `urn:antares:distsub:` endpoint never leaves the broker, it re-enters
    /// it. The delivery path hands it here instead of naming the module that
    /// owns distributed subscriptions; `wire` installs the handler. `None`
    /// drops such a notification, which is what a broker that never created
    /// an internal subscription should do with one.
    pub csource_notification: Option<CsourceNotification>,
    /// True only under bus=nats (set by the broker's wiring): multiple
    /// processes share the store, so interval-subscription firings must be
    /// claimed single-winner. bus=local keeps the direct path — a
    /// claim there would disturb the 046_12 bookkeeping ordering for
    /// nothing.
    pub nats: bool,
    /// History gate 2 (ANTARES_TEMPORAL_RECORD): which changed attribute
    /// instances the write path records into history. Default `All`, which
    /// the ETSI temporal suites assume.
    pub temporal_record: TemporalRecord,
    /// Behaviour profile keys ([`Profile`]); the broker sets it from the env.
    pub profile: Profile,
    /// The mesh identity of the gateway whose provenance headers the broker
    /// believes (ANTARES_PROVENANCE_FROM, T-4665); `None` believes nobody.
    pub provenance_from: Option<String>,
    /// A request to the NGSI-LD API that carries no subject header is served
    /// only with the admin bearer token (T-5032,
    /// [`crate::admin_auth::subject_or_admin`]). `false` (the default)
    /// serves it as before; the platform build sets it when the deployment
    /// names subject headers.
    pub subject_required: bool,
    /// History gate 3 (T-4275): the (Tenant, expanded type) pairs whose
    /// retention rule says `history: "off"`. The views reload replaces the
    /// set; an Entity with any of these types records no attribute instance.
    pub history_off: HistoryOff,
    /// The base URL remote Context Sources reach THIS broker at — used as
    /// the notification endpoint of forwarded subscription copies
    /// (5.8.1.4). ANTARES_PUBLIC_URL, defaulting to
    /// http://{host_alias}:{ANTARES_HTTP_PORT} (portless when 80/unset).
    pub public_url: String,
    /// 5.5.15 resource-pressure signal: max snapshots per tenant — above it
    /// the lowest-snapshotPriority snapshots are evicted. Snapshot documents
    /// themselves live in the store (Kind::Snapshot) so persistent modes
    /// survive restarts.
    pub snapshot_cap: usize,
    /// How a notification is delivered: attempts, backoff, age ceiling.
    /// Default = one attempt (5.8.6 as written).
    pub delivery: antares_notifier::DeliveryPolicy,
}

impl std::ops::Deref for AppState {
    type Target = Shared;

    fn deref(&self) -> &Shared {
        &self.shared
    }
}

impl std::ops::DerefMut for AppState {
    fn deref_mut(&mut self) -> &mut Shared {
        // the memoised in-process router captured the settings about to
        // change: a later call must build one over the new ones
        self.inbound = Arc::new(std::sync::OnceLock::new());
        Arc::make_mut(&mut self.shared)
    }
}

impl AppState {
    /// An in-process answer read whole, under `max_answer_bytes` and
    /// charged to `answer_pool` until the returned value drops
    /// (T-5895).
    ///
    /// # Errors
    /// [`crate::bounds::AnswerRefusal`]: over the per-answer bound, no room in
    /// the pool, or a failed body.
    pub async fn read_answer(
        &self,
        body: axum::body::Body,
        surface: &'static str,
    ) -> Result<crate::bounds::HeldAnswer, crate::bounds::AnswerRefusal> {
        crate::bounds::read_answer(&self.answer_pool, body, self.max_answer_bytes, surface).await
    }

    /// This state's broker, told apart from the other brokers of the
    /// process: its narrowed copies share it, and no live broker shares it
    /// with another.
    pub(crate) fn broker(&self) -> usize {
        Arc::as_ptr(&self.shared).addr()
    }

    /// Whether `other` is this state: the same broker and the same view.
    #[must_use]
    pub fn same(&self, other: &Self) -> bool {
        #[cfg(feature = "views")]
        let view = match (&self.view, &other.view) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        };
        #[cfg(not(feature = "views"))]
        let view = true;
        Arc::ptr_eq(&self.shared, &other.shared) && view
    }

    /// The built-in store: in-memory, or — under the reserved harness
    /// variable `ANTARES_TEST_STORE=file` — a fresh on-disk redb store per
    /// state, so the same test binary proves the durable backend without a
    /// second copy of every test. The broker's own boot path never calls
    /// this; it composes from `ANTARES_STORE` in `with_drivers`.
    #[cfg(feature = "test-kit")]
    pub fn new(host_alias: String) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        if std::env::var("ANTARES_TEST_STORE").as_deref() == Ok("file") {
            let dir = std::env::temp_dir()
                .join("antares-test-store")
                .join(uuid::Uuid::new_v4().simple().to_string());
            // reachable only through the harness variable read above; a
            // harness that cannot open its own store has nothing to run
            #[allow(clippy::expect_used)]
            let store = Store::open_file(&dir).expect("open the redb test store");
            return Self::with_store(host_alias, Arc::new(AnyStore::Mem(store)), "file");
        }
        Self::with_store(
            host_alias,
            Arc::new(AnyStore::Mem(Store::default())),
            "memory",
        )
    }

    /// Convenience over the built-in backends: one `AnyStore` serves as
    /// both drivers.
    #[cfg(feature = "test-kit")]
    pub fn with_store(host_alias: String, store: Arc<AnyStore>, store_name: &str) -> Self {
        let temporal: Arc<dyn TemporalDriver> = store.clone();
        Self::with_drivers(host_alias, store, temporal, store_name)
    }

    pub fn with_drivers(
        host_alias: String,
        store: Arc<dyn CurrentStateDriver>,
        temporal: Arc<dyn TemporalDriver>,
        store_name: &str,
    ) -> Self {
        // One policy value, read once, shared by every outbound path —
        // the gate (scheme/breakers) and the clients (DNS pinning, redirect
        // cap) can never disagree about what is allowed.
        let egress_policy = antares_jsonld::EgressPolicy::from_env();
        // Cached-@context rows are the ONE source of truth for 5.13
        // existence — wire the write-through HERE so every composition
        // (native binary, wasm, tests) gets it; a composition that forgets
        // the writer is the "expiry checked in some paths" disease with
        // @contexts instead of expiry.
        let loader = Arc::new(Loader::new());
        {
            let store = store.clone();
            loader.set_cache_writer(Arc::new(move |tenant, url, ctx_value| {
                let store = store.clone();
                Box::pin(async move {
                    if hosted_row_id(&*store, tenant, url).await.is_some() {
                        return; // broker-local (Hosted/Implicit) URLs are not Cached entries
                    }
                    let id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, url.as_bytes());
                    // A refetch (staleness, delete+reload) must keep the row's
                    // identity and hit counters — only the body is new — and
                    // it swaps the body under the row lock, so a bump racing
                    // it keeps its count. Only a row that is not there is
                    // written whole.
                    // The row this writes is `Cached` and belongs to no Tenant
                    // (ADR-0021), but the call still acts for the Tenant whose
                    // resolution triggered the fetch: that is what lets the guard
                    // above see this Tenant's own Hosted rows.
                    let id = id.to_string();
                    let body = serde_json::json!({"@context": ctx_value});
                    let refetched = store
                        .context_mutate(
                            tenant,
                            &id,
                            Box::new(|row| {
                                row.as_object_mut()
                                    .ok_or(())?
                                    .insert("body".into(), body.clone());
                                Ok(())
                            }),
                        )
                        .await;
                    if matches!(refetched, Ok(Some(Ok(())))) {
                        return;
                    }
                    let doc = serde_json::json!({
                        "url": url,
                        "localId": id,
                        "kind": "Cached",
                        "createdAt": now_iso(),
                        "numberOfHits": 0,
                        "lastUsage": null,
                        "body": body,
                    });
                    if let Err(e) = store.context_put(tenant, &id, doc).await {
                        // a client-named @context URL may carry userinfo
                        tracing::warn!(
                            "@context write-through failed for {}: {e}",
                            antares_notifier::redact_userinfo(url)
                        );
                    }
                })
            }));
        }
        // 5.13.3.5: hit counters live in the SHARED row, not per instance
        // — behind a load balancer per-instance counters split-brain.
        // A bump that finds the row gone reports
        // a cross-instance delete; the loader then drops its warm copies so
        // the delete is honoured everywhere (5.13.5.4). Pinned core contexts
        // have no row and are never evicted.
        {
            let store = store.clone();
            loader.set_usage_bump(Arc::new(move |tenant, url| {
                let store = store.clone();
                Box::pin(async move {
                    if Loader::is_pinned_core(url) {
                        return true;
                    }
                    let id = match hosted_row_id(&*store, tenant, url).await {
                        Some(id) => id,
                        None => uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, url.as_bytes())
                            .to_string(),
                    };
                    // under the row lock: a bump racing a delete leaves the row
                    // deleted, and two racing bumps both count
                    let bumped = store
                        .context_mutate(
                            tenant,
                            &id,
                            Box::new(|row| {
                                let o = row.as_object_mut().ok_or(())?;
                                let hits = o
                                    .get("numberOfHits")
                                    .and_then(serde_json::Value::as_u64)
                                    .unwrap_or(0)
                                    .saturating_add(1);
                                o.insert("numberOfHits".into(), serde_json::json!(hits));
                                o.insert("lastUsage".into(), serde_json::json!(now_iso()));
                                Ok(())
                            }),
                        )
                        .await;
                    match bumped {
                        Ok(Some(Ok(()))) => true,
                        // a row that cannot carry the counters is left
                        // uncounted, and still exists: it must not be evicted
                        Ok(Some(Err(()))) => true,
                        Ok(None) => false,
                        // a store hiccup must never evict a healthy cache entry
                        Err(e) => {
                            tracing::warn!(
                                "@context hit bump failed for {}: {e}",
                                antares_notifier::redact_userinfo(url)
                            );
                            true
                        }
                    }
                })
            }));
        }
        // 5.13.1: an @context this broker HOSTS is served from its row, not
        // fetched. Its warm copy in the loader is only a cache — a restart
        // reloads Cached rows alone, and the bounded cache may evict it at
        // any moment — and the URL under it is minted from the request Host
        // header, so a miss would otherwise send the broker to whatever
        // address a client put there for the term mappings of its own
        // Tenant's payloads. The row carries the owner, so 5.5.10 still
        // decides who may resolve it.
        {
            let store = store.clone();
            loader.set_local_lookup(Arc::new(move |tenant, url| {
                let store = store.clone();
                Box::pin(async move {
                    // A miss for a named Tenant still asks the DEFAULT one: a
                    // fetch of this URL would reach the serve endpoint with no
                    // NGSILD-Tenant, get the default Tenant's row and cache it
                    // for everyone (F-ANT4-002). Found here, it keeps its owner.
                    let default = antares_model::TenantId::default();
                    let (_, row) = match hosted_row(&*store, tenant, url).await {
                        Some(found) => found,
                        None if tenant.is_some_and(|t| *t != default) => {
                            hosted_row(&*store, Some(&default), url).await?
                        }
                        None => return None,
                    };
                    // Ownership is one rule for the whole broker (ADR-0021): a
                    // Cached row is a copy of a public document and belongs to no
                    // Tenant, everything else belongs to its `owner` — with the
                    // DEFAULT Tenant for a row written before that member
                    // existed, which is the Tenant that lists, serves and deletes
                    // it.
                    let owner = antares_store::context_row_owner(&row)
                        .and_then(|o| antares_model::TenantId::new_internal(o).ok());
                    Some((
                        owner,
                        row.pointer("/body/@context").cloned().unwrap_or_default(),
                    ))
                })
            }));
        }
        // 5.8.1.4: this URL is handed to peer brokers as the notification
        // endpoint for distributed subscriptions — the default must carry
        // the HTTP port or peers dial port 80 (ETSI-matrix ADV_02 shape).
        let public_url =
            std::env::var("ANTARES_PUBLIC_URL").unwrap_or_else(|_| {
                match std::env::var("ANTARES_HTTP_PORT") {
                    Ok(p) if p != "80" => format!("http://{host_alias}:{p}"),
                    _ => format!("http://{host_alias}"),
                }
            });
        let temporal_name = temporal.supported().then(|| store_name.to_owned());
        // 6.3.8 is the mandatory binding; clause 7's MQTT one is optional and
        // registers only when compiled in. Both go through the registry, so
        // notification delivery never names a transport.
        let http = outbound_client(
            egress_policy,
            std::time::Duration::from_secs(slow_factor().saturating_mul(5)),
        );
        let mut sinks = antares_notifier::SinkRegistry::default();
        sinks.register(Box::new(antares_notifier::HttpSink::new(http.clone())));
        #[cfg(feature = "mqtt")]
        sinks.register(Box::new(antares_notifier::mqtt::MqttSink::default()));
        Self {
            #[cfg(feature = "views")]
            view: None,
            inbound: Arc::new(std::sync::OnceLock::new()),
            shared: Arc::new(Shared {
                store,
                temporal,
                store_name: store_name.to_owned(),
                temporal_name,
                loader,
                started: Instant::now(),
                started_at: now_iso(),
                host_alias,
                default_limit: *crate::bounds::DEFAULT_LIMIT,
                max_limit: *crate::bounds::MAX_LIMIT,
                temporal_instance_limit: crate::bounds::temporal_instance_limit(),
                temporal_default_instances: crate::bounds::temporal_default_instances(),
                max_answer_bytes: crate::bounds::MAX_ANSWER_BYTES,
                answer_pool: crate::bounds::AnswerPool::new(
                    usize::try_from(crate::bounds::MemoryBudget::default().answers())
                        .unwrap_or(usize::MAX),
                ),
                implicit_context_bytes: crate::bounds::MemoryBudget::default().implicit_contexts(),
                http: http.clone(),
                fed_http: outbound_client(
                    egress_policy,
                    std::time::Duration::from_secs(slow_factor().saturating_mul(8)),
                ),
                sinks: Arc::new(sinks),
                forward_transport: Arc::new(std::sync::OnceLock::new()),
                #[cfg(feature = "views")]
                views: Arc::default(),
                surfaces: Arc::new(vec![Box::new(crate::Admin)]),
                limits: Arc::new(crate::bounds::LimitStats::default()),
                tenant_bytes: Arc::default(),
                known_tenants: Arc::default(),
                mem_stats: None,
                bus_stats: None,
                egress: Arc::new(crate::egress::Egress::new(egress_policy)),
                policy: None,
                admin_auth: None,
                draining: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                pending_changes: Arc::default(),
                sub_sync: None,
                sub_mirror: None,
                change_flush: None,
                reg_sync: None,
                reg_fail_sync: None,
                reg_mirror: None,
                metrics_render: None,
                csource_notification: None,
                nats: false,
                temporal_record: TemporalRecord::All,
                profile: Profile::default(),
                provenance_from: None,
                subject_required: false,
                history_off: HistoryOff::default(),
                public_url,
                snapshot_cap: 1024,
                delivery: antares_notifier::DeliveryPolicy::default(),
            }),
        }
    }

    /// Attach a policy engine (ADR-0020). One engine per broker, chosen at
    /// startup: an engine that could be swapped per request would make what
    /// a caller may see a function of when they asked. Call before the
    /// state is shared.
    #[must_use]
    pub fn with_policy(mut self, engine: Arc<dyn crate::policy::PolicyEngine>) -> Self {
        self.policy = Some(engine);
        self
    }

    /// Puts every `/q` route except health and readiness (and metrics, when
    /// `auth` leaves them open) behind `auth`'s bearer secret. Set before
    /// the router is built: the admin surface reads it when it mounts.
    #[must_use]
    pub fn with_admin_auth(mut self, auth: crate::admin_auth::AdminAuth) -> Self {
        self.admin_auth = Some(Arc::new(auth));
        self
    }

    /// This state narrowed to `view`: every operation served through the
    /// returned state, and everything it spawns or calls in-process, is
    /// composed with the view (ADR-0023). It shares the store and every
    /// other handle; the in-process router memo is its own, so a call
    /// through it never reuses a router built without the view.
    #[cfg(feature = "views")]
    pub fn with_view(&self, view: Arc<crate::views::CompiledView>) -> Self {
        let mut st = self.clone();
        st.view = Some(view);
        st.inbound = Arc::new(std::sync::OnceLock::new());
        st
    }

    /// Every stored view document's (ADR-0023 rule 10) id and digest, the
    /// bodies left in the store (T-4354).
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn view_digests(&self) -> Result<Vec<(String, u64)>, String> {
        self.store
            .list_digests(&view_tenant()?, antares_store::Kind::View)
            .await
            .map_err(|e| e.to_string())
    }

    /// The stored view document `id`, if there is one.
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn view_doc(&self, id: &str) -> Result<Option<serde_json::Value>, String> {
        self.store
            .get(&view_tenant()?, antares_store::Kind::View, id)
            .await
            .map_err(|e| e.to_string())
    }

    /// Store view document `id`; `true` when it replaced one. The caller
    /// validated it: the store keeps what it is given.
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn put_view_doc(&self, id: &str, doc: serde_json::Value) -> Result<bool, String> {
        self.store
            .upsert(&view_tenant()?, antares_store::Kind::View, id, doc)
            .await
            .map_err(|e| e.to_string())
    }

    /// Remove view document `id`; `false` when there was none.
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn delete_view_doc(&self, id: &str) -> Result<bool, String> {
        self.store
            .delete(&view_tenant()?, antares_store::Kind::View, id)
            .await
            .map_err(|e| e.to_string())
    }

    /// Every Tenant the store knows, sorted; `None` when the store keeps
    /// no inventory. For maintenance sweeps that must reach a Tenant no
    /// document names any more (F-ANT4-011).
    #[cfg(feature = "views")]
    pub async fn tenant_names(&self) -> Option<Vec<String>> {
        self.store.tenant_ids().await.ok()
    }

    /// The Entity types Tenant `t`'s live Entities carry, expanded and
    /// sorted, read from the store's type list; `None` when the store keeps
    /// none. For a surface over a view of every type (F-ANT5-010).
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn stored_type_names(
        &self,
        t: &antares_model::TenantId,
    ) -> Result<Option<Vec<String>>, String> {
        self.store
            .entity_type_names(t)
            .await
            .map_err(|e| e.to_string())
    }

    /// One page of Tenant `t`'s Entities of one of `types` (of any type when
    /// `types` is empty), each holding only
    /// `attrs` (expanded IRIs), from match `offset` on; a store that cannot
    /// page answers every match and the page is cut here. For maintenance
    /// sweeps (T-4018): no view, no policy, no read hook.
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn scan_entities(
        &self,
        t: &antares_model::TenantId,
        types: &[String],
        attrs: &[String],
        offset: usize,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, String> {
        // OR-of-AND groups (4.17): one group per type, so any of them
        // matches; no type at all reads every type
        let types: Vec<Vec<String>> = types.iter().map(|t| vec![t.clone()]).collect();
        let filter = antares_store::filter::EntityFilter {
            types: (!types.is_empty()).then_some(types.as_slice()),
            attrs: Some(attrs),
            page: Some(antares_store::filter::Page {
                offset: i64::try_from(offset).unwrap_or(i64::MAX),
                limit: i64::try_from(limit).unwrap_or(i64::MAX),
                count: false,
            }),
            ..antares_store::filter::EntityFilter::default()
        };
        let found = self
            .store
            .query_entities(t, &filter)
            .await
            .map_err(|e| e.to_string())?;
        Ok(if found.paged {
            found.rows
        } else {
            found.rows.into_iter().skip(offset).take(limit).collect()
        })
    }

    /// Change each of Tenant `t`'s Entities `ids` under its row lock: `f`
    /// edits the stored document and answers whether it changed it, and
    /// only those are written, in place, as maintenance (no `modifiedAt`,
    /// no history, no notification). The answer holds, per id, whether it
    /// was written (`false` for an id that is gone).
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn mutate_entities(
        &self,
        t: &antares_model::TenantId,
        ids: &[String],
        mut f: impl FnMut(&str, &mut serde_json::Value) -> bool + Send,
    ) -> Result<Vec<bool>, String> {
        use antares_store::CurrentStateDriverExt;
        let results = self
            .store
            .batch_mutate(t, ids, |id, doc| if f(id, doc) { Ok(()) } else { Err(()) })
            .await
            .map_err(|e| e.to_string())?;
        Ok(results
            .into_iter()
            .map(|r| matches!(r, Some(Ok(()))))
            .collect())
    }

    /// Every stored export grant (ADR-0025 rule 5), as written.
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn grant_docs(&self) -> Result<Vec<serde_json::Value>, String> {
        self.store
            .list(&grant_tenant()?, antares_store::Kind::View)
            .await
            .map_err(|e| e.to_string())
    }

    /// The stored export grant `id`, if there is one.
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn grant_doc(&self, id: &str) -> Result<Option<serde_json::Value>, String> {
        self.store
            .get(&grant_tenant()?, antares_store::Kind::View, id)
            .await
            .map_err(|e| e.to_string())
    }

    /// Store export grant `id`; `true` when it replaced one. The caller
    /// validated it.
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn put_grant_doc(&self, id: &str, doc: serde_json::Value) -> Result<bool, String> {
        self.store
            .upsert(&grant_tenant()?, antares_store::Kind::View, id, doc)
            .await
            .map_err(|e| e.to_string())
    }

    /// Remove export grant `id`; `false` when there was none.
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn delete_grant_doc(&self, id: &str) -> Result<bool, String> {
        self.store
            .delete(&grant_tenant()?, antares_store::Kind::View, id)
            .await
            .map_err(|e| e.to_string())
    }

    /// Every stored policy rule document (ADR-0020 amendment), as written.
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn policy_docs(&self) -> Result<Vec<serde_json::Value>, String> {
        self.store
            .list(&policy_tenant()?, antares_store::Kind::View)
            .await
            .map_err(|e| e.to_string())
    }

    /// The stored policy rule document of `tenant`, if there is one.
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn policy_doc(&self, tenant: &str) -> Result<Option<serde_json::Value>, String> {
        self.store
            .get(&policy_tenant()?, antares_store::Kind::View, tenant)
            .await
            .map_err(|e| e.to_string())
    }

    /// Store the policy rule document of `tenant`; `true` when it replaced
    /// one. The caller bounded it and the engine took it.
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn put_policy_doc(
        &self,
        tenant: &str,
        doc: serde_json::Value,
    ) -> Result<bool, String> {
        self.store
            .upsert(&policy_tenant()?, antares_store::Kind::View, tenant, doc)
            .await
            .map_err(|e| e.to_string())
    }

    /// Remove the policy rule document of `tenant`; `false` when there was
    /// none.
    ///
    /// # Errors
    /// The store's.
    #[cfg(feature = "views")]
    pub async fn delete_policy_doc(&self, tenant: &str) -> Result<bool, String> {
        self.store
            .delete(&policy_tenant()?, antares_store::Kind::View, tenant)
            .await
            .map_err(|e| e.to_string())
    }

    /// The engine `/q/health` reports. No engine reports the built-in name:
    /// the decision an operator reads is the one the broker takes.
    pub fn policy_name(&self) -> &str {
        self.policy
            .as_ref()
            .map_or(crate::policy::BUILT_IN_NAME, |e| e.name())
    }

    /// Register one more notification binding (6.3.8). A deployment adds a
    /// sink for a scheme this workspace does not ship; endpoints naming that
    /// scheme then validate and deliver through it. Call before the state is
    /// shared — a clone already handed out keeps the registry it was made
    /// with.
    ///
    /// Not behind `test-kit`, unlike the mirror accessors beside it: this is
    /// the sink seam itself (ADR-0016), the call `docs/src/extending.md`
    /// tells a deployment to make, and `examples/plugin-example` is the host
    /// that proves it from outside. A release library without it has no
    /// notification binding seam at all.
    #[must_use]
    pub fn with_sink(mut self, sink: Box<dyn antares_notifier::NotificationSink>) -> Self {
        match Arc::get_mut(&mut self.sinks) {
            Some(reg) => reg.register(sink),
            None => tracing::warn!("sink registry already shared; binding not registered"),
        }
        self
    }

    /// Register the transport every federation forward asks before the
    /// socket (ADR-0025 rule 8, as amended by ADR-0027 section 9). A
    /// forward it answers `None` for goes over HTTP as before. Every clone
    /// of this state shares the one slot, before or after this call. The
    /// transport usually holds such a clone, so it lives as long as the
    /// state's last clone does: register it in a host that keeps its state
    /// for the life of the process, as the broker does.
    ///
    /// # Errors
    /// When a transport is already registered: a second one would leave
    /// the choice between them to registration order.
    pub fn with_forward_transport(
        self,
        transport: Arc<dyn crate::ForwardTransport>,
    ) -> Result<Self, String> {
        self.forward_transport
            .set(transport)
            .map_err(|_| "a forward transport is already registered".to_owned())?;
        Ok(self)
    }

    /// Mount one more HTTP surface (`/q`, `/x`, or below `/x`). A prefix
    /// outside those, or one that overlaps a surface already mounted, is an
    /// error the caller is expected to make fatal: a surface that could
    /// shadow a spec resource would make conformance a function of
    /// deployment configuration, and two surfaces on one prefix would leave
    /// the winner to route-matching order. Call before the state is shared.
    pub fn with_surface(mut self, s: Box<dyn crate::ApiSurface>) -> Result<Self, String> {
        crate::surface::check_prefix(s.prefix())?;
        if let Some(clash) = self
            .surfaces
            .iter()
            .find(|m| crate::surface::overlaps(m.prefix(), s.prefix()))
        {
            return Err(format!(
                "api surface {:?} claims {:?}, already served by {:?} at {:?}",
                s.name(),
                s.prefix(),
                clash.name(),
                clash.prefix()
            ));
        }
        match Arc::get_mut(&mut self.surfaces) {
            Some(v) => v.push(s),
            None => return Err("api surfaces already shared; register before serving".into()),
        }
        Ok(self)
    }

    /// Replace the mounted surfaces with a deployment's own selection —
    /// what a binary reads out of its configuration, rather than what the
    /// default mounting put there. Same prefix rules as `with_surface`, and
    /// a selection may leave admin out: `/q` is then not served at all.
    pub fn with_surfaces(mut self, list: Vec<Box<dyn crate::ApiSurface>>) -> Result<Self, String> {
        self.surfaces = Arc::new(Vec::new());
        for s in list {
            self = self.with_surface(s)?;
        }
        Ok(self)
    }

    /// Serve one request through this broker's own router, in process.
    ///
    /// This is the seam a façade for another standard (SensorThings, OGC
    /// API, WFS, OData) is built on: the façade is an [`crate::ApiSurface`]
    /// under `/x/<standard>` that translates its own request into an NGSI-LD
    /// one and calls this. There is no second data path — the inner request
    /// takes the same route as one off the socket, so negotiation, the
    /// bounds wall, tenancy, the policy seam, history and notifications all
    /// apply exactly once and exactly as they do for an NGSI-LD client.
    ///
    /// `caller` is the outer request's headers, and the ones that decide
    /// WHICH data an operation runs against are copied into the inner
    /// request when it does not set them itself:
    ///
    /// - `NGSILD-Tenant` (6.3.14) — a façade that forgot it would answer
    ///   every caller out of the default tenant, so it is not left to the
    ///   façade to remember;
    /// - `NGSILD-Snapshot` (6.3.22) — for the same reason: a façade called
    ///   inside a snapshot request must not quietly serve live data;
    /// - `Link` (6.3.5) — the `@context` the caller supplied, so a term
    ///   means the same thing on both sides of the translation;
    /// - every header `ANTARES_POLICY_SUBJECT_HEADERS` names, so the policy
    ///   engine is asked about the caller rather than about the façade.
    ///
    /// All values of a copied header are carried, never just the first: a
    /// repeated `NGSILD-Tenant` is `BadRequestData` (6.3.14), and a façade
    /// must not be the place where a repeat is laundered into a single valid
    /// value.
    ///
    /// `&self` on purpose — an inner call runs while the outer handler is
    /// suspended on it, and the router clone is an `Arc` bump over the same
    /// state. The router itself is built once per state, on the first call:
    /// building one costs about 1.5 ms, which no façade should pay per
    /// request. After that first call the state counts as shared, so
    /// `with_surface` and the other builders refuse — which is the rule
    /// they already state.
    pub async fn call(
        &self,
        caller: &axum::http::HeaderMap,
        req: axum::http::Request<axum::body::Body>,
    ) -> axum::response::Response {
        use axum::response::IntoResponse as _;
        // A façade route that translates into a request its own surface
        // serves would call itself for as long as the stack lasts, and
        // every frame builds a router. The chain is counted per task, so a
        // façade over a façade is served and a loop is not.
        let depth = in_process_depth();
        if depth >= crate::bounds::MAX_IN_PROCESS_CALL_DEPTH {
            return crate::negotiate::ApiError::from(antares_model::NgsiError::InternalError(
                format!(
                    "in-process call depth exceeded {}",
                    crate::bounds::MAX_IN_PROCESS_CALL_DEPTH
                ),
            ))
            .into_response();
        }
        let router = self.inbound.get_or_init(|| {
            // Every layer of a built router captured an `AppState` clone, so
            // the memo owns states. The clone it is built from must NOT be
            // able to reach THIS cell: a state that could would hold itself
            // alive for the life of the process and pin its store handle
            // with it — a file store would keep its lock after the host
            // dropped everything. A fresh cell means the memo of a nested
            // façade call hangs off this one and dies with it.
            let mut inner = self.clone();
            inner.inbound = Arc::new(std::sync::OnceLock::new());
            crate::router(inner)
        });
        let (mut parts, body) = req.into_parts();
        for name in &PROPAGATED {
            if parts.headers.contains_key(name) {
                continue;
            }
            for v in caller.get_all(name) {
                parts.headers.append(name.clone(), v.clone());
            }
        }
        for name in crate::policy::SUBJECT_HEADERS.iter() {
            let Ok(name) = axum::http::HeaderName::from_bytes(name.as_bytes()) else {
                continue;
            };
            if parts.headers.contains_key(&name) {
                continue;
            }
            for v in caller.get_all(&name) {
                parts.headers.append(name.clone(), v.clone());
            }
        }
        let req = axum::http::Request::from_parts(parts, body);
        match INPROCESS_DEPTH
            .scope(
                depth.saturating_add(1),
                tower::ServiceExt::oneshot(router.clone(), req),
            )
            .await
        {
            Ok(r) => r,
            // `Router`'s error is `Infallible`; the arm stays honest rather
            // than unwrapping (the workspace denies unwrap outside tests).
            Err(_) => crate::negotiate::ApiError::from(antares_model::NgsiError::InternalError(
                "the in-process router failed".into(),
            ))
            .into_response(),
        }
    }

    /// [`AppState::call`] on a task of its own, at the caller's in-process
    /// depth. A chain of calls each awaiting the next polls every frame
    /// inside the one before it, and a few federation handlers deep that
    /// is more stack than a worker thread has; on its own task each frame
    /// is polled from the scheduler, as a request over HTTP is. Dropping
    /// the returned future (a forward's timeout) aborts the task.
    ///
    /// The new task inherits no view: the call runs through `view` when one
    /// is given ([`crate::policy::through_view`]), so the engine decides a
    /// forward under a grant's view as a request through that view (T-3935),
    /// and never by the view of the request that triggered the forward.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn call_on_task(
        &self,
        view: Option<&str>,
        caller: axum::http::HeaderMap,
        req: axum::http::Request<axum::body::Body>,
    ) -> axum::response::Response {
        use axum::response::IntoResponse as _;
        /// Aborts the task when the forward waiting on it is dropped.
        struct AbortOnDrop(tokio::task::AbortHandle);
        impl Drop for AbortOnDrop {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let st = self.clone();
        let view = view.map(str::to_owned);
        let task = tokio::spawn(INPROCESS_DEPTH.scope(in_process_depth(), async move {
            match view {
                Some(id) => crate::policy::through_view(&id, st.call(&caller, req)).await,
                None => st.call(&caller, req).await,
            }
        }));
        let _abort = AbortOnDrop(task.abort_handle());
        task.await.unwrap_or_else(|_| {
            crate::negotiate::ApiError::from(antares_model::NgsiError::InternalError(
                "the in-process call failed".into(),
            ))
            .into_response()
        })
    }

    /// [`AppState::call`] with the terms of the request defined by
    /// `context` (a JSON-LD `@context` value, resolved as a body's would
    /// be) instead of a `Link` header: a surface that writes a `q` naming
    /// Attributes outside the core @context says what its names mean, and
    /// no header of the outer client changes it.
    #[cfg(feature = "views")]
    pub async fn call_in_context(
        &self,
        caller: &axum::http::HeaderMap,
        req: axum::http::Request<axum::body::Body>,
        context: Arc<serde_json::Value>,
    ) -> axum::response::Response {
        INPROCESS_CONTEXT
            .scope(context, self.call(caller, req))
            .await
    }

    /// Temporal auto-recording happens synchronously in the write path in
    /// EVERY bus mode (read-your-writes) — but only when the loaded
    /// temporal driver actually records anything.
    pub fn record_locally(&self) -> bool {
        self.temporal.supported()
    }

    /// Fire the subscription-sync hook for one written row. `kind` decides
    /// what the mirror does with it: a Subscription is indexed as a document,
    /// a Context Source Registration Subscription only wakes the interval
    /// sweep (5.11.7) — it is matched against registrations, not entities.
    pub(crate) fn sub_changed(
        &self,
        tenant: &antares_model::TenantId,
        kind: antares_store::Kind,
        id: &str,
        doc: Option<&serde_json::Value>,
    ) {
        if let Some(h) = &self.sub_sync {
            h(tenant, kind, id, doc);
        }
    }

    /// 5.2.34: stamp the per-registration cooldown locally AND on the other
    /// api pods (no-op half in local mode). Keyed per tenant — the id alone
    /// is client-chosen per tenant (5.5.10) and must not gate a neighbour.
    pub(crate) fn reg_cooldown_stamp(
        &self,
        tenant: &antares_model::TenantId,
        reg_id: &str,
        ok: bool,
    ) {
        let key = crate::egress::reg_key(tenant.as_str(), reg_id);
        self.egress.reg_record(&key, ok);
        if let Some(h) = &self.reg_fail_sync {
            // the broadcast carries the COMPOSED key; receiving pods stamp it
            // verbatim, so their lookups agree without re-deriving anything
            h(&key, ok);
        }
    }

    /// Fire the registration-delta hook (no-op in local mode).
    pub(crate) fn reg_changed(
        &self,
        tenant: &antares_model::TenantId,
        id: &str,
        doc: Option<&serde_json::Value>,
    ) {
        if let Some(h) = &self.reg_sync {
            h(tenant, id, doc);
        }
    }
}

/// The ONE outbound-client construction for this crate (timeouts at
/// construction). Title-case headers are an http1 knob and timeouts are
/// client-level knobs — both native-only; the browser's fetch supplies its
/// own transport on wasm32.
/// Outbound timeouts stretch 10× when the test binary runs under a
/// sanitizer (ANTARES_TEST_SANITIZER, set by the strict workflow):
/// ThreadSanitizer slows every thread, and 371 tests sharing one runner
/// pushed loopback forwards past the 2 s connect / 5 s total limits —
/// each run failing a different test with a 504. Production is unchanged.
pub fn slow_factor() -> u64 {
    antares_jsonld::slow_factor()
}

fn outbound_client(
    policy: antares_jsonld::EgressPolicy,
    total: std::time::Duration,
) -> antares_jsonld::HttpClient {
    let b = antares_jsonld::with_timeouts(
        antares_jsonld::client_builder(policy),
        std::time::Duration::from_secs(slow_factor().saturating_mul(2)),
        total,
    );
    // the suite's notification receiver asserts header names
    // case-sensitively ("Link", "X-Additional-Key")
    #[cfg(not(target_arch = "wasm32"))]
    let b = b.http1_title_case_headers();
    // reqwest fails to build only when the process cannot initialise its TLS
    // backend: no outbound request of any kind can be made after that
    #[allow(clippy::expect_used)]
    let c = b.build().expect("http client");
    antares_jsonld::wrap_client(c)
}

/// 5.13.1: an @context this broker HOSTS (Hosted or ImplicitlyCreated) is
/// identified by its stored row, never by the URL's shape — a peer broker or
/// an attacker can serve a document under the same resource path, and such a
/// URL is external to us (a Cached entry). The row records the URL it was
/// minted under, so that is what decides: a URL whose trailing segment names
/// a stored row but whose origin is somebody else's names a document this
/// broker does not host, and the stored row — another Tenant's, as often as
/// not — must not be read, counted or rewritten for it. Returns the local
/// row id when the URL is the one the row was minted under.
async fn hosted_row(
    store: &dyn CurrentStateDriver,
    tenant: Option<&antares_model::TenantId>,
    url: &str,
) -> Option<(String, serde_json::Value)> {
    let (_, seg) = url.rsplit_once("/ngsi-ld/v1/jsonldContexts/")?;
    let seg = seg.split(['?', '#']).next().unwrap_or(seg);
    if seg.is_empty() || seg.contains('/') {
        return None;
    }
    let row = store
        .context_get(tenant, seg)
        .await
        .ok()
        .flatten()
        .filter(|row| row["url"].as_str() == Some(url))?;
    Some((seg.to_owned(), row))
}

async fn hosted_row_id(
    store: &dyn CurrentStateDriver,
    tenant: Option<&antares_model::TenantId>,
    url: &str,
) -> Option<String> {
    hosted_row(store, tenant, url).await.map(|(id, _)| id)
}

/// Server-managed timestamp, ISO 8601 UTC with milliseconds.
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(feature = "views")]
fn view_tenant() -> Result<antares_model::TenantId, String> {
    antares_model::TenantId::new_internal(crate::views::VIEW_TENANT).map_err(|e| e.to_string())
}

#[cfg(feature = "views")]
fn grant_tenant() -> Result<antares_model::TenantId, String> {
    antares_model::TenantId::new_internal(crate::views::GRANT_TENANT).map_err(|e| e.to_string())
}

#[cfg(feature = "views")]
fn policy_tenant() -> Result<antares_model::TenantId, String> {
    antares_model::TenantId::new_internal(crate::views::POLICY_TENANT).map_err(|e| e.to_string())
}

#[cfg(test)]
mod jsonld_context_locality_5_13 {
    use super::*;

    /// Serve one @context document under `path` on a loopback port, counting
    /// fetches (the negative half: a warm copy must NOT be refetched).
    fn context_server(path: &str) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let fetches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n = fetches.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let body = r#"{"@context":{"peerTemp":"http://example.org/peerTemp"}}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/ld+json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                use std::io::{Read, Write};
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(resp.as_bytes());
            }
        });
        (format!("http://127.0.0.1:{port}{path}"), fetches)
    }

    fn fetch_count(c: &Arc<std::sync::atomic::AtomicUsize>) -> usize {
        c.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 5.13.1: a Cached @context is one this broker fetched from elsewhere.
    /// Which URLs this broker HOSTS is a property of the stored rows, not of
    /// the URL path, so a peer's @context served under the same resource path
    /// is persisted as Cached and never mistaken for a row deleted through
    /// another instance (5.13.5.4).
    #[tokio::test]
    async fn a_peer_context_url_under_the_local_path_is_cached() {
        let st = AppState::new("me".into());
        let (url, fetches) = context_server("/ngsi-ld/v1/jsonldContexts/peer-ctx");
        let user = serde_json::json!(url);
        st.loader.resolve(&user).await.expect("resolve");
        let id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, url.as_bytes()).to_string();
        let row = st
            .store
            .context_get(None, &id)
            .await
            .expect("store")
            .expect("the fetched @context is persisted as a Cached row");
        assert_eq!(row["kind"], "Cached");
        assert_eq!(row["url"], serde_json::json!(url));
        st.loader.resolve(&user).await.expect("resolve again");
        assert_eq!(
            fetch_count(&fetches),
            1,
            "a warm, still-existing @context must not be refetched"
        );
        assert_eq!(
            st.store
                .context_get(None, &id)
                .await
                .expect("store")
                .expect("row")["numberOfHits"],
            serde_json::json!(2),
            "both counted uses land on the shared row (5.13.3.5)"
        );
    }

    /// F-ANT4-002: the default Tenant's Hosted @context is not fetched for
    /// another Tenant through the broker's own (tenant-less) serve URL.
    #[tokio::test]
    async fn a_default_tenant_context_is_not_fetched_for_another_tenant() {
        let st = AppState::new("me".into());
        let owner = antares_model::TenantId::default();
        let (url, fetches) = context_server("/ngsi-ld/v1/jsonldContexts/local-d");
        st.store
            .context_put(
                Some(&owner),
                "local-d",
                serde_json::json!({
                    "url": url, "localId": "local-d", "kind": "Hosted",
                    "owner": owner.as_str(), "createdAt": now_iso(),
                    "body": {"@context": {"peerTemp": "http://example.org/peerTemp"}},
                }),
            )
            .await
            .expect("seed hosted row");
        let beta = antares_model::TenantId::new("beta").expect("tenant");
        let user = serde_json::json!(url);
        for _ in 0..2 {
            let e = st.loader.resolve_for(&beta, &user).await;
            assert!(
                matches!(e, Err(antares_model::NgsiError::LdContextNotAvailable(_))),
                "{e:?}"
            );
        }
        assert_eq!(fetch_count(&fetches), 0, "never fetched over HTTP");
        st.loader
            .resolve_for(&owner, &user)
            .await
            .expect("owner resolves");
    }

    /// F-ANT2-008: a Cached @context serves every Tenant, so a named Tenant
    /// may neither delete nor reload it; the default Tenant may.
    #[tokio::test]
    async fn only_the_default_tenant_deletes_a_cached_context() {
        let st = AppState::new("me".into());
        let (url, _) = context_server("/shared.jsonld");
        let alpha = antares_model::TenantId::new("alpha").expect("tenant");
        st.loader
            .resolve_for(&alpha, &serde_json::json!(url))
            .await
            .expect("cached");
        let path = format!(
            "/ngsi-ld/v1/jsonldContexts/{}",
            url.replace(':', "%3A").replace('/', "%2F")
        );
        let send = |method: &str, uri: String, tenant: Option<&str>| {
            let mut b = axum::http::Request::builder().method(method).uri(uri);
            if let Some(t) = tenant {
                b = b.header("NGSILD-Tenant", t);
            }
            let req = b.body(axum::body::Body::empty()).expect("request");
            tower::ServiceExt::oneshot(crate::router(st.clone()), req)
        };
        for uri in [path.clone(), format!("{path}?reload=true")] {
            let refused = send("DELETE", uri, Some("beta")).await.expect("response");
            assert_eq!(refused.status(), axum::http::StatusCode::FORBIDDEN);
        }
        let details = send("GET", format!("{path}?details=true"), None)
            .await
            .expect("response");
        assert_eq!(
            details.status(),
            axum::http::StatusCode::OK,
            "the row is kept"
        );
        let deleted = send("DELETE", path, None).await.expect("response");
        assert_eq!(deleted.status(), axum::http::StatusCode::NO_CONTENT);
    }

    /// F-ANT2-004: a Tenant purge drops its Hosted @context from the loader
    /// too, so the name recreated is not served the old term mappings.
    #[tokio::test]
    async fn a_purged_tenants_hosted_context_resolves_no_more() {
        let st = AppState::new("me".into());
        let t = antares_model::TenantId::new("purgectx").expect("tenant");
        let (url, fetches) = context_server("/ngsi-ld/v1/jsonldContexts/local-p");
        st.store
            .context_put(
                Some(&t),
                "local-p",
                serde_json::json!({
                    "url": url, "localId": "local-p", "kind": "Hosted",
                    "owner": t.as_str(), "createdAt": now_iso(),
                    "body": {"@context": {"peerTemp": "http://example.org/peerTemp"}},
                }),
            )
            .await
            .expect("seed hosted row");
        let user = serde_json::json!(url);
        st.loader
            .resolve_for(&t, &user)
            .await
            .expect("owner resolves");
        let req = |m: &str, uri: &str, body: &str| {
            axum::http::Request::builder()
                .method(m)
                .uri(uri)
                .header("NGSILD-Tenant", "purgectx")
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(axum::body::Body::from(body.to_owned()))
                .expect("request")
        };
        let entity = r#"{"id": "urn:ngsi-ld:Room:p", "type": "Room"}"#;
        let made = tower::ServiceExt::oneshot(
            crate::router(st.clone()),
            req("POST", "/ngsi-ld/v1/entities", entity),
        )
        .await
        .expect("response");
        assert_eq!(made.status(), axum::http::StatusCode::CREATED);
        let purged = tower::ServiceExt::oneshot(
            crate::router(st.clone()),
            req("DELETE", "/q/tenants/purgectx", ""),
        )
        .await
        .expect("response");
        assert_eq!(purged.status(), axum::http::StatusCode::NO_CONTENT);
        // the purge released the loader's entry for the row while it still
        // listed it; run after the store purge it listed nothing
        assert!(st.loader.usage_get(&url).await.is_none());
        assert_eq!(fetch_count(&fetches), 0);
    }

    /// 5.13.3.5 counts uses of an @context this broker hosts on its own
    /// stored row — no second, Cached copy of the same document, and no
    /// fetch of its URL: the row holds the document (5.13.1 "Hosted").
    #[tokio::test]
    async fn a_hosted_context_is_counted_on_its_own_row() {
        let st = AppState::new("me".into());
        let owner = antares_model::TenantId::default();
        let (url, fetches) = context_server("/ngsi-ld/v1/jsonldContexts/local-1");
        // no `owner` member: a row written before it existed belongs to the
        // default Tenant, which is the one resolving below
        st.store
            .context_put(
                Some(&owner),
                "local-1",
                serde_json::json!({
                    "url": url,
                    "localId": "local-1",
                    "kind": "Hosted",
                    "createdAt": now_iso(),
                    "body": {"@context": {"peerTemp": "http://example.org/peerTemp"}},
                }),
            )
            .await
            .expect("seed hosted row");
        let user = serde_json::json!(url);
        st.loader.resolve_for(&owner, &user).await.expect("resolve");
        st.loader
            .resolve_for(&owner, &user)
            .await
            .expect("resolve again");
        let cached = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, url.as_bytes()).to_string();
        assert!(
            st.store
                .context_get(None, &cached)
                .await
                .expect("store")
                .is_none(),
            "a hosted @context must not be duplicated as a Cached row"
        );
        assert_eq!(
            st.store
                .context_get(Some(&owner), "local-1")
                .await
                .expect("store")
                .expect("row")["numberOfHits"],
            serde_json::json!(2)
        );
        assert_eq!(
            fetch_count(&fetches),
            0,
            "a hosted @context is read from its row, never fetched over the network"
        );
    }

    /// 5.13.1: what this broker HOSTS comes from its own store, not from the
    /// network. The in-process copy is a cache, and a miss — a restart (only
    /// `Cached` rows are preloaded) or an eviction from the bounded document
    /// cache — must not turn into an outbound GET of a URL a client chose:
    /// the URL is minted from the request's `Host` header when
    /// `ANTARES_PUBLIC_URL` is unset, so a spoofed one would send the broker
    /// to the spoofer for the term mappings that expand that Tenant's
    /// payloads. 5.5.10 still holds through the store: the row's owner is
    /// the only Tenant it resolves for.
    #[tokio::test]
    async fn a_hosted_context_resolves_from_the_store_after_its_copy_is_gone() {
        let st = AppState::new("me".into());
        let alpha = antares_model::TenantId::new("alpha").expect("tenant");
        let beta = antares_model::TenantId::new("beta").expect("tenant");
        // a dead port: anything resolved can only have come from the row
        let url = "http://127.0.0.1:9/ngsi-ld/v1/jsonldContexts/hosted-1";
        st.store
            .context_put(
                Some(&alpha),
                "hosted-1",
                serde_json::json!({
                    "url": url,
                    "localId": "hosted-1",
                    "kind": "Hosted",
                    "createdAt": now_iso(),
                    "owner": "alpha",
                    "body": {"@context": {"secret": "https://alpha.example/secret"}},
                }),
            )
            .await
            .expect("seed hosted row");
        let user = serde_json::json!(url);
        let ctx = st
            .loader
            .resolve_for(&alpha, &user)
            .await
            .expect("the owning Tenant resolves its stored @context");
        assert_eq!(ctx.expand_key("secret"), "https://alpha.example/secret");
        let err = st
            .loader
            .resolve_for(&beta, &user)
            .await
            .expect_err("another Tenant may not resolve it (5.5.10)");
        assert!(
            matches!(err, antares_model::NgsiError::LdContextNotAvailable(_)),
            "got {err:?}"
        );
    }

    /// 5.13.1 + 5.5.10: a stored @context with no `owner` member is not
    /// ownerless. `contexts.rs` `row_visible` reads a row written before that
    /// member existed as the DEFAULT Tenant's — it is listed, served and
    /// deleted through that Tenant alone — so resolving one must answer the
    /// same way. Read as "belongs to no Tenant" it would expand every other
    /// Tenant's payloads with the default Tenant's private term mappings.
    #[tokio::test]
    async fn a_stored_context_without_an_owner_belongs_to_the_default_tenant() {
        let st = AppState::new("me".into());
        let default = antares_model::TenantId::new("default").expect("tenant");
        let other = antares_model::TenantId::new("beta").expect("tenant");
        let url = "http://127.0.0.1:9/ngsi-ld/v1/jsonldContexts/legacy-1";
        st.store
            .context_put(
                Some(&default),
                "legacy-1",
                serde_json::json!({
                    "url": url,
                    "localId": "legacy-1",
                    "kind": "Hosted",
                    "createdAt": now_iso(),
                    "body": {"@context": {"legacy": "https://legacy.example/term"}},
                }),
            )
            .await
            .expect("seed a row from before the owner member");
        let user = serde_json::json!(url);
        let ctx = st
            .loader
            .resolve_for(&default, &user)
            .await
            .expect("the Tenant the row belongs to resolves it");
        assert_eq!(ctx.expand_key("legacy"), "https://legacy.example/term");
        let err = st
            .loader
            .resolve_for(&other, &user)
            .await
            .expect_err("no other Tenant may resolve it (5.5.10)");
        assert!(
            matches!(err, antares_model::NgsiError::LdContextNotAvailable(_)),
            "got {err:?}"
        );
    }

    /// 5.13.1 again, with the local id of a row that exists: what this broker
    /// hosts is the row MINTED under that URL, so a document served from
    /// somewhere else under the same resource path — with the local id of a
    /// Hosted @context another Tenant added — is external. It becomes a
    /// Cached row of its own, and the Tenant's row is neither read for its
    /// mappings nor counted against.
    #[tokio::test]
    async fn a_peer_url_reusing_a_stored_local_id_leaves_that_row_alone() {
        let st = AppState::new("me".into());
        let (url, _) = context_server("/ngsi-ld/v1/jsonldContexts/alpha-1");
        let row = serde_json::json!({
            "url": "https://broker.example/ngsi-ld/v1/jsonldContexts/alpha-1",
            "localId": "alpha-1",
            "kind": "Hosted",
            "createdAt": now_iso(),
            "owner": "alpha",
            "body": {"@context": {"a": "http://example.org/a"}},
        });
        let alpha = antares_model::TenantId::new("alpha").expect("tenant");
        st.store
            .context_put(Some(&alpha), "alpha-1", row.clone())
            .await
            .expect("seed hosted row");
        st.loader
            .resolve_for(&antares_model::TenantId::default(), &serde_json::json!(url))
            .await
            .expect("resolve");
        assert_eq!(
            st.store
                .context_get(Some(&alpha), "alpha-1")
                .await
                .expect("store")
                .expect("row"),
            row,
            "another Tenant's row must be untouched by a peer URL"
        );
        let cached = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, url.as_bytes()).to_string();
        assert_eq!(
            st.store
                .context_get(None, &cached)
                .await
                .expect("store")
                .expect("row")["kind"],
            serde_json::json!("Cached"),
            "the fetched document is this broker's Cached copy (5.13.1)"
        );
    }
}

#[cfg(all(test, feature = "test-kit"))]
mod clone_cost {
    use super::*;

    /// T-3689: every middleware layer and extractor of a request clones the
    /// state; a clone copies no String, no client and no handle.
    #[test]
    fn a_clone_shares_the_per_process_part() {
        let a = AppState::new("antares-test".into());
        let b = a.clone();
        assert!(std::ptr::eq(&a.http, &b.http), "the HTTP client is copied");
        assert!(std::ptr::eq(&a.fed_http, &b.fed_http));
        assert!(std::ptr::eq(a.public_url.as_ptr(), b.public_url.as_ptr()));
        assert!(std::ptr::eq(a.store_name.as_ptr(), b.store_name.as_ptr()));
    }

    /// A setting written before serving is the writer's own: a state cloned
    /// earlier keeps what it had, as it did when every field was copied.
    #[test]
    fn a_write_after_a_clone_is_not_seen_by_the_clone() {
        let a = AppState::new("antares-test".into());
        let mut c = a.clone();
        c.max_limit = 7;
        assert_eq!((a.max_limit, c.max_limit), (1000, 7));
        assert!(!std::ptr::eq(&a.http, &c.http));
    }

    /// F-ANT2-024: a setting written after an in-process call reaches the
    /// next call; the memoised router of the clone was built over the old
    /// settings and answered `limit=5` as if `max_limit` were still 1000.
    #[tokio::test]
    async fn a_write_after_a_call_reaches_the_next_call() {
        let call = |st: &AppState| {
            let req = axum::http::Request::get("/ngsi-ld/v1/entities?type=T&limit=5")
                .body(axum::body::Body::empty())
                .expect("request");
            let st = st.clone();
            async move { st.call(&axum::http::HeaderMap::new(), req).await.status() }
        };
        let a = AppState::new("antares-test".into());
        assert_eq!(call(&a).await, axum::http::StatusCode::OK);
        let mut c = a.clone();
        c.max_limit = 1;
        assert_eq!(call(&c).await, axum::http::StatusCode::FORBIDDEN);
    }
}
