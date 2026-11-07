// SPDX-License-Identifier: EUPL-1.2
//! The policy seam: one trait, one built-in engine, every engine an addon
//! (ADR-0020).
//!
//! The broker takes no authorization decision of its own. It asks the
//! engine a deployment gave it and obeys, and what an engine may answer is
//! deliberately narrow: allow, deny, or narrow the operation. Nothing here
//! parses a credential or validates a token — authentication, rate limiting
//! and quotas stay in the gateway in front of the broker.
//!
//! Everything in this module is core code, compiled and tested in every
//! build. The only engine the broker ships is [`AllowAll`], and conformance
//! is asserted against it; any other engine is an addon crate outside
//! `crates/`, behind an off-by-default `antares-broker` feature.
//!
//! The seam fails closed: an engine that errors, panics or runs past
//! [`TIMEOUT`] denies. A deployment that wires in a broken engine loses
//! service, never its access rules.

/// What [`Filter::metadata`] holds, here beside it for an engine crate.
pub mod metadata;

use antares_ql::geo::GeoQuery;
use antares_ql::QNode;
use axum::http::HeaderMap;
pub use metadata::Metadata;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;

/// The `@context` [`Filter::project`] reads rule conditions with.
static CORE_CONTEXT: LazyLock<antares_jsonld::Context> =
    LazyLock::new(antares_jsonld::core_context);
use std::time::Duration;

/// What [`PolicyEngine::decide`] hands back. Boxed because an engine that
/// asks a policy server has to await, and the trait must stay object-safe:
/// the broker holds one `Arc<dyn PolicyEngine>` chosen at startup.
pub type DecisionFuture<'a> = Pin<Box<dyn Future<Output = Decision> + Send + 'a>>;

/// What [`PolicyEngine::decide_each`] hands back: one [`Decision`] per
/// Operation it was given, in the same order.
pub type DecisionsFuture<'a> = Pin<Box<dyn Future<Output = Vec<Decision>> + Send + 'a>>;

/// How long an engine has to answer before the broker stops waiting and
/// denies. Deployment knob (`ANTARES_POLICY_TIMEOUT_MS`), read once at
/// first use. The default is short on purpose: the seam sits in front of
/// every request, and an engine that cannot answer inside it is an outage
/// either way — failing closed at 250 ms is the difference between a 403
/// and a broker that stops accepting.
pub static TIMEOUT: LazyLock<Duration> = LazyLock::new(|| {
    let env = std::env::var("ANTARES_POLICY_TIMEOUT_MS").ok();
    Duration::from_millis(timeout_ms(env.as_deref()))
});

/// `ANTARES_POLICY_TIMEOUT_MS` in milliseconds: a positive integer, else 250.
fn timeout_ms(env: Option<&str>) -> u64 {
    env.and_then(|v| v.parse().ok())
        .filter(|n: &u64| *n > 0)
        .unwrap_or(250)
}

/// The reasons the seam denies on its own, rather than on an engine's word.
/// Fixed strings: they reach the client in a ProblemDetails `detail`, and
/// an engine's own text about why it failed is not the client's business.
pub const ENGINE_TIMED_OUT: &str = "the policy engine did not answer in time";
pub const ENGINE_FAILED: &str = "the policy engine failed";

/// The clauses whose operations act on everything the tenant holds. There
/// is no narrowed form of "delete every entity" or "snapshot the tenant",
/// so a [`Decision::Filter`] on one of them is answered as a deny
/// ([`resolve`]).
pub const WHOLE_TENANT: [&str; 4] = ["5.6.21", "5.16.1", "5.16.2", "5.16.7"];

/// The clauses whose handlers read the [`Filter`] an engine returns: the
/// reads that can serve less than they were asked for. Every other clause
/// takes its operation whole — a create either writes the Entity or does
/// not — so a [`Decision::Filter`] there would be dropped, and the broker
/// would perform in full an operation the engine believed it had narrowed.
/// That is the one direction the seam may not fail in, so those clauses
/// answer a narrowing as a deny ([`resolve`]).
/// Discovery (5.7.5 to 5.7.10) is among them: ADR-0023 rule 5 computes it
/// from the Entities the narrowing selects.
/// Query Subscriptions (5.8.4, 5.11.5) and Query Registrations (5.10.2)
/// narrow the list ([`RECORD_LISTS`]).
pub const FILTERABLE: [&str; 15] = [
    "5.7.1", "5.7.2", "5.7.3", "5.7.4", "5.7.5", "5.7.6", "5.7.7", "5.7.8", "5.7.9", "5.7.10",
    "5.14.4", "5.14.5", "5.8.4", "5.11.5", "5.10.2",
];

/// The FILTERABLE clauses that list Subscriptions or Registrations. A
/// narrowing there selects which documents are listed, counted and paged,
/// and projects nothing: those are the client's own 5.2.12 and 5.2.9
/// records, not Entities with Attributes to drop, so a [`Filter`] that
/// projects is a deny there ([`resolve`], T-3673).
pub const RECORD_LISTS: [&str; 3] = ["5.8.4", "5.11.5", "5.10.2"];

/// List and Serve @context (5.13.3, 5.13.4). A stored `@context` may define
/// a term for an Attribute the caller may not see, so a narrowing there,
/// whatever it holds, says the caller sees fewer names than the tenant
/// holds: the list is the core `@context` alone and a stored one is not
/// found (ADR-0023 rule 5, T-4009). [`resolve`] passes it through.
pub const NAME_READS: [&str; 2] = ["5.13.3", "5.13.4"];

/// Who is asking. The headers are the ones a deployment names for the
/// seam to carry, copied verbatim and never interpreted: the broker does
/// not know what a token is. They never leave this process — stripped from every
/// forwarded request, absent from notifications, dead letters and logs,
/// which is why [`std::fmt::Debug`] here prints names and not values.
#[derive(Clone)]
pub struct Subject {
    pub tenant: antares_model::TenantId,
    pub headers: Vec<(String, String)>,
}

/// The name of the reserved pair in [`Subject::headers`] that says which view
/// the request came through (ADR-0020 amendment, T-3702). `@` is no
/// header-name character (RFC 9110 `tchar`), so no request can carry it:
/// only [`through_view`] puts it there.
pub const VIEW_PAIR: &str = "@view";

tokio::task_local! {
    /// The view the in-process request running in this task came through.
    static VIEW: std::sync::Arc<str>;
}

/// Runs `f` as a request through the view `id`: every [`Subject`] built in
/// it carries the [`VIEW_PAIR`]. The view surfaces' operation layer is the
/// one caller; a request off the socket never runs inside it.
pub async fn through_view<F: Future>(id: &str, f: F) -> F::Output {
    VIEW.scope(std::sync::Arc::from(id), f).await
}

impl Subject {
    /// The view the request came through, if it came through one.
    #[must_use]
    pub fn view(&self) -> Option<&str> {
        self.headers
            .iter()
            .find(|(name, _)| name == VIEW_PAIR)
            .map(|(_, id)| id.as_str())
    }
}

impl std::fmt::Debug for Subject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subject")
            .field("tenant", &self.tenant)
            .field(
                "headers",
                &self.headers.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// What is being asked for, after negotiation and expansion: the request
/// the broker is about to run, in the terms the operation itself uses.
/// Every name is expanded, so an engine writes its rules against IRIs and
/// not against whatever short name the caller's `@context` happened to use.
pub struct Operation<'a> {
    /// The CIM 009 clause of the operation, e.g. `"5.6.1"`.
    pub clause: &'static str,
    /// The Entity identifiers the operation names, as the handler holds
    /// them; empty for a query that names none.
    pub ids: &'a [&'a str],
    pub types: &'a [String],
    pub attrs: &'a [String],
    pub q: Option<&'a QNode>,
    pub scope_q: Option<&'a str>,
    pub geo: Option<&'a GeoQuery>,
    /// The request body of a write, expanded. `None` for a read.
    pub body: Option<&'a Value>,
    /// What a write by id lands on, as stored (ADR-0023 rule 7): `None`
    /// for a read, a create, or an id the tenant does not hold.
    pub stored: Option<&'a StoredImage>,
}

/// The stored Entity a write by id changes, as an engine judges it
/// (ADR-0023 rule 7, operation matrix item 2, T-3483): what the Entity is,
/// not what the request says it is. Names only, expanded; never a value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredImage {
    pub types: Vec<String>,
    pub scope: Vec<String>,
    pub attribute_names: Vec<String>,
}

impl StoredImage {
    /// The image of one stored, expanded Entity document.
    pub fn of(doc: &Value) -> Self {
        let strings = |v: Option<&Value>| match v {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        };
        Self {
            types: strings(doc.get("type")),
            scope: strings(doc.get("scope")),
            attribute_names: doc
                .as_object()
                .map(|o| {
                    o.keys()
                        .filter(|k| {
                            !k.starts_with("__")
                                && !k.starts_with('@')
                                && !matches!(
                                    k.as_str(),
                                    "id" | "type"
                                        | "scope"
                                        | "createdAt"
                                        | "modifiedAt"
                                        | "deletedAt"
                                )
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// Shape and counts, never the payload: an `Operation` carries the
/// caller's data, and a log line is not where it belongs.
impl std::fmt::Debug for Operation<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Operation")
            .field("clause", &self.clause)
            .field("ids", &self.ids.len())
            .field("types", &self.types.len())
            .field("attrs", &self.attrs.len())
            .field("q", &self.q.is_some())
            .field("scope_q", &self.scope_q.is_some())
            .field("geo", &self.geo.is_some())
            .field("body", &self.body.is_some())
            .field("stored", &self.stored.is_some())
            .finish()
    }
}

impl Operation<'_> {
    /// The operation with nothing but its clause: what a handler that
    /// carries no ids, types or attributes passes, and the base every other
    /// call site fills in with struct-update syntax.
    pub const fn new(clause: &'static str) -> Operation<'static> {
        Operation {
            clause,
            ids: &[],
            types: &[],
            attrs: &[],
            q: None,
            scope_q: None,
            geo: None,
            body: None,
            stored: None,
        }
    }
}

/// What an engine may answer: an operation is allowed, refused, allowed
/// over less than it asked for, or allowed on a condition the stored Entity
/// has to meet.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    /// Refused, with the engine's own reason. Answered 403 with a
    /// ProblemDetails whose type is an Antares URI — Table 6.3.2-1 names no
    /// access-denied error, so this is an Antares decision.
    Deny(String),
    /// Allowed over less. The caller cannot tell a hidden entity from an
    /// absent one.
    Filter(Filter),
    /// Allowed only if the stored Entity the write lands on is one the
    /// filter selects (gap G12): its `q`, `scopeQ`, areas and the types and
    /// conditions of its `rules`, judged on stored values, which the
    /// [`Operation`] does not carry. The store judges the document it is
    /// about to change, inside the write's own read-modify-write, so a
    /// concurrent write between this answer and the commit cannot carry the
    /// write onto an Entity the filter no longer selects. An Entity it does
    /// not select answers as an absent one. Honoured on [`WRITE_BY_ID`], with
    /// a filter that selects and projects nothing; anywhere else a deny
    /// ([`resolve`]).
    AllowIf(Filter),
}

/// The writes by id a [`Decision::AllowIf`] can condition: each changes or
/// deletes Entities that are already stored, by id, and judges each of them
/// inside its store operation. A create has no stored Entity to judge; Batch
/// Upsert updates the members its condition selects and creates none of the
/// conditioned ones (T-3668). The temporal writes 5.6.12 to 5.6.15 judge the
/// evolution's latest instances (`Condition::pre_evolution`); 5.6.16 is
/// not one: the temporal seam has no delete that reads the document first.
pub const WRITE_BY_ID: [&str; 16] = [
    "5.6.2", "5.6.3", "5.6.4", "5.6.5", "5.6.6", "5.6.17", "5.6.18", "5.6.19", "5.6.8", "5.6.9",
    "5.6.10", "5.6.20", "5.6.12", "5.6.13", "5.6.14", "5.6.15",
];

/// Why a write conditioned on its stored Entity is refused rather than
/// forwarded: a Context Source's copy is not in this store, so nothing here
/// can judge it.
pub const CONDITION_NOT_FORWARDED: &str =
    "a write conditioned on the stored Entity is not forwarded to a Context Source";

/// How much less. Every member narrows and none widens: the `q`, `scopeQ`
/// and `geo_q` are conjoined with the caller's own, `pick` keeps a subset of
/// the members, `omit` removes some.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    /// Conjoined into the query the store runs, on the AST — never on the
    /// query string, where the 4.9 precedence of `;` against `|` would have
    /// to be re-derived by whoever writes the rule. Boxed, like `geo_q`: a
    /// parsed query is half the size of the struct, and every [`Decision`]
    /// and [`NotifyDecision`] would carry it inline.
    pub q: Option<Box<QNode>>,
    /// Conjoined the same way with the request's own `scopeQ` (4.18).
    pub scope_q: Option<String>,
    /// An area the Entity must lie in (4.10), held beside the request's own
    /// geoquery and never in its place: 7.2.5 lets a request carry one, so
    /// the store query takes both and every one must hold (ADR-0023 rule 1).
    /// A [`GeoQuery`] is only built by `GeoQuery::from_params`, which bounds
    /// it like a request's. Boxed: a parsed geometry is large, and every
    /// [`Decision`] would carry its size.
    pub geo_q: Option<Box<GeoQuery>>,
    /// Further areas, every one of which must hold as well: a view and an
    /// engine that both narrow by area compose into `geo_q` and these
    /// (ADR-0023 rule 1). An engine with one area leaves this empty; read
    /// them all through [`Filter::areas`].
    pub geo_also: Vec<GeoQuery>,
    /// Members removed from every document served.
    pub omit: Vec<String>,
    /// If non-empty, the only members served beside the document frame.
    pub pick: Vec<String>,
    /// Attributes allowed per rule (ADR-0023 rule 11, gap G4): when
    /// non-empty, an Entity is served only if at least one [`Rule`] holds
    /// for it, and it keeps the union of the `attributes` of the rules that
    /// hold, nothing else beside its frame. An Entity no rule holds for is
    /// left out of the answer. Applied to every document the answer
    /// carries, joined targets and notified Entities included, each by its
    /// own rules. It intersects with `pick`: both narrow.
    pub rules: Vec<Rule>,
    /// Attributes removed per Entity type: an Entity loses every Attribute
    /// any of its types omits.
    pub omit_by_type: BTreeMap<String, Vec<String>>,
    /// Answer [`RESTRICTED_HEADER`]`: true`, so a client can know the
    /// answer was narrowed. Narrowing is otherwise silent.
    pub restricted: bool,
    /// The engine's advisory, sent as `NGSILD-Warning` (TS 104 176 6.3.17,
    /// gap G8): e.g. the Entity types a filter on an Attribute the caller
    /// may not filter on left out, so a developer can tell that refusal
    /// from an empty answer. The text is the engine's, never an echo of the
    /// request, and names only what the caller may already see.
    pub warning: Option<Warning>,
    /// Windows of `observedAt` the instances of a temporal read (5.7.3,
    /// 5.7.4) must fall into, every one of them (gap G6): the request's own
    /// 4.11 window is clamped to them before the store runs, so the store
    /// fetches no instance outside them. A window the request does not reach
    /// answers nothing, never a refusal that tells where the grant ends. A
    /// slice, not a `Vec`: every [`Decision`] carries a `Filter`'s size.
    pub temporal: Box<[Window]>,
    /// The metadata members each Attribute shows (platform ADR-N-066 2.4,
    /// DM-139): stripped from the answer after the handler, and a `q`,
    /// `orderBy` or `timeproperty` naming a stripped one is refused 400
    /// ([`metadata`]). `None` passes every member. Boxed for the
    /// size every [`Decision`] carries.
    pub metadata: Option<Box<metadata::Metadata>>,
}

/// A half-open interval `[from, to)` of `observedAt`; an open end is the
/// beginning or the end of time (gap G6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Window {
    /// The earliest instant admitted.
    pub from: Option<TimeBound>,
    /// The first instant no longer admitted.
    pub to: Option<TimeBound>,
}

/// One end of a [`Window`]: a fixed instant, or a duration before the
/// moment the request is decided, so "the last 24 hours" slides with now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeBound {
    /// This instant.
    At(chrono::DateTime<chrono::Utc>),
    /// This long before now.
    Ago(chrono::Duration),
}

impl TimeBound {
    fn at(self, now: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
        match self {
            Self::At(t) => t,
            Self::Ago(d) => now
                .checked_sub_signed(d)
                .unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC),
        }
    }

    /// The bound at `now` in milliseconds, the precision 4.11 accepts: a
    /// start (`up`) rounds up and an end down, so rounding only narrows.
    fn render(self, now: chrono::DateTime<chrono::Utc>, up: bool) -> String {
        let t = self.at(now);
        let sub = i64::from(t.timestamp_subsec_nanos() % 1_000_000);
        let t = if up && sub > 0 {
            t.checked_add_signed(chrono::Duration::nanoseconds(
                1_000_000_i64.saturating_sub(sub),
            ))
            .unwrap_or(t)
        } else {
            t
        };
        t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }
}

impl Window {
    /// Whether an instance observed at `observed_at` falls inside, the
    /// bounds resolved against `now`; 4.11's half-open comparison.
    #[must_use]
    pub fn admits(&self, observed_at: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
        let t = antares_model::dt_key(observed_at);
        self.from
            .is_none_or(|b| t >= antares_model::dt_key(&b.render(now, true)))
            && self
                .to
                .is_none_or(|b| t < antares_model::dt_key(&b.render(now, false)))
    }

    /// Whether every instance observed at or after `start` is past this
    /// window's start, the bound resolved against `now`.
    #[must_use]
    pub fn opens_by(&self, start: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.from.is_none_or(|b| {
            antares_model::dt_key(start) >= antares_model::dt_key(&b.render(now, true))
        })
    }
}

/// The text of a [`Filter::warning`]: printable ASCII on one line, at most
/// [`Warning::MAX_LEN`] bytes, no `"` or `\`, so it travels as one quoted
/// `warn-text` of an `NGSILD-Warning` value. Composed filters keep each
/// side's text, at most [`Warning::MAX_TEXTS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning(Box<[String]>); // a slice: every Decision carries a Filter's size

impl Warning {
    /// Bytes in one text.
    pub const MAX_LEN: usize = 256;
    /// Texts a composed filter keeps.
    pub const MAX_TEXTS: usize = 4;

    /// One advisory.
    ///
    /// # Errors
    /// When `text` is empty, longer than [`Warning::MAX_LEN`], or holds a
    /// character outside printable ASCII (a line break, a control
    /// character), a `"` or a `\\`: it could not travel as one header line.
    pub fn new(text: &str) -> Result<Self, String> {
        if text.is_empty() || text.len() > Self::MAX_LEN {
            return Err(format!(
                "a policy warning holds 1 to {} bytes",
                Self::MAX_LEN
            ));
        }
        if !text
            .bytes()
            .all(|b| (0x20..=0x7e).contains(&b) && b != b'"' && b != b'\\')
        {
            return Err(
                "a policy warning is printable ASCII on one line, without '\"' or '\\\\'".into(),
            );
        }
        Ok(Self(Box::new([text.to_owned()])))
    }

    /// The texts, in the order the filters were composed.
    #[must_use]
    pub fn texts(&self) -> &[String] {
        &self.0
    }

    /// `a` and `b` as one: each text once, `a`'s first.
    #[must_use]
    pub fn join(a: Option<&Self>, b: Option<Self>) -> Option<Self> {
        match (a, b) {
            (Some(a), Some(b)) => {
                let mut texts = a.0.to_vec();
                for t in b.0 {
                    if !texts.contains(&t) {
                        texts.push(t);
                    }
                }
                texts.truncate(Self::MAX_TEXTS);
                Some(Self(texts.into()))
            }
            (Some(a), None) => Some(a.clone()),
            (None, b) => b,
        }
    }
}

/// One allowlist of a [`Filter`] (ADR-0023 rule 11): the Entities of
/// `types` for which the condition holds show `attributes`. The condition
/// (`q`, `scope_q`, `geo_q`, each optional) is the policy's: it is read on
/// the stored Entity, which may carry Attributes the caller cannot see.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rule {
    /// Type names, at least one; a rule naming none holds for nothing.
    pub types: Vec<String>,
    /// On the Entity's own members. A linked path (4.9
    /// `LinkedEntityRelation`) never holds: the visible set of one Entity
    /// must not depend on another Entity the caller may not read.
    pub q: Option<Box<QNode>>,
    pub scope_q: Option<String>,
    pub geo_q: Option<Box<GeoQuery>>,
    /// Further areas the Entity must lie in, as [`Filter::geo_also`].
    pub geo_also: Vec<GeoQuery>,
    /// Entity ids the rule holds for (gap G2). With `id_patterns`, an
    /// Entity is selected when its id is one of these or matches one of
    /// them; both empty select every id.
    pub ids: Vec<String>,
    /// Regular expressions an Entity id may match instead, each anchored
    /// at both ends. One that does not compile, or compiles past
    /// `antares_ql::regex`'s bound, matches nothing (fail closed).
    pub id_patterns: Vec<String>,
    /// The Attributes an Entity this rule holds for shows.
    pub attributes: Vec<String>,
    /// The rule shows every Attribute, and `attributes` is not read
    /// (ADR-0023 rule 11, amended by T-3637): an engine's grant over the
    /// whole Entity. A view never sets it.
    pub every_attribute: bool,
    /// Windows of `observedAt` (gap G6): in a temporal read (5.7.3, 5.7.4)
    /// an instance of these `attributes` is shown through this rule only if
    /// it falls into every one. A current-state read shows what the rule
    /// shows, as the gateway's grant windows did.
    pub temporal: Vec<Window>,
}

impl Rule {
    /// Every area the Entity must lie in: `geo_q` and `geo_also`.
    pub fn areas(&self) -> impl Iterator<Item = &GeoQuery> {
        self.geo_q.as_deref().into_iter().chain(&self.geo_also)
    }

    /// Whether the rule names `doc` by one of its types and, when it
    /// narrows ids, by its id. Neither changes over an Entity's history,
    /// so a temporal instance is selected the same way (unlike the
    /// condition, [`Rule::is_conditional`]).
    pub fn selects(&self, doc: &Value) -> bool {
        // `entity_types` read in place: this runs per rule per Entity of a
        // page, and the list it would allocate is read once.
        let named = |t: &str| self.types.iter().any(|r| r == t);
        let typed = match doc.get("type") {
            Some(Value::String(t)) => named(t),
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).any(named),
            _ => false,
        };
        typed
            && match doc.get("id").and_then(Value::as_str) {
                Some(id) => self.selects_id(id),
                None => self.ids.is_empty() && self.id_patterns.is_empty(),
            }
    }

    /// Whether the rule's `ids` and `id_patterns` select `id`.
    pub fn selects_id(&self, id: &str) -> bool {
        (self.ids.is_empty() && self.id_patterns.is_empty())
            || self.ids.iter().any(|i| i == id)
            || self.id_patterns.iter().any(|p| {
                antares_ql::regex::compile(&format!("^(?:{p})$")).is_ok_and(|re| re.is_match(id))
            })
    }

    /// Whether the rule has a condition beside its types.
    pub fn is_conditional(&self) -> bool {
        self.q.is_some() || self.scope_q.is_some() || self.areas().next().is_some()
    }

    /// Whether the rule holds for exactly the Entities of its types: no
    /// condition, no ids, no id patterns (what [`Self::holds`] reads beside
    /// the types).
    pub fn admits_by_type_alone(&self) -> bool {
        !self.is_conditional() && self.ids.is_empty() && self.id_patterns.is_empty()
    }

    /// Whether the rule holds for `doc`, an Entity in the form `ctx`
    /// expands names to (the broker's internal form: expanded keys, each
    /// Attribute an array of instances).
    pub fn holds(&self, doc: &Value, ctx: &antares_jsonld::Context) -> bool {
        self.selects(doc)
            && self
                .scope_q
                .as_deref()
                .is_none_or(|s| antares_ql::scope::scope_matches(s, doc))
            && self.areas().all(|g| g.matches(doc, ctx))
            && self.q.as_deref().is_none_or(|q| {
                !q.has_linked_paths() && antares_ql::eval::eval_q(q, doc, ctx, &|_| None)
            })
    }

    /// The Attributes the condition reads, expanded through `ctx`: the head
    /// of every `q` term and the geoquery's GeoProperty.
    pub fn reads(&self, ctx: &antares_jsonld::Context) -> Vec<String> {
        let mut out: Vec<String> = self
            .q
            .as_deref()
            .map(|q| {
                q.attribute_paths()
                    .into_iter()
                    .filter_map(|p| p.split(['.', '[', '{']).next())
                    .map(|h| ctx.expand_key(h))
                    .collect()
            })
            .unwrap_or_default();
        for g in self.areas() {
            out.push(if g.geoproperty.is_empty() {
                antares_ql::geo::LOCATION_IRI.to_owned()
            } else {
                ctx.expand_key(&g.geoproperty)
            });
        }
        out.sort();
        out.dedup();
        out
    }

    /// ADR-0023 rule 11 disclosure: the Attributes the condition reads that
    /// the rule does not show. Seeing the rule's Attributes tells the
    /// caller the condition held, so these are what the visible set
    /// discloses; the view loader reports them.
    pub fn discloses(&self, ctx: &antares_jsonld::Context) -> Vec<String> {
        if self.every_attribute {
            return Vec::new();
        }
        self.reads(ctx)
            .into_iter()
            .filter(|r| !self.attributes.iter().any(|a| ctx.expand_key(a) == *r))
            .collect()
    }
}

/// The answer of a broker without an engine: a [`Filter`] that narrows
/// nothing, one shared instance so asking costs no allocation.
pub fn unnarrowed() -> std::sync::Arc<Filter> {
    static NOTHING: LazyLock<std::sync::Arc<Filter>> =
        LazyLock::new(|| std::sync::Arc::new(Filter::default()));
    NOTHING.clone()
}

/// What a [`Filter`]'s rules let one Entity show: `only` (when set) is the
/// union of the matched rules' Attributes, `omitted` what its types omit.
#[derive(Debug, Clone, Default, PartialEq)]
///
/// The names borrow from the rules that decided them, so judging an Entity
/// copies no Attribute name (T-3697); [`Shown::into_owned`] and
/// [`Shown::map_names`] give a set that outlives the rules.
pub struct Shown<'a> {
    pub only: Option<std::collections::BTreeSet<Cow<'a, str>>>,
    pub omitted: std::collections::BTreeSet<Cow<'a, str>>,
}

impl Shown<'_> {
    /// Nothing beside the frame: what a projection falls back to for an
    /// Entity no rule holds for, should one reach it.
    pub fn nothing() -> Self {
        Shown {
            only: Some(Default::default()),
            omitted: Default::default(),
        }
    }

    pub fn allows(&self, k: &str) -> bool {
        self.only.as_ref().is_none_or(|o| o.contains(k)) && !self.omitted.contains(k)
    }

    /// The same set with every name mapped, for a document in another form.
    pub fn map_names(&self, f: impl Fn(&str) -> String) -> Shown<'static> {
        Shown {
            only: self
                .only
                .as_ref()
                .map(|o| o.iter().map(|n| Cow::Owned(f(n))).collect()),
            omitted: self.omitted.iter().map(|n| Cow::Owned(f(n))).collect(),
        }
    }

    /// The same set, owning its names.
    pub fn into_owned(self) -> Shown<'static> {
        let own = |n: Cow<'_, str>| Cow::Owned(n.into_owned());
        Shown {
            only: self.only.map(|o| o.into_iter().map(own).collect()),
            omitted: self.omitted.into_iter().map(own).collect(),
        }
    }
}

/// The types of an Entity document: its `type` member, a string or an
/// array of strings (5.2.4), in whatever form the document is in.
pub fn entity_types(doc: &Value) -> Vec<&str> {
    match doc.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// What identifies the document rather than describing it: 5.2.4 makes
/// `id` and `type` mandatory members of an Entity, and an answer without
/// them is not an Entity at all. A projection never removes these.
const FRAME: [&str; 3] = ["id", "type", "@context"];

impl Filter {
    /// Every area the Entity must lie in: `geo_q` and `geo_also`.
    pub fn areas(&self) -> impl Iterator<Item = &GeoQuery> {
        self.geo_q.as_deref().into_iter().chain(&self.geo_also)
    }

    /// Apply `pick`/`omit` and the rules to one document by member name.
    /// This is the reference semantics `run_policy_contract` holds an
    /// engine's answer against; a served document is projected through the
    /// request's own 5.5.2 representation instead
    /// (`repr::narrow_projection`), where the names are expanded against
    /// the request `@context` first. Rule conditions are read with the core
    /// `@context`; a document no rule holds for keeps its frame here, while
    /// the served paths leave it out.
    pub fn project(&self, doc: &mut Value) {
        let shown = self.has_rules().then(|| {
            self.shown(doc, &CORE_CONTEXT)
                .unwrap_or_else(Shown::nothing)
        });
        let Some(obj) = doc.as_object_mut() else {
            return;
        };
        if !self.pick.is_empty() {
            obj.retain(|k, _| FRAME.contains(&k.as_str()) || self.pick.iter().any(|p| p == k));
        }
        for name in &self.omit {
            if FRAME.contains(&name.as_str()) {
                continue;
            }
            obj.remove(name);
        }
        if let Some(shown) = shown {
            obj.retain(|k, _| FRAME.contains(&k.as_str()) || shown.allows(k));
        }
    }

    /// True when the filter would change nothing, which is how an engine
    /// that means "allow" can say so with an empty `Filter`.
    pub fn is_empty(&self) -> bool {
        self.q.is_none()
            && self.scope_q.is_none()
            && self.areas().next().is_none()
            && self.omit.is_empty()
            && self.pick.is_empty()
            && !self.has_rules()
            && self.temporal.is_empty()
            && self.metadata.is_none()
    }

    /// Whether the filter removes anything from what it selects: a
    /// projection, a per-type omit, a rule's Attribute list or a temporal
    /// window. A write condition ([`Decision::AllowIf`]) may not: the write
    /// is performed whole or not at all.
    pub fn projects(&self) -> bool {
        !self.pick.is_empty()
            || !self.omit.is_empty()
            || !self.omit_by_type.is_empty()
            || !self.temporal.is_empty()
            || self
                .rules
                .iter()
                .any(|r| !r.attributes.is_empty() || !r.temporal.is_empty())
    }

    /// Whether the filter carries rules or per-type omits (ADR-0023 rule 11).
    pub fn has_rules(&self) -> bool {
        !self.rules.is_empty() || !self.omit_by_type.is_empty()
    }

    /// Whether a rule holds for `doc` (every document without rules): what
    /// [`Self::shown`] answers `Some` for, without building the set.
    pub fn admits(&self, doc: &Value, ctx: &antares_jsonld::Context) -> bool {
        self.rules.is_empty() || self.rules.iter().any(|r| r.holds(doc, ctx))
    }

    /// ADR-0023 rule 11: what `doc` may show, or `None` when `rules` is
    /// non-empty and none holds for it (the Entity is out). An Entity sees
    /// the union of the Attributes of the rules that hold for it, less what
    /// any of its types omits. Names compare as written: the caller brings
    /// rules and document into one form first (`repr::type_rules` expands
    /// both), and `ctx` is the one the conditions' names expand through.
    pub fn shown(&self, doc: &Value, ctx: &antares_jsonld::Context) -> Option<Shown<'_>> {
        let only = if self.rules.is_empty() {
            None
        } else {
            let mut held = self.rules.iter().filter(|r| r.holds(doc, ctx)).peekable();
            held.peek()?;
            let held: Vec<&Rule> = held.collect();
            if held.iter().any(|r| r.every_attribute) {
                None
            } else {
                Some(
                    held.iter()
                        .flat_map(|r| r.attributes.iter().map(|a| Cow::Borrowed(a.as_str())))
                        .collect(),
                )
            }
        };
        if self.omit_by_type.is_empty() {
            return Some(Shown {
                only,
                omitted: Default::default(),
            });
        }
        let omitted = entity_types(doc)
            .into_iter()
            .filter_map(|t| self.omit_by_type.get(t))
            .flatten()
            .map(|a| Cow::Borrowed(a.as_str()))
            .collect();
        Some(Shown { only, omitted })
    }

    /// ADR-0023 rule 7 per rule (K22): a write is allowed only when some
    /// rule holds for its post-image and every Attribute the write names is
    /// shown by a rule that holds, none of which its types omit: the write
    /// sets what the Entity then shows, the union of its rules. One rule
    /// need not show them all, so a type whose list a view splits over
    /// several rules is written whole (T-4753). The check a view's engine
    /// runs before it allows a write.
    pub fn admits_write(
        &self,
        post_image: &Value,
        written: &[&str],
        ctx: &antares_jsonld::Context,
    ) -> bool {
        let types = entity_types(post_image);
        let omitted = |w: &str| {
            types
                .iter()
                .filter_map(|t| self.omit_by_type.get(*t))
                .any(|o| o.iter().any(|a| a == w))
        };
        if written.iter().any(|w| omitted(w)) {
            return false;
        }
        if self.rules.is_empty() {
            return true;
        }
        let holding: Vec<&Rule> = self
            .rules
            .iter()
            .filter(|r| r.holds(post_image, ctx))
            .collect();
        !holding.is_empty()
            && (holding.iter().any(|r| r.every_attribute)
                || written
                    .iter()
                    .all(|w| holding.iter().any(|r| r.attributes.iter().any(|a| a == w))))
    }

    /// Say the answer was narrowed, and add the engine's warning, when the
    /// engine asked for either. Narrowing is otherwise silent: a caller
    /// cannot tell an Entity it may not see from one that is not there.
    /// `alias` is the warn-agent, this broker's host alias.
    pub fn mark(&self, alias: &str, headers: &mut HeaderMap) {
        if self.restricted {
            headers.insert(
                RESTRICTED_HEADER,
                axum::http::HeaderValue::from_static("true"),
            );
        }
        for text in self.warning.iter().flat_map(Warning::texts) {
            // 199 Miscellaneous warning (RFC 7234 5.5), as the gateway's
            // dropped-types warning was.
            let value = crate::federation::warning(199, alias, text);
            if let Ok(v) = axum::http::HeaderValue::from_str(&value) {
                headers.append("NGSILD-Warning", v);
            }
        }
    }

    /// The request's temporal parameters with its 4.11 window clamped to
    /// [`Filter::temporal`], resolved against now (gap G6), for every reader
    /// after the gate: the store's instance range, the forward, the links.
    /// `required` is the operation's (5.7.4 demands a window, 5.7.3 does
    /// not), so a request that lacked one is still refused. The windows are
    /// on `observedAt`: a request on another timeproperty is answered
    /// nothing, since the store applies one range on one timeproperty.
    ///
    /// # Errors
    /// The request's own 4.11 window is invalid.
    pub fn narrow_window(
        &self,
        params: &std::collections::HashMap<String, String>,
        required: bool,
    ) -> Result<std::collections::HashMap<String, String>, antares_model::NgsiError> {
        let mut out = params.clone();
        if self.temporal.is_empty() {
            return Ok(out);
        }
        let asked = crate::temporalq::TemporalQ::from_params(params, required)?;
        let key = |s: &String| antares_model::dt_key(s);
        let (mut from, mut to) = match &asked {
            Some(tq) if tq.timeproperty != "observedAt" => {
                let epoch = "1970-01-01T00:00:00Z".to_owned();
                (Some(epoch.clone()), Some(epoch))
            }
            Some(tq) => match tq.timerel.as_str() {
                "before" => (None, Some(tq.time_at.clone())),
                "after" => (Some(tq.time_at.clone()), None),
                "between" => (Some(tq.time_at.clone()), tq.end_time_at.clone()),
                _ => (None, None),
            },
            None => (None, None),
        };
        let now = chrono::Utc::now();
        for w in &self.temporal {
            if let Some(b) = w.from.map(|b| b.render(now, true)) {
                from = Some(match from {
                    Some(f) if key(&f) >= key(&b) => f,
                    _ => b,
                });
            }
            if let Some(b) = w.to.map(|b| b.render(now, false)) {
                to = Some(match to {
                    Some(t) if key(&t) <= key(&b) => t,
                    _ => b,
                });
            }
        }
        // `between` an instant and itself admits nothing: the answer is
        // empty, as for a window over a period with no data
        if let (Some(f), Some(t)) = (&from, &to) {
            if key(f) >= key(t) {
                to = Some(f.clone());
            }
        }
        let (timerel, time_at, end) = match (from, to) {
            (Some(f), Some(t)) => ("between", f, Some(t)),
            (Some(f), None) => ("after", f, None),
            (None, Some(t)) => ("before", t, None),
            (None, None) => return Ok(out),
        };
        out.insert("timerel".into(), timerel.into());
        out.insert("timeAt".into(), time_at);
        match end {
            Some(e) => out.insert("endTimeAt".into(), e),
            None => out.remove("endTimeAt"),
        };
        Ok(out)
    }

    /// The request's own query parameters, narrowed by this decision.
    ///
    /// The `q` conjunction is made on the AST and rendered back once
    /// (`antares_ql`'s renderer parenthesises an `Or` inside an `And`, so
    /// 4.9's `;`-over-`|` precedence is the renderer's problem and not the
    /// rule writer's). Every consumer below reads the narrowed parameters:
    /// the store push-down, the local re-check that 5.7.2.4 runs over
    /// merged results, and the query the request is forwarded with.
    ///
    /// A `scopeQ` narrowing is set when the request carries none, and
    /// intersected with the request's own when it carries one. 4.19's `and`
    /// is over independent per-pattern predicates, so it distributes over
    /// the `,`/`|` disjunction and the intersection is itself a Scope
    /// Query — `antares_ql::scope::intersect_scope_q` writes it. Where the
    /// product cannot be written down the seam answers the only way that is
    /// not wider than the engine decided: it refuses.
    ///
    /// `geo_q` stays out of the parameters: they hold one geoquery (7.2.5)
    /// and it is the request's. The narrowing's area reaches the store
    /// query beside it through `repr::narrowing`.
    ///
    /// ADR-0023 rule 2: a term of the request's own `q` on an Attribute
    /// this `pick`/`omit` hides reads as a term on an Attribute the Entity
    /// does not have (see [`Filter::hide_terms`]). `ctx` is the request's,
    /// the one its `q` terms expand through.
    pub fn narrow_params(
        &self,
        params: &std::collections::HashMap<String, String>,
        ctx: &antares_jsonld::Context,
    ) -> Result<std::collections::HashMap<String, String>, Denied> {
        let mut out = params.clone();
        // the request's own `q` is parsed and refused long before a filter
        // reaches it; an unparsable one here is a caller that narrowed
        // something it never validated
        let own = match out.get("q").map(|q| antares_ql::parse_q(q)).transpose() {
            Ok(own) => own.map(|own| self.hide_terms(own, ctx)),
            Err(_) => return Err(Denied(ENGINE_FAILED.to_owned())),
        };
        // threat V23: the policy part is held to its share of the narrowed
        // caps on its own, so whether it fits never depends on the caller's
        // `q` (see `antares_ql::parse_narrowed_q`)
        if self
            .q
            .as_deref()
            .is_some_and(|q| !antares_ql::fits_policy_q(q))
        {
            return Err(Denied(ENGINE_FAILED.to_owned()));
        }
        let narrowed = match (own, &self.q) {
            (Some(own), Some(extra)) => Some(QNode::And(vec![own, (**extra).clone()])),
            (own, extra) => own.or_else(|| extra.as_deref().cloned()),
        };
        if let Some(narrowed) = narrowed {
            out.insert("q".to_owned(), narrowed.to_string());
        }
        if let Some(scope) = &self.scope_q {
            let narrowed = match out.get("scopeQ") {
                None => scope.clone(),
                Some(own) => antares_ql::scope::intersect_scope_q(own, scope)
                    .ok_or_else(|| Denied(SCOPE_NOT_NARROWABLE.to_owned()))?,
            };
            out.insert("scopeQ".to_owned(), narrowed);
        }
        Ok(out)
    }
}

impl Filter {
    /// Whether this `pick`/`omit` hides the Attribute a q term names as
    /// `written`, `iri` once expanded. A pick narrows Attributes only: the
    /// system members it never names stay readable, as in the projection.
    fn hides(&self, written: &str, iri: &str) -> bool {
        let named = |names: &[String]| {
            names
                .iter()
                .any(|n| crate::repr::policy_context().expand_key(n) == iri)
        };
        named(&self.omit)
            || (!self.pick.is_empty()
                && antares_ql::is_non_system_attr(written)
                && !named(&self.pick))
    }

    /// ADR-0023 rule 2 (T-3482): a term on an Attribute this narrowing
    /// hides reads as a term on one the Entity does not have — a
    /// comparison or `attr` is false, `!attr` is true — so the hidden value
    /// is never compared, on the store path or in memory. The q grammar
    /// has no constants, so false is written `a;!a` and true `a|!a`, on
    /// the hidden name itself: both hold whatever the Entity carries. The
    /// path's top Attribute decides, so a sub-attribute or a hop through a
    /// hidden Relationship is as hidden as the Attribute. Per-type rules
    /// are not read here: their q is read on what the rules show
    /// (`repr::client_q`).
    pub fn hide_terms(&self, node: QNode, ctx: &antares_jsonld::Context) -> QNode {
        if self.omit.is_empty() && self.pick.is_empty() {
            return node;
        }
        let constant = |top: &str, holds: bool| {
            let is = |negated| QNode::Exists {
                path: antares_ql::QPath::dotted(vec![top.to_owned()]),
                negated,
            };
            let pair = vec![is(false), is(true)];
            if holds {
                QNode::Or(pair)
            } else {
                QNode::And(pair)
            }
        };
        let hidden = |path: &antares_ql::QPath| {
            path.top()
                .filter(|top| self.hides(top, &ctx.expand_key(top)))
                .map(str::to_owned)
        };
        match node {
            QNode::And(nodes) => {
                QNode::And(nodes.into_iter().map(|n| self.hide_terms(n, ctx)).collect())
            }
            QNode::Or(nodes) => {
                QNode::Or(nodes.into_iter().map(|n| self.hide_terms(n, ctx)).collect())
            }
            QNode::Cmp { path, op, value } => match hidden(&path) {
                Some(top) => constant(&top, false),
                None => QNode::Cmp { path, op, value },
            },
            QNode::Exists { path, negated } => match hidden(&path) {
                Some(top) => constant(&top, negated),
                None => QNode::Exists { path, negated },
            },
        }
    }
}

/// The response header a `Filter { restricted: true }` adds. It is not in
/// the `NGSILD-` namespace: that prefix is ETSI's, carries the headers
/// clause 6.3 defines (`NGSILD-Tenant`, `NGSILD-EntityMap`,
/// `NGSILD-Results-Count`, `NGSILD-Warning`), and a broker-invented header
/// under it would collide with whatever a later version puts there — the
/// same reason a refusal answers `urn:antares:error:AccessDenied` rather
/// than an invented `uri.etsi.org` type.
pub const RESTRICTED_HEADER: &str = "Antares-Results-Restricted";

/// The refusal a `scopeQ` narrowing gets when its intersection with the
/// request's own cannot be written as a Scope Query — one side selects
/// nothing, or the distributed product is too large to express (see
/// [`Filter::narrow_params`]).
pub const SCOPE_NOT_NARROWABLE: &str =
    "the request's own scope query cannot be narrowed by the policy engine";

/// What an engine may answer about one notification, for one subscription.
#[derive(Debug, Clone, PartialEq)]
pub enum NotifyDecision {
    Deliver,
    /// Send it, narrowed: the same `pick`/`omit` projection the query path
    /// applies, over the entities of `data`. A notification `Filter` that
    /// carries `q` or `scopeQ` is refused as a [`NotifyDecision::Drop`]:
    /// the entities were selected by the subscription's own conditions long
    /// before this point, there is nothing left to re-run the query against,
    /// and delivering the notification unfiltered would tell the engine a
    /// narrowing was applied that never was.
    Filter(Box<Filter>),
    /// Do not send. 5.8.6 counts this as no attempt at all — the
    /// notification was never sent, so it is neither a success nor a
    /// failure, and `timesSent` does not move.
    Drop,
}

/// The seam. One implementation ships with the broker; every other one is
/// an addon crate a deployment builds itself.
pub trait PolicyEngine: Send + Sync {
    /// The name a deployment selects the engine by, and the name
    /// `/q/health` reports.
    fn name(&self) -> &str;

    /// Fires once per request, after negotiation and expansion, before the
    /// operation and before any fan-out (ADR-0014 `on_request`).
    fn decide<'a>(&'a self, subject: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a>;

    /// Fires once per batch (5.6.7 to 5.6.10, 5.6.20) with one Operation
    /// per member, so a batch is refused member by member and the allowed
    /// members are applied (207). The answer holds one Decision per
    /// Operation, in order; an answer of any other length denies the whole
    /// batch. The default asks [`decide`](Self::decide) once per member; an
    /// engine that asks a server overrides it to ask once.
    fn decide_each<'a>(
        &'a self,
        subject: &'a Subject,
        ops: &'a [Operation<'a>],
    ) -> DecisionsFuture<'a> {
        Box::pin(async move {
            let mut out = Vec::with_capacity(ops.len());
            for op in ops {
                out.push(self.decide(subject, op).await);
            }
            out
        })
    }

    /// The probe a surface lists its operations by (MCP D-3, T-3784): one
    /// Operation per clause, naming nothing, and each answer says whether
    /// some request of that clause could be allowed for `subject`, never
    /// whether this one is. The default decides the empty Operations as
    /// [`decide_each`](Self::decide_each) would; an engine whose rules need
    /// a type to answer a read overrides it, so a grant that takes the
    /// queried types is offered, and a refusal of every request is not.
    fn offers_each<'a>(
        &'a self,
        subject: &'a Subject,
        ops: &'a [Operation<'a>],
    ) -> DecisionsFuture<'a> {
        self.decide_each(subject, ops)
    }

    /// Fires once per notification document per subscription, before the
    /// egress check and the send (ADR-0014 `pre_notify`). Synchronous: it
    /// sits inside the delivery the broker is about to make, and an engine
    /// that has to ask a server for every notification is a design the seam
    /// declines to make easy.
    fn pre_notify(
        &self,
        subject: &Subject,
        sub: &Value,
        notification: &mut Value,
    ) -> NotifyDecision;

    /// [`pre_notify`](Self::pre_notify) with the `@context` the notification's data is
    /// compacted with: the Subscription's own, so an engine expanding a type or Attribute
    /// name reads the IRI the subscriber meant, not the core context's (F-R2ANT1-002). The
    /// broker calls this one; an engine that reads no names keeps the default.
    fn pre_notify_in(
        &self,
        subject: &Subject,
        sub: &Value,
        notification: &mut Value,
        _ctx: &antares_jsonld::Context,
    ) -> NotifyDecision {
        self.pre_notify(subject, sub, notification)
    }

    /// Whether the engine reads rule documents through
    /// [`load`](Self::load) (ADR-0020 amendment). No by default: `/q/policy`
    /// then answers 404, as it does for a broker without an engine.
    fn loads_documents(&self) -> bool {
        false
    }

    /// Take `document` as the rule document of `tenant`, or drop that
    /// Tenant's document when it is `Value::Null`. An `Err` leaves the
    /// engine as it was: the previous document keeps deciding. The reason
    /// goes back to the admin caller who wrote the document and nowhere
    /// else, so it may name parts of it. The default refuses every
    /// document.
    ///
    /// # Errors
    /// Why the engine refuses the document.
    fn load(&self, tenant: &str, document: &Value) -> Result<(), String> {
        let _ = (tenant, document);
        Err(format!(
            "the policy engine {:?} reads no rule documents",
            self.name()
        ))
    }
}

/// The name of the engine the broker ships, which is also the name
/// `/q/health` reports when no engine is attached at all: the two are the
/// same decision — allow — and an operator reading it learns what the
/// broker does, not which object made it happen.
pub const BUILT_IN_NAME: &str = "allow-all";

/// The engine the broker ships, and the one conformance is asserted
/// against: it decides nothing, so the broker behaves exactly as it did
/// before the seam existed.
///
/// A deployment that attaches no engine does not get an instance of this:
/// it gets no engine at all ([`AppState::policy`](crate::state::AppState)
/// is `None`), and every gate answers the empty [`Filter`] without building
/// a [`Subject`], boxing a future or arming the [`TIMEOUT`] timer. The two
/// paths decide the same thing, which is what
/// `the_built_in_answer_is_the_bypassed_answer` holds them to.
pub struct AllowAll;

impl PolicyEngine for AllowAll {
    fn name(&self) -> &str {
        BUILT_IN_NAME
    }

    fn decide<'a>(&'a self, _subject: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(Decision::Allow))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// A `Filter` only means something where the broker can serve less than it
/// was asked for, and [`FILTERABLE`] is that list. Everywhere else the
/// narrowing would be silently dropped: "purge every entity, but only the
/// ones you may see" is a different operation from the one the client
/// asked for and would delete less than the 204 claims, and a create writes
/// its Entity or does not. Those operations take the strict reading: narrow
/// means refuse. An empty `Filter` asks for nothing and is an allow.
pub fn resolve(clause: &str, decision: Decision) -> Decision {
    match decision {
        Decision::AllowIf(_) if !WRITE_BY_ID.contains(&clause) => Decision::Deny(format!(
            "{clause} is not a write by id and cannot be conditioned on the stored Entity"
        )),
        Decision::AllowIf(f) if f.projects() => {
            Decision::Deny("a write condition selects Entities and projects nothing".into())
        }
        Decision::AllowIf(f) if Condition::of(&f).is_none() => Decision::Allow,
        Decision::Filter(f) if RECORD_LISTS.contains(&clause) && f.projects() => Decision::Deny(
            format!("{clause} lists records whole and a narrowing there cannot project"),
        ),
        Decision::Filter(f) if !FILTERABLE.contains(&clause) && !NAME_READS.contains(&clause) => {
            if f.is_empty() {
                Decision::Allow
            } else if WHOLE_TENANT.contains(&clause) {
                Decision::Deny(format!(
                    "{clause} acts on everything the tenant holds and cannot be narrowed"
                ))
            } else {
                Decision::Deny(format!(
                    "{clause} is performed whole or not at all and cannot be narrowed"
                ))
            }
        }
        other => other,
    }
}

/// Ask the engine, and fail closed. An engine that panics, or that runs
/// past [`TIMEOUT`], denies: a seam that waved the request through on its
/// own failure would turn a broken addon into an open door.
///
/// The panic is caught in place rather than on a spawned task, because the
/// task boundary would demand `'static` of the operation, which borrows the
/// request. It is caught TWICE, because an engine has two places to fail:
/// `decide` returns a boxed future, and a synchronous engine does its whole
/// decision in the call that builds that future — the reference engine is
/// `Box::pin(ready(self.judge(..)))` — so guarding only the future guards
/// the half that does nothing.
///
/// [`TIMEOUT`] can only race the future. Work done before the future exists
/// holds the executor thread, and no timer inside the same task can
/// interrupt it; an engine that blocks is a deployment's own bug, and the
/// bound that catches it is the request timeout in front of the broker.
pub async fn decide(engine: &dyn PolicyEngine, subject: &Subject, op: &Operation<'_>) -> Decision {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use futures_util::FutureExt as _;
        let built =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| engine.decide(subject, op)));
        let Ok(fut) = built else {
            tracing::error!("policy engine {} panicked; denying", engine.name());
            metrics::counter!("antares_policy_failures_total", "reason" => "panic").increment(1);
            return Decision::Deny(ENGINE_FAILED.to_owned());
        };
        let guarded = std::panic::AssertUnwindSafe(fut).catch_unwind();
        match tokio::time::timeout(*TIMEOUT, guarded).await {
            Ok(Ok(d)) => resolve(op.clause, d),
            Ok(Err(_)) => {
                tracing::error!("policy engine {} panicked; denying", engine.name());
                metrics::counter!("antares_policy_failures_total", "reason" => "panic")
                    .increment(1);
                Decision::Deny(ENGINE_FAILED.to_owned())
            }
            Err(_) => {
                tracing::error!(
                    "policy engine {} did not answer within {:?}; denying",
                    engine.name(),
                    *TIMEOUT
                );
                metrics::counter!("antares_policy_failures_total", "reason" => "timeout")
                    .increment(1);
                Decision::Deny(ENGINE_TIMED_OUT.to_owned())
            }
        }
    }
    // The browser build has no timer to race against and aborts on panic;
    // it also loads no addon, so the engine is always the built-in one.
    #[cfg(target_arch = "wasm32")]
    {
        resolve(op.clause, engine.decide(subject, op).await)
    }
}

/// [`decide`] for a batch: one answer per Operation, failing closed the
/// same way. A panic, a timeout (one [`TIMEOUT`] for the whole batch) or an
/// answer whose length does not match the batch denies every member, and
/// each answer passes [`resolve`] for its own clause.
pub async fn decide_each(
    engine: &dyn PolicyEngine,
    subject: &Subject,
    ops: &[Operation<'_>],
) -> Vec<Decision> {
    guarded(engine, ops, || engine.decide_each(subject, ops)).await
}

/// One batch answer of `engine` to `ops` from `ask`, failing closed as
/// [`decide_each`] documents, each answer passed through [`resolve`].
async fn guarded<'a>(
    engine: &dyn PolicyEngine,
    ops: &[Operation<'_>],
    ask: impl FnOnce() -> DecisionsFuture<'a>,
) -> Vec<Decision> {
    let all = |why: &str| vec![Decision::Deny(why.to_owned()); ops.len()];
    #[cfg(not(target_arch = "wasm32"))]
    let answer = {
        use futures_util::FutureExt as _;
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(ask));
        let Ok(fut) = built else {
            tracing::error!("policy engine {} panicked; denying", engine.name());
            metrics::counter!("antares_policy_failures_total", "reason" => "panic").increment(1);
            return all(ENGINE_FAILED);
        };
        let guarded = std::panic::AssertUnwindSafe(fut).catch_unwind();
        match tokio::time::timeout(*TIMEOUT, guarded).await {
            Ok(Ok(d)) => d,
            Ok(Err(_)) => {
                tracing::error!("policy engine {} panicked; denying", engine.name());
                metrics::counter!("antares_policy_failures_total", "reason" => "panic")
                    .increment(1);
                return all(ENGINE_FAILED);
            }
            Err(_) => {
                tracing::error!(
                    "policy engine {} did not answer within {:?}; denying",
                    engine.name(),
                    *TIMEOUT
                );
                metrics::counter!("antares_policy_failures_total", "reason" => "timeout")
                    .increment(1);
                return all(ENGINE_TIMED_OUT);
            }
        }
    };
    #[cfg(target_arch = "wasm32")]
    let answer = ask().await;
    if answer.len() != ops.len() {
        tracing::error!(
            "policy engine {} answered {} decisions for {} operations; denying",
            engine.name(),
            answer.len(),
            ops.len()
        );
        metrics::counter!("antares_policy_failures_total", "reason" => "length").increment(1);
        return all(ENGINE_FAILED);
    }
    answer
        .into_iter()
        .zip(ops)
        .map(|(d, op)| resolve(op.clause, d))
        .collect()
}

/// Whether any request of each of `clauses` could be allowed for the caller
/// of `headers`: one batch-shaped probe a surface lists its operations by
/// (MCP D-3), never a decision on a request. Each clause is asked as an
/// Operation that names nothing, through [`PolicyEngine::offers_each`],
/// failing closed as [`decide_each`] does (a panic, a timeout or a short
/// answer hides every clause), and a narrowing on a clause that cannot be
/// narrowed hides it. With no engine attached every clause is offered.
pub async fn offers(
    engine: Option<&dyn PolicyEngine>,
    tenant: &antares_model::TenantId,
    headers: &HeaderMap,
    clauses: &[&'static str],
) -> Vec<bool> {
    let Some(engine) = engine else {
        return vec![true; clauses.len()];
    };
    let ops: Vec<Operation<'_>> = clauses
        .iter()
        .map(|clause| Operation {
            clause,
            ids: &[],
            types: &[],
            attrs: &[],
            q: None,
            scope_q: None,
            geo: None,
            body: None,
            stored: None,
        })
        .collect();
    let subject = subject_of(tenant, headers);
    guarded(engine, &ops, || engine.offers_each(&subject, &ops))
        .await
        .into_iter()
        .map(|d| !matches!(d, Decision::Deny(_)))
        .collect()
}

/// Ask the engine about one notification, and fail closed: an engine that
/// panics drops the notification. The document it was handed may already be
/// half-edited, and the broker cannot know which half — so it is not sent.
pub fn pre_notify(
    engine: &dyn PolicyEngine,
    subject: &Subject,
    sub: &Value,
    notification: &mut Value,
    ctx: &antares_jsonld::Context,
) -> NotifyDecision {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        engine.pre_notify_in(subject, sub, notification, ctx)
    })) {
        Ok(d) => d,
        Err(_) => {
            tracing::error!(
                "policy engine {} panicked on a notification; dropping it",
                engine.name()
            );
            metrics::counter!("antares_policy_failures_total", "reason" => "panic").increment(1);
            NotifyDecision::Drop
        }
    }
}

/// The ProblemDetails `type` and `title` of a refusal. Table 6.3.2-1 names
/// no access-denied error, so none is invented under the ETSI namespace: a
/// refusal is answered with this broker's own URN, documented in the book
/// and noted in the 6.3.2 ledger entry. A client that reads error types can
/// tell a policy refusal from a spec error by the namespace alone.
pub const ACCESS_DENIED_TYPE: &str = "urn:antares:error:AccessDenied";
pub const ACCESS_DENIED_TITLE: &str = "AccessDenied";

/// The request headers copied into a [`Subject`], comma-separated
/// (`ANTARES_POLICY_SUBJECT_HEADERS`), matched case-insensitively and read
/// once at first use. Empty by default: an engine that wants an identity
/// names the header that carries it, and a broker that copied every header
/// into the subject would be handing an addon the whole request.
pub static SUBJECT_HEADERS: LazyLock<Vec<String>> = LazyLock::new(|| {
    std::env::var("ANTARES_POLICY_SUBJECT_HEADERS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
});

/// Drops every header in `names` (the subject headers) from a request whose
/// `l5d-client-id`, the mesh identity the Linkerd proxy in front of the
/// broker sets from the caller's mTLS certificate, is not in `from`
/// (`ANTARES_POLICY_SUBJECT_FROM`). Any pod that reaches the broker port can
/// write `x-jc-subject`; only the mesh identity tells the gateway from it. A
/// request stripped here names nobody: the engine treats it as the cluster's
/// own, and on a broker that requires a subject the API layer serves it only
/// with the admin bearer ([`crate::admin_auth::subject_or_admin`]). The
/// binary applies it once, to the request it accepted;
/// the in-process hops a surface makes carry the outer request's subject.
pub fn drop_subject_unless_from(headers: &mut HeaderMap, names: &[String], from: &[String]) {
    let believed = headers
        .get("l5d-client-id")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|id| from.iter().any(|f| f == id));
    if believed {
        return;
    }
    for name in names {
        headers.remove(name.as_str());
    }
}

/// Who this request is from, as far as the seam is concerned: the tenant it
/// addresses and the headers a deployment named. A header the request does
/// not carry is simply absent — the seam invents nothing.
pub fn subject_of(tenant: &antares_model::TenantId, headers: &HeaderMap) -> Subject {
    let mut carried = Vec::new();
    for name in SUBJECT_HEADERS.iter() {
        for value in headers.get_all(name.as_str()) {
            if let Ok(v) = value.to_str() {
                carried.push((name.clone(), v.to_owned()));
            }
        }
    }
    if let Ok(view) = VIEW.try_with(std::sync::Arc::clone) {
        carried.push((VIEW_PAIR.to_owned(), view.to_string()));
    }
    Subject {
        tenant: tenant.clone(),
        headers: carried,
    }
}

/// The member a Subscription carries its creator's subject in. A
/// broker-internal member like `__context` and `__via`: a client can
/// neither set it nor read it back, it is stripped from every served
/// representation and from the 5.8.1.4 copy forwarded to a Context Source,
/// and 5.2.12 defines no such member — the whole `__` prefix is the
/// broker's, so a member added later inherits every one of those rules
/// instead of a new list to forget.
pub(crate) const SUBJECT_MEMBER: &str = "__subject";

/// What a write by id hands the engine about the Entity it lands on.
#[derive(Debug, Default)]
pub(crate) struct PreImage {
    /// The stored Entity's types joined with the ones the request body
    /// names: an engine judging by type judges the Entity the write changes
    /// and what it changes it to (ADR-0020).
    pub types: Vec<String>,
    /// The stored Entity itself ([`Operation::stored`]).
    pub stored: Option<StoredImage>,
}

/// The [`PreImage`] of a write by id. The store is read once per request,
/// and only with an engine attached, so vanilla reads nothing more. An
/// absent Entity or a store error gives no stored image, and the write
/// answers it as before.
///
/// Decided (worker-2, T-3483, owner may override): the write still reads
/// the Entity again inside its own store operation. Handing it this image,
/// read outside the store's transaction, would turn a concurrent write
/// into a lost update; the gate's read is the one the type lookup already
/// made, so no read is added.
pub(crate) async fn pre_image(
    st: &crate::state::AppState,
    tenant: &antares_model::TenantId,
    id: &str,
    body: Option<&Value>,
) -> Result<PreImage, antares_model::NgsiError> {
    if st.policy.is_none() {
        return Ok(PreImage::default());
    }
    // A failed read is the request's failure, never an absent Entity: a
    // missing pre-image would let a rule keyed on the stored types or
    // headers decide over nothing (fail-closed, ADR-0020; F-ANT2-009).
    let stored = st
        .store
        .get(tenant, antares_store::Kind::Entity, id)
        .await?;
    Ok(image(stored, body))
}

/// The [`PreImage`] of a temporal write by id (5.6.12 to 5.6.16, T-3607):
/// the Entity's temporal evolution, which a history-only Entity has
/// without a current state, else its current state. Read only with an
/// engine attached, as [`pre_image`].
pub(crate) async fn temporal_pre_image(
    st: &crate::state::AppState,
    tenant: &antares_model::TenantId,
    id: &str,
) -> Result<PreImage, antares_model::NgsiError> {
    if st.policy.is_none() {
        return Ok(PreImage::default());
    }
    match st.temporal.get(tenant, id).await? {
        Some(evolution) => Ok(image(Some(evolution), None)),
        None => pre_image(st, tenant, id, None).await,
    }
}

/// The image of a stored document, with the types the body adds.
pub(crate) fn image(stored: Option<Value>, body: Option<&Value>) -> PreImage {
    let mut types: Vec<String> = Vec::new();
    for doc in stored.as_ref().into_iter().chain(body) {
        for t in entity_types(doc) {
            if !types.iter().any(|o| o == t) {
                types.push(t.to_owned());
            }
        }
    }
    PreImage {
        types,
        stored: stored.as_ref().map(StoredImage::of),
    }
}

/// The stored form of a subject: only the headers a deployment named, since
/// the tenant is where the subscription already lives. `None` when the
/// subject carries nothing, so a deployment that named no header stores no
/// member at all.
pub(crate) fn subject_member(subject: &Subject) -> Option<Value> {
    (!subject.headers.is_empty())
        .then(|| serde_json::to_value(&subject.headers).ok())
        .flatten()
}

/// The subject a notification is delivered under: the one stored when the
/// subscription was created. A subscription created before the deployment
/// named its headers — or by the broker itself, for the 5.8.1.4 internal
/// copies — simply has none, and the engine is asked about a subject with
/// no headers rather than about the wrong one.
pub(crate) fn stored_subject(tenant: &antares_model::TenantId, sub: &Value) -> Subject {
    Subject {
        tenant: tenant.clone(),
        headers: sub
            .get(SUBJECT_MEMBER)
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default(),
    }
}

/// Remove every broker-internal member from a document about to be served.
///
/// The `__` prefix is the broker's: the notification `@context` (5.8.6), the
/// 6.3.18 Via chain, a snapshot's synthetic tenant and [`SUBJECT_MEMBER`]
/// all live under it, and no NGSI-LD data type defines a member there. One
/// predicate rather than a list per document type, so a member added later
/// is hidden by construction instead of by remembering every serve point.
pub(crate) fn strip_internal(doc: &mut Value) {
    if let Some(o) = doc.as_object_mut() {
        o.retain(|k, _| !k.starts_with("__"));
    }
}

/// Whether a stored document was made by this subject. Compared on the
/// headers alone: the tenant is where the document already lives, and a
/// deployment that named no header has no subjects to tell apart — every
/// request is the same one, and everything it stored is its own.
pub(crate) fn belongs_to(doc: &Value, subject: &Subject) -> bool {
    stored_subject(&subject.tenant, doc).headers == subject.headers
}

/// A refusal on its way out of a handler. Its own type rather than an
/// `ApiError`, so this module names nothing above it and stays a leaf; the
/// `?` in a handler converts it through `From`.
#[derive(Debug, Clone, PartialEq)]
pub struct Denied(pub String);

/// The one call a handler makes. Every operation passes through it exactly
/// once, which is what `every_route_asks_the_policy_engine_exactly_once` asserts by
/// walking the router with a counting engine.
///
/// The returned [`Filter`] is what the answer has to be narrowed by; an
/// allow returns the empty filter, which narrows nothing.
pub async fn gate(
    engine: &dyn PolicyEngine,
    tenant: &antares_model::TenantId,
    headers: &HeaderMap,
    op: &Operation<'_>,
) -> Result<Filter, Denied> {
    match decide(engine, &subject_of(tenant, headers), op).await {
        Decision::Allow => Ok(Filter::default()),
        // On a write by id the filter is the condition ([`Condition::of`]);
        // `resolve` lets an `AllowIf` through nowhere else, and turns a
        // `Filter` there into a deny, so a handler of such a clause reads
        // what it gets back as the condition and nothing else.
        Decision::Filter(f) | Decision::AllowIf(f) => {
            metadata::hold(f.metadata.as_deref())?;
            Ok(f)
        }
        Decision::Deny(why) => Err(Denied(why)),
    }
}

/// [`gate`](fn@gate) for a batch: per member the refusal reason, or the
/// condition its stored Entity has to meet (empty when there is none). A
/// batch writes its members whole, so a `Filter` on a member is a refusal
/// ([`resolve`]: no batch clause is FILTERABLE).
pub async fn gate_each(
    engine: &dyn PolicyEngine,
    tenant: &antares_model::TenantId,
    headers: &HeaderMap,
    ops: &[Operation<'_>],
) -> Vec<Result<Filter, String>> {
    decide_each(engine, &subject_of(tenant, headers), ops)
        .await
        .into_iter()
        .map(|d| match d {
            Decision::Deny(why) => Err(why),
            Decision::AllowIf(f) => Ok(f),
            Decision::Allow | Decision::Filter(_) => Ok(Filter::default()),
        })
        .collect()
}

/// A stored Subscription or Registration as a [`Condition`] reads it: `id`,
/// `type` and `scope` as an Entity carries them, every other member one
/// Property holding its value, and `subject` the carried header values of
/// the subject that created it ([`SUBJECT_MEMBER`]), one instance each, so
/// an engine selects the caller's own with `q=subject=="<its value>"`. A
/// member a client wrote under `subject` is not the subject and is left
/// out, as are the broker's other `__` members.
// ponytail: values only, without the header names; an engine whose two
// headers can carry the same value tells them apart by a rule's ids.
fn record_image(doc: &Value) -> Value {
    let ctx = crate::repr::policy_context();
    let mut out = serde_json::Map::new();
    for (k, v) in doc.as_object().into_iter().flatten() {
        match k.as_str() {
            "id" | "scope" => {
                out.insert(k.clone(), v.clone());
            }
            "type" => {
                let t = v
                    .as_str()
                    .map_or_else(|| v.clone(), |t| ctx.expand_key(t).into());
                out.insert(k.clone(), t);
            }
            "subject" => {}
            k if k.starts_with("__") => {}
            k => {
                out.insert(ctx.expand_key(k), serde_json::json!([{ "value": v }]));
            }
        }
    }
    let mut carried: Vec<(String, String)> = doc
        .get(SUBJECT_MEMBER)
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    // The view is where the creator came from, not who they are: a view
    // id equal to some user's name must not select that user's documents.
    carried.retain(|(name, _)| name != VIEW_PAIR);
    if !carried.is_empty() {
        let values = carried
            .into_iter()
            .map(|(_, v)| serde_json::json!({ "value": v }));
        out.insert(ctx.expand_key("subject"), values.collect());
    }
    Value::Object(out)
}

/// What a write by id needs of the Entity it lands on
/// ([`Decision::AllowIf`]), in the form a stored document is read in. The
/// handler checks it inside the store's own read-modify-write, on the
/// document the store is about to change.
///
/// The same selection a read's narrowing makes ([`crate::repr::PolicyReach`]),
/// judged synchronously on one document.
pub(crate) type Condition = crate::repr::PolicyReach;

impl Condition {
    /// The selection `f` makes, whatever it projects.
    pub(crate) fn new(f: &Filter) -> Self {
        Self {
            q: f.q.as_deref().cloned(),
            scope_q: f.scope_q.clone(),
            areas: f.areas().cloned().collect(),
            rules: crate::repr::type_rules(f),
        }
    }

    /// The condition `f` sets, or `None` when it selects every Entity.
    pub(crate) fn of(f: &Filter) -> Option<Self> {
        let c = Self::new(f);
        (!c.selects_every()).then_some(c)
    }

    /// Whether the condition selects every Entity: no `q`, no `scopeQ`, no
    /// area and no rules.
    pub(crate) fn selects_every(&self) -> bool {
        self.q.is_none() && self.scope_q.is_none() && self.areas.is_empty() && self.rules.is_none()
    }

    /// Whether the stored document `doc` is selected. A linked path in a
    /// `q` never holds, as in a rule (ADR-0023 rule 11): the check runs
    /// inside the store's write and reads no other Entity.
    pub(crate) fn holds(&self, doc: &Value) -> bool {
        let ctx = crate::repr::policy_context();
        crate::repr::shown(self.rules.as_deref(), doc).is_some()
            && self
                .scope_q
                .as_deref()
                .is_none_or(|s| crate::scope_matches(s, doc))
            && self.areas.iter().all(|g| g.matches(doc, ctx))
            && self.q.as_ref().is_none_or(|q| {
                !q.has_linked_paths() && antares_ql::eval::eval_q(q, doc, ctx, &|_| None)
            })
    }

    /// Whether a Subscription or Registration is selected (T-3673): the
    /// condition read over [`record_image`] of the stored document.
    pub(crate) fn holds_for_record(&self, doc: &Value) -> bool {
        self.holds(&record_image(doc))
    }

    /// Inside the store closure: an Entity the condition does not select is
    /// not found, the answer an absent one gets.
    pub(crate) fn pre(
        cond: Option<&Self>,
        id: &str,
        doc: &Value,
    ) -> Result<(), antares_model::NgsiError> {
        if cond.is_none_or(|c| c.holds(doc)) {
            Ok(())
        } else {
            Err(antares_model::NgsiError::ResourceNotFound(format!(
                "entity {id} not found"
            )))
        }
    }

    /// [`Condition::pre`] on a Temporal Evolution, inside the temporal
    /// store's closure: judged on [`latest_image`], and one it does not
    /// select is the absent evolution the temporal writes answer.
    pub(crate) fn pre_evolution(
        cond: Option<&Self>,
        id: &str,
        doc: &Value,
    ) -> Result<(), antares_model::NgsiError> {
        if cond.is_none_or(|c| c.holds(&latest_image(doc))) {
            Ok(())
        } else {
            Err(antares_model::NgsiError::ResourceNotFound(format!(
                "temporal entity {id} not found"
            )))
        }
    }

    /// A conditioned write that would reach a Context Source is refused:
    /// nothing in this store can judge the Source's copy.
    pub(crate) fn unforwarded(cond: Option<&Self>, forwards: bool) -> Result<(), Denied> {
        if cond.is_some() && forwards {
            Err(Denied(CONDITION_NOT_FORWARDED.to_owned()))
        } else {
            Ok(())
        }
    }
}

/// A Temporal Evolution as the Entity it now is: each Attribute keeps its
/// latest instance per `datasetId` (the greatest `observedAt`, else
/// `createdAt`; the later one on a tie) and a temporal `scope` the value of
/// its latest instance. A condition judged on every instance would let a
/// value the Entity held once select it for good.
// ponytail: instances without a time compare as the earliest; the temporal
// writes stamp `createdAt` on every instance they store.
pub(crate) fn latest_image(doc: &Value) -> Value {
    let time = |i: &Value| {
        ["observedAt", "createdAt"]
            .iter()
            .find_map(|k| i.get(*k).and_then(Value::as_str))
            .map(antares_model::dt_key)
            .unwrap_or_default()
    };
    let mut out = serde_json::Map::new();
    for (k, v) in doc.as_object().into_iter().flatten() {
        let instances = match v.as_array() {
            Some(a) if !antares_model::is_meta(k) || k == "scope" => a,
            _ => {
                out.insert(k.clone(), v.clone());
                continue;
            }
        };
        if !instances.iter().all(Value::is_object) {
            out.insert(k.clone(), v.clone());
            continue;
        }
        let mut latest: Vec<&Value> = Vec::new();
        for i in instances {
            let ds = i.get("datasetId");
            match latest.iter_mut().find(|l| l.get("datasetId") == ds) {
                Some(l) if time(i) >= time(l) => *l = i,
                Some(_) => {}
                None => latest.push(i),
            }
        }
        let kept = if k == "scope" {
            latest
                .first()
                .and_then(|i| i.get("value"))
                .cloned()
                .unwrap_or(Value::Null)
        } else {
            Value::Array(latest.into_iter().cloned().collect())
        };
        out.insert(k.clone(), kept);
    }
    Value::Object(out)
}

/// The members of a JSON object, or nothing when it is not one.
#[cfg(any(test, feature = "test-kit"))]
fn members(v: &Value) -> Vec<String> {
    v.as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// The contract every engine has to hold, so an addon's own tests can call
/// it. It asserts the three things an engine can actually get wrong — it
/// stops answering, it hands back an answer the seam has to override, or it
/// puts something into a notification that was not there — and it asserts
/// them through the seam, so an engine that passes here passes as the
/// broker will call it.
///
/// The core runs it against [`AllowAll`]; `examples/plugin-example` runs it
/// against the reference engine.
#[cfg(any(test, feature = "test-kit"))]
pub async fn run_policy_contract(engine: &dyn PolicyEngine) {
    let name = engine.name();
    assert!(!name.is_empty(), "an engine answers to a name");

    let subject = Subject {
        tenant: antares_model::TenantId::default(),
        headers: vec![("X-Subject".into(), "someone".into())],
    };
    let doc = serde_json::json!({
        "id": "urn:ngsi-ld:Vehicle:1",
        "type": "Vehicle",
        "speed": {"type": "Property", "value": 10},
        "brand": {"type": "Property", "value": "Skoda"}
    });

    // The engine answers, and the answer is its own: a deny carrying one of
    // the seam's own reasons means it timed out or panicked instead.
    for clause in ["5.6.1", "5.7.2", "5.8.1"] {
        match decide(engine, &subject, &Operation::new(clause)).await {
            Decision::Allow => {}
            Decision::Deny(why) => assert!(
                why != ENGINE_TIMED_OUT && why != ENGINE_FAILED,
                "{name}: {clause} was denied by the seam, not by the engine: {why}"
            ),
            Decision::Filter(f) => {
                let mut narrowed = doc.clone();
                f.project(&mut narrowed);
                let before = members(&doc);
                for key in members(&narrowed) {
                    assert!(
                        before.contains(&key),
                        "{name}: the filter for {clause} added {key:?} to the answer"
                    );
                }
            }
            // `decide` answers a condition on anything but a write by id as
            // a deny, so reaching here is the seam's fault, not the engine's.
            // A contract harness (test-kit) fails by panicking, as asserts do.
            #[allow(clippy::panic)]
            Decision::AllowIf(_) => panic!("{name}: a condition on {clause} passed the seam"),
        }
    }

    // An operation the broker performs whole is allowed or refused, never
    // done to less than it says: WHOLE_TENANT because there is no narrowed
    // form of "delete everything", and the writes because no handler there
    // reads a Filter at all (FILTERABLE).
    for clause in WHOLE_TENANT
        .iter()
        .chain(["5.6.1", "5.6.6", "5.8.1", "5.9.2"].iter())
    {
        assert!(
            !matches!(
                decide(engine, &subject, &Operation::new(clause)).await,
                Decision::Filter(_)
            ),
            "{name}: {clause} was answered with a filter"
        );
    }

    // `pre_notify` holds the notification by `&mut`, which is the one place
    // an engine can widen rather than narrow: a member it puts there is a
    // member no subscriber asked for and no store answered with.
    let sub = serde_json::json!({"id": "urn:ngsi-ld:Subscription:1", "type": "Subscription"});
    let mut notification = serde_json::json!({
        "id": "urn:ngsi-ld:Notification:1",
        "type": "Notification",
        "subscriptionId": "urn:ngsi-ld:Subscription:1",
        "notifiedAt": "2026-01-01T00:00:00Z",
        "data": [doc.clone()]
    });
    let before = members(&notification);
    let entity_members = members(&doc);
    if pre_notify(engine, &subject, &sub, &mut notification, &CORE_CONTEXT) != NotifyDecision::Drop
    {
        for key in members(&notification) {
            assert!(
                before.contains(&key),
                "{name}: pre_notify added {key:?} to the notification"
            );
        }
        let served = notification
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for entity in &served {
            for key in members(entity) {
                assert!(
                    entity_members.contains(&key),
                    "{name}: pre_notify added {key:?} to a notified Entity"
                );
            }
        }
    }

    // And the seam stops waiting for this engine, whatever it does.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let slow = Slow(engine);
        assert_eq!(
            decide(&slow, &subject, &Operation::new("5.6.1")).await,
            Decision::Deny(ENGINE_TIMED_OUT.to_owned()),
            "{name}: the seam waited past its timeout instead of denying"
        );
    }
}

/// The same engine, one timeout slower: what the contract wraps it in to
/// prove the seam stops waiting.
#[cfg(all(any(test, feature = "test-kit"), not(target_arch = "wasm32")))]
struct Slow<'e>(&'e dyn PolicyEngine);

#[cfg(all(any(test, feature = "test-kit"), not(target_arch = "wasm32")))]
impl PolicyEngine for Slow<'_> {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn decide<'a>(&'a self, subject: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(async move {
            tokio::time::sleep(
                TIMEOUT
                    .saturating_mul(2)
                    .saturating_add(Duration::from_millis(50)),
            )
            .await;
            self.0.decide(subject, op).await
        })
    }

    fn pre_notify(&self, s: &Subject, sub: &Value, n: &mut Value) -> NotifyDecision {
        self.0.pre_notify(s, sub, n)
    }
}

/// The most id segments a probe narrows by; past them it narrows by the
/// literal alone.
const MAX_ID_SEGMENTS: usize = 64;

/// The exact id prefixes a rule's one id pattern forces, from the segments
/// the Tenant's ids show ([`antares_store::filter::id_pattern_segments`]):
/// the store reads them as ranges of an index in their own order, where the
/// literal of `types_attrs::id_narrowing` filtered a walk of the whole type, which for
/// a few late matches still took 10 s on dev (T-5661). `None` when the
/// pattern does not split or the store keeps no segments.
pub(crate) async fn id_prefixes(
    st: &crate::state::AppState,
    tenant: &antares_model::TenantId,
    rule: Option<&Rule>,
) -> Result<Option<Vec<String>>, antares_model::NgsiError> {
    let Some(seg) =
        rule.filter(|r| r.ids.is_empty())
            .and_then(|r| match r.id_patterns.as_slice() {
                [p] => antares_store::filter::id_pattern_segments(p),
                _ => None,
            })
    else {
        return Ok(None);
    };
    Ok(st
        .store
        .entity_id_segments(tenant, &seg.lead, seg.sep, MAX_ID_SEGMENTS)
        .await?
        .map(|segs| {
            segs.into_iter()
                .map(|s| format!("{s}{}", seg.tail))
                .collect()
        }))
}

/// The exact id prefixes every rule of `rules` forces, united: a necessary
/// condition of the rules' OR, as [`id_prefixes`] is of one rule. A query
/// under a dataset view whose Policy narrows by one `idPattern` walked every
/// Entity of the type for its few matches, the regex judged per row: 10 s and
/// the gateway's 503 for `limit=1` over 128 678 KPIs on prod (T-5787).
/// `None` when a rule that holds for anything does not split.
pub(crate) async fn rules_id_prefixes(
    st: &crate::state::AppState,
    tenant: &antares_model::TenantId,
    rules: &[Rule],
) -> Result<Option<Vec<String>>, antares_model::NgsiError> {
    let mut all = Vec::new();
    // a rule naming no type holds for nothing (`Rule::types`)
    for rule in rules.iter().filter(|r| !r.types.is_empty()) {
        match id_prefixes(st, tenant, Some(rule)).await? {
            Some(ps) => all.extend(ps),
            None => return Ok(None),
        }
    }
    Ok(Some(all).filter(|a| !a.is_empty()))
}

/// The `idPattern`s every rule of `rules` that holds for anything narrows
/// by, when that is all they narrow by beside their types and they all name
/// the same ones, each of which the store applies exactly
/// ([`antares_store::filter::id_pattern_sql`]): then the rules hold for
/// exactly the Entities of their types whose id matches one, and the store
/// decides, pages and counts them. A dataset Endpoint's Policy narrows its
/// type by one such pattern; judged per row, a count over 164 000 Trees
/// took past the gateway's 10 s (T-5826).
pub(crate) fn shared_id_patterns(rules: &[Rule]) -> Option<&[String]> {
    let mut typed = rules.iter().filter(|r| !r.types.is_empty());
    let first = typed.next()?;
    let narrows =
        |r: &Rule| !r.is_conditional() && r.ids.is_empty() && r.id_patterns == first.id_patterns;
    let exact = |p: &String| {
        antares_store::filter::id_pattern_sql(p).is_some()
            && antares_ql::regex::compile(&format!("^(?:{p})$")).is_ok()
    };
    (narrows(first)
        && typed.all(narrows)
        && !first.id_patterns.is_empty()
        && first.id_patterns.iter().all(exact))
    .then_some(first.id_patterns.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-4875: a subject header is believed from a listed mesh identity
    /// alone; a forged one from any other pod, or with no identity, is gone.
    #[test]
    fn subject_is_dropped_unless_from_a_listed_mesh_identity() {
        let names = vec!["x-jc-subject".to_owned(), "x-jc-groups".to_owned()];
        let from = vec![
            "context-gateway.context-gateway.serviceaccount.identity.linkerd.cluster.local"
                .to_owned(),
        ];
        let request = |id: Option<&str>| {
            let mut h = HeaderMap::new();
            h.insert("x-jc-subject", "alice".parse().unwrap());
            h.append("x-jc-groups", "admins".parse().unwrap());
            h.insert("x-other", "kept".parse().unwrap());
            if let Some(id) = id {
                h.insert("l5d-client-id", id.parse().unwrap());
            }
            h
        };
        let mut gateway = request(Some(from[0].as_str()));
        drop_subject_unless_from(&mut gateway, &names, &from);
        assert_eq!(gateway.get("x-jc-subject").unwrap(), "alice");
        assert_eq!(gateway.get("x-jc-groups").unwrap(), "admins");
        for id in [
            None,
            Some("default.scratch.serviceaccount.identity.linkerd.cluster.local"),
            Some(""),
        ] {
            let mut h = request(id);
            drop_subject_unless_from(&mut h, &names, &from);
            assert!(h.get("x-jc-subject").is_none(), "{id:?}");
            assert!(h.get("x-jc-groups").is_none(), "{id:?}");
            assert_eq!(h.get("x-other").unwrap(), "kept");
        }
    }

    /// `admits` is `shown(..).is_some()` without the set (T-3667).
    #[test]
    fn admits_is_whether_shown_answers() {
        let ctx = antares_jsonld::core_context();
        let car = serde_json::json!({"id": "urn:ngsi-ld:Car:1", "type": "Car"});
        let rule = |t: &str| Rule {
            types: vec![t.to_owned()],
            ..Rule::default()
        };
        let cases = [
            Filter::default(),
            Filter {
                omit_by_type: BTreeMap::from([("Car".to_owned(), vec!["speed".to_owned()])]),
                ..Filter::default()
            },
            Filter {
                rules: vec![rule("Car")],
                ..Filter::default()
            },
            Filter {
                rules: vec![rule("User")],
                ..Filter::default()
            },
            Filter {
                rules: vec![rule("User"), rule("Car")],
                ..Filter::default()
            },
        ];
        let admitted: Vec<bool> = cases.iter().map(|f| f.admits(&car, &ctx)).collect();
        assert_eq!(admitted, [true, true, true, false, true]);
        for f in &cases {
            assert_eq!(f.admits(&car, &ctx), f.shown(&car, &ctx).is_some(), "{f:?}");
        }
    }

    /// T-3684: an engine that keeps the trait's defaults reads no rule
    /// documents (ADR-0020 amendment): it says so, so `/q/policy` answers
    /// 404, and a document handed to it anyway is refused, never taken.
    #[test]
    fn an_engine_on_the_defaults_reads_no_rule_documents() {
        assert!(!AllowAll.loads_documents());
        let refused = AllowAll.load("city", &serde_json::json!({"rules": []}));
        assert!(
            refused
                .as_ref()
                .is_err_and(|why| why.contains(BUILT_IN_NAME)),
            "{refused:?}"
        );
    }

    /// T-3684: `offers` answers one verdict per clause, in the clauses'
    /// order, false for a Deny; with no engine every clause is offered.
    #[tokio::test]
    async fn offers_answers_each_clause_in_order() {
        struct DenyRetrieve;
        impl PolicyEngine for DenyRetrieve {
            fn name(&self) -> &str {
                "deny-retrieve"
            }
            fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
                Box::pin(std::future::ready(if op.clause == "retrieveEntity" {
                    Decision::Deny("no".into())
                } else {
                    Decision::Allow
                }))
            }
            fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
                NotifyDecision::Deliver
            }
        }
        let (tenant, headers) = (antares_model::TenantId::default(), HeaderMap::new());
        let clauses = ["queryEntity", "retrieveEntity", "queryTemporal"];
        assert_eq!(
            offers(Some(&DenyRetrieve), &tenant, &headers, &clauses).await,
            [true, false, true]
        );
        assert_eq!(offers(None, &tenant, &headers, &clauses).await, [true; 3]);
    }

    /// T-3482: only the caller's own q is rewritten; the engine's
    /// condition may read what it hides from the caller.
    #[test]
    fn a_hidden_term_is_rewritten_and_the_engines_own_q_is_not() {
        let filter = Filter {
            omit: vec!["salary".into()],
            q: Some(Box::new(antares_ql::parse_q("salary>0").expect("q"))),
            ..Filter::default()
        };
        let ctx = antares_jsonld::core_context();
        let params = std::collections::HashMap::from([(
            "q".to_owned(),
            "salary==1|!salary|name==\"x\"".to_owned(),
        )]);
        let out = filter.narrow_params(&params, &ctx).expect("narrowed");
        assert_eq!(
            out["q"],
            "(salary;!salary|(salary|!salary)|name==\"x\");salary>0"
        );
        let nothing_hidden = Filter::default();
        let out = nothing_hidden
            .narrow_params(&params, &ctx)
            .expect("narrowed");
        assert_eq!(out["q"], params["q"]);
    }
    use futures_util::FutureExt as _;

    const NS: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

    fn iri(n: &str) -> String {
        format!("{NS}{n}")
    }

    /// Zone A: the square (0,0)-(1,1).
    fn zone_a() -> Box<GeoQuery> {
        let params = std::collections::HashMap::from([
            ("georel".to_owned(), "within".to_owned()),
            ("geometry".to_owned(), "Polygon".to_owned()),
            (
                "coordinates".to_owned(),
                "[[[0,0],[1,0],[1,1],[0,1],[0,0]]]".to_owned(),
            ),
        ]);
        Box::new(
            GeoQuery::from_params(&params)
                .expect("geoquery")
                .expect("present"),
        )
    }

    fn rule(t: &str, attrs: &[&str]) -> Rule {
        Rule {
            types: vec![iri(t)],
            attributes: attrs.iter().map(|a| iri(a)).collect(),
            ..Rule::default()
        }
    }

    /// `{Car in zone A: speed, location, status}`, `{Car: status}`,
    /// `{User: name}`, in the internal form `repr::type_rules` produces.
    fn zones() -> Filter {
        Filter {
            rules: vec![
                Rule {
                    geo_q: Some(zone_a()),
                    attributes: vec![
                        iri("speed"),
                        antares_ql::geo::LOCATION_IRI.to_owned(),
                        iri("status"),
                    ],
                    ..rule("Car", &[])
                },
                rule("Car", &["status"]),
                rule("User", &["name"]),
            ],
            ..Filter::default()
        }
    }

    fn car(lon: f64) -> Value {
        json!({"id": "urn:ngsi-ld:Car:1", "type": [iri("Car")],
               (iri("speed")): [{"type": "Property", "value": 80}],
               (iri("status")): [{"type": "Property", "value": "ok"}],
               "https://uri.etsi.org/ngsi-ld/location": [{"type": "GeoProperty",
                   "value": {"type": "Point", "coordinates": [lon, 0.5]}}]})
    }

    fn ctx() -> antares_jsonld::Context {
        antares_jsonld::core_context()
    }

    /// Gap G2: a rule that narrows ids selects by id, and a document with no
    /// id only when it narrows none.
    #[test]
    fn a_rule_selects_by_type_and_id() {
        let any = rule("Car", &["speed"]);
        let one = Rule {
            ids: vec!["urn:ngsi-ld:Car:1".into()],
            ..rule("Car", &["speed"])
        };
        let doc = car(0.5);
        let anonymous = json!({"type": [iri("Car")]});
        assert!(any.selects(&doc) && one.selects(&doc));
        assert!(any.selects(&anonymous), "no id narrowing, no id needed");
        assert!(!one.selects(&anonymous), "an id narrowing needs an id");
        assert!(!one.selects(&json!({"id": "urn:ngsi-ld:Car:2", "type": [iri("Car")]})));
        assert!(!one.selects(&json!({"id": "urn:ngsi-ld:Car:1", "type": [iri("Bus")]})));
    }

    /// ADR-0023 rule 11: the union over the rules that hold, nothing for an
    /// Entity none holds for, and an omit on any type wins.
    #[test]
    fn shown_is_the_union_of_the_rules_that_hold_and_any_omit_wins() {
        let f = zones();
        let ctx = ctx();
        let inside = f.shown(&car(0.5), &ctx).expect("in view");
        let outside = f.shown(&car(5.0), &ctx).expect("in view");
        assert!(inside.allows(&iri("speed")) && inside.allows(&iri("status")));
        assert!(!outside.allows(&iri("speed")), "speed only in zone A");
        assert!(outside.allows(&iri("status")));
        let robot = json!({"id": "urn:ngsi-ld:Robot:1", "type": [iri("Robot")]});
        assert_eq!(f.shown(&robot, &ctx), None, "no rule holds: out");
        let both = json!({"id": "urn:x", "type": [iri("Car"), iri("User")],
                          (iri("name")): [{"type": "Property", "value": "n"}]});
        let shown = f.shown(&both, &ctx).expect("in view");
        assert!(
            shown.allows(&iri("name")) && shown.allows(&iri("status")),
            "union"
        );
        let omitting = Filter {
            omit_by_type: BTreeMap::from([(iri("User"), vec![iri("status")])]),
            ..zones()
        };
        let shown = omitting.shown(&both, &ctx).expect("in view");
        assert!(!shown.allows(&iri("status")), "an omit on any type wins");
        let only_omit = Filter {
            omit_by_type: omitting.omit_by_type.clone(),
            ..Filter::default()
        };
        let shown = only_omit.shown(&robot, &ctx).expect("no rules: in");
        assert!(shown.allows(&iri("serial")), "no rules: omit alone");
    }

    /// A rule whose `q` follows a link never holds (fail closed), and a
    /// rule naming no type holds for nothing.
    #[test]
    fn a_rule_reading_a_link_or_naming_no_type_never_holds() {
        let ctx = ctx();
        let linked = Rule {
            q: Some(Box::new(
                antares_ql::parse_q("ownedBy{name}==\"x\"").expect("q"),
            )),
            ..rule("Car", &["speed"])
        };
        assert!(linked.q.as_deref().is_some_and(QNode::has_linked_paths));
        assert!(!linked.holds(&car(0.5), &ctx));
        let negated = Rule {
            q: Some(Box::new(antares_ql::parse_q("!ownedBy{name}").expect("q"))),
            ..rule("Car", &["speed"])
        };
        assert!(!negated.holds(&car(0.5), &ctx), "not even negated");
        assert!(!Rule::default().holds(&car(0.5), &ctx));
        let local = Rule {
            q: Some(Box::new(antares_ql::parse_q("speed>50").expect("q"))),
            ..rule("Car", &["status"])
        };
        assert!(local.holds(&car(0.5), &ctx));
    }

    /// ADR-0023 rule 7 per rule (K22): a write names only Attributes the
    /// rules that hold for its post-image show.
    #[test]
    fn write_that_adds_speed_outside_zone_a_is_refused() {
        let f = zones();
        let ctx = ctx();
        let speed = iri("speed");
        let status = iri("status");
        assert!(f.admits_write(&car(0.5), &[&speed, &status], &ctx));
        assert!(
            !f.admits_write(&car(5.0), &[&speed], &ctx),
            "speed outside A"
        );
        assert!(f.admits_write(&car(5.0), &[&status], &ctx));
        let name = iri("name");
        assert!(
            !f.admits_write(&car(0.5), &[&speed, &name], &ctx),
            "no one rule shows both"
        );
        let robot = json!({"id": "urn:ngsi-ld:Robot:1", "type": [iri("Robot")]});
        assert!(!f.admits_write(&robot, &[], &ctx), "outside every rule");
    }

    /// A type's list split over two rules of the same types (the Portal
    /// splits past 256 names) is written whole: a write naming one name of
    /// each part is admitted, and a name neither part shows is still refused
    /// (T-4753: 17,025 open-data rows refused on prod).
    #[test]
    fn a_write_across_a_split_list_is_admitted_and_an_unlisted_name_refused() {
        let ctx = ctx();
        let f = Filter {
            rules: vec![rule("Car", &["speed"]), rule("Car", &["status"])],
            ..Filter::default()
        };
        let (speed, status, name) = (iri("speed"), iri("status"), iri("name"));
        assert!(f.admits_write(&car(5.0), &[&speed, &status], &ctx));
        assert!(!f.admits_write(&car(5.0), &[&speed, &name], &ctx));
        let user = json!({"id": "urn:ngsi-ld:User:1", "type": [iri("User")]});
        assert!(!f.admits_write(&user, &[&speed], &ctx), "no rule holds");
    }

    /// A rule over every Attribute admits a write naming any of them, and
    /// discloses nothing it reads (T-3637).
    #[test]
    fn a_rule_over_every_attribute_admits_any_written_name() {
        let ctx = ctx();
        let whole = Rule {
            every_attribute: true,
            q: Some(Box::new(antares_ql::parse_q("speed>50").expect("q"))),
            ..rule("Car", &[])
        };
        let f = Filter {
            rules: vec![whole.clone()],
            ..Filter::default()
        };
        let (speed, name) = (iri("speed"), iri("name"));
        assert!(f.admits_write(&car(0.5), &[&speed, &name], &ctx));
        let narrow = Filter {
            rules: vec![rule("Car", &[])],
            ..Filter::default()
        };
        assert!(!narrow.admits_write(&car(0.5), &[&speed], &ctx));
        assert!(whole.discloses(&ctx).is_empty());
    }

    /// The loader's disclosure report: zone A's rule reads `location` and
    /// shows it, so it discloses nothing; a rule on `speed>100` showing only
    /// `status` discloses `speed`.
    #[test]
    fn discloses_names_what_the_condition_reads_and_the_rule_does_not_show() {
        let ctx = ctx();
        assert!(zones().rules[0].discloses(&ctx).is_empty());
        let fast = Rule {
            q: Some(Box::new(antares_ql::parse_q("speed>100").expect("q"))),
            geo_q: Some(zone_a()),
            ..rule("Car", &["status"])
        };
        assert_eq!(
            fast.discloses(&ctx),
            vec![
                iri("speed"),
                "https://uri.etsi.org/ngsi-ld/location".to_owned()
            ]
        );
    }

    #[test]
    fn project_keeps_the_union_and_the_frame() {
        let f = Filter {
            rules: vec![
                Rule {
                    types: vec!["Car".to_owned()],
                    attributes: vec!["speed".to_owned(), "email".to_owned()],
                    ..Rule::default()
                },
                Rule {
                    types: vec!["User".to_owned()],
                    attributes: vec!["name".to_owned()],
                    ..Rule::default()
                },
            ],
            omit_by_type: BTreeMap::from([("User".to_owned(), vec!["email".to_owned()])]),
            ..Filter::default()
        };
        let mut doc = json!({"id": "urn:x", "type": ["Car", "User"], "@context": "c",
                             "speed": 1, "name": 2, "email": 3, "serial": 4});
        f.project(&mut doc);
        assert_eq!(
            doc,
            json!({"id": "urn:x", "type": ["Car", "User"], "@context": "c",
                   "speed": 1, "name": 2})
        );
    }
    use serde_json::json;

    fn subject() -> Subject {
        Subject {
            tenant: antares_model::TenantId::default(),
            headers: vec![("X-Subject".into(), "a-token-shaped-string".into())],
        }
    }

    /// Run the contract and report whether it refused the engine.
    async fn contract_holds(engine: &dyn PolicyEngine) -> bool {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::AssertUnwindSafe(run_policy_contract(engine))
            .catch_unwind()
            .await;
        std::panic::set_hook(hook);
        outcome.is_ok()
    }

    #[tokio::test]
    async fn the_built_in_engine_holds_the_contract() {
        run_policy_contract(&AllowAll).await;
    }

    /// An engine that narrows is what the seam is for, and the contract
    /// must not stand in its way.
    struct Narrowing;

    impl PolicyEngine for Narrowing {
        fn name(&self) -> &str {
            "narrowing"
        }
        fn decide<'a>(&'a self, _s: &'a Subject, _o: &'a Operation<'a>) -> DecisionFuture<'a> {
            Box::pin(std::future::ready(Decision::Filter(Filter {
                omit: vec!["brand".into()],
                restricted: true,
                ..Filter::default()
            })))
        }
        fn pre_notify(&self, _s: &Subject, _sub: &Value, n: &mut Value) -> NotifyDecision {
            if let Some(data) = n.get_mut("data").and_then(Value::as_array_mut) {
                for entity in data {
                    if let Some(o) = entity.as_object_mut() {
                        o.remove("brand");
                    }
                }
            }
            NotifyDecision::Filter(Box::new(Filter {
                omit: vec!["brand".into()],
                ..Filter::default()
            }))
        }
    }

    #[tokio::test]
    async fn an_engine_that_only_narrows_holds_the_contract() {
        assert!(contract_holds(&Narrowing).await);
    }

    /// The contract has teeth or it proves nothing: this engine puts a
    /// member into a notification nobody asked for.
    struct Widening;

    impl PolicyEngine for Widening {
        fn name(&self) -> &str {
            "widening"
        }
        fn decide<'a>(&'a self, _s: &'a Subject, _o: &'a Operation<'a>) -> DecisionFuture<'a> {
            Box::pin(std::future::ready(Decision::Allow))
        }
        fn pre_notify(&self, _s: &Subject, _sub: &Value, n: &mut Value) -> NotifyDecision {
            if let Some(o) = n.as_object_mut() {
                o.insert("stowaway".into(), json!(true));
            }
            NotifyDecision::Deliver
        }
    }

    #[tokio::test]
    async fn the_contract_refuses_an_engine_that_widens_a_notification() {
        assert!(
            !contract_holds(&Widening).await,
            "the contract passed an engine that added a member to a notification"
        );
    }

    #[test]
    fn a_pick_keeps_the_frame_and_drops_the_rest() {
        let mut doc = json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle",
                             "speed": 1, "brand": "Skoda"});
        Filter {
            pick: vec!["speed".into()],
            ..Filter::default()
        }
        .project(&mut doc);
        assert_eq!(
            doc,
            json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle", "speed": 1})
        );
    }

    #[test]
    fn an_omit_cannot_remove_what_makes_it_an_entity() {
        let mut doc = json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle", "speed": 1});
        Filter {
            omit: vec!["id".into(), "type".into(), "speed".into()],
            ..Filter::default()
        }
        .project(&mut doc);
        assert_eq!(
            doc,
            json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle"})
        );
    }

    #[test]
    fn a_pick_of_something_absent_adds_nothing() {
        let mut doc = json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle", "speed": 1});
        Filter {
            pick: vec!["mileage".into()],
            ..Filter::default()
        }
        .project(&mut doc);
        assert_eq!(
            doc,
            json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle"})
        );
    }

    #[test]
    fn a_projection_leaves_a_non_object_alone() {
        let mut doc = json!(["not", "an", "object"]);
        Filter {
            pick: vec!["speed".into()],
            omit: vec!["brand".into()],
            ..Filter::default()
        }
        .project(&mut doc);
        assert_eq!(doc, json!(["not", "an", "object"]));
    }

    #[test]
    fn narrowing_a_whole_tenant_operation_is_a_refusal() {
        let narrowed = Decision::Filter(Filter {
            omit: vec!["speed".into()],
            ..Filter::default()
        });
        for clause in WHOLE_TENANT {
            assert!(matches!(
                resolve(clause, narrowed.clone()),
                Decision::Deny(_)
            ));
        }
        // and every other clause outside FILTERABLE, because the handler
        // there never reads the Filter
        for clause in ["5.6.1", "5.6.6", "5.8.1", "5.9.2", "5.13.2", "5.8.4"] {
            assert!(
                matches!(resolve(clause, narrowed.clone()), Decision::Deny(_)),
                "{clause} dropped a narrowing instead of refusing it"
            );
        }
    }

    /// The reads that do read it keep it: the seam refuses what would be
    /// dropped, and nothing more.
    #[test]
    fn a_filterable_read_keeps_its_narrowing() {
        let narrowed = Decision::Filter(Filter {
            omit: vec!["speed".into()],
            ..Filter::default()
        });
        let selected = Decision::Filter(Filter {
            scope_q: Some("/mine".into()),
            ..Filter::default()
        });
        for clause in FILTERABLE {
            assert_eq!(resolve(clause, selected.clone()), selected, "{clause}");
            if RECORD_LISTS.contains(&clause) {
                // a record list selects and never projects (T-3673)
                assert!(matches!(
                    resolve(clause, narrowed.clone()),
                    Decision::Deny(_)
                ));
            } else {
                assert_eq!(resolve(clause, narrowed.clone()), narrowed, "{clause}");
            }
        }
    }

    #[test]
    fn an_empty_filter_on_a_whole_tenant_operation_is_not_a_refusal() {
        assert_eq!(
            resolve("5.6.21", Decision::Filter(Filter::default())),
            Decision::Allow
        );
    }

    struct Panicking;

    impl PolicyEngine for Panicking {
        fn name(&self) -> &str {
            "panicking"
        }
        fn decide<'a>(&'a self, _s: &'a Subject, _o: &'a Operation<'a>) -> DecisionFuture<'a> {
            Box::pin(async { panic!("the engine is broken") })
        }
        fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
            panic!("the engine is broken")
        }
    }

    /// The other half of the same failure: a synchronous engine decides in
    /// the call that BUILDS the future, so a guard around the future alone
    /// never sees it. The reference engine has exactly this shape.
    struct PanickingEagerly;

    impl PolicyEngine for PanickingEagerly {
        fn name(&self) -> &str {
            "panicking-eagerly"
        }
        fn decide<'a>(&'a self, _s: &'a Subject, _o: &'a Operation<'a>) -> DecisionFuture<'a> {
            panic!("the engine is broken")
        }
        fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
            NotifyDecision::Deliver
        }
    }

    #[tokio::test]
    async fn an_engine_that_panics_before_it_returns_a_future_denies_too() {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let decided = decide(&PanickingEagerly, &subject(), &Operation::new("5.6.1")).await;
        std::panic::set_hook(hook);
        assert_eq!(decided, Decision::Deny(ENGINE_FAILED.to_owned()));
    }

    #[tokio::test]
    async fn an_engine_that_panics_denies_rather_than_allows() {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let decided = decide(&Panicking, &subject(), &Operation::new("5.6.1")).await;
        let mut notification = json!({"type": "Notification"});
        let notified = pre_notify(
            &Panicking,
            &subject(),
            &json!({}),
            &mut notification,
            &CORE_CONTEXT,
        );
        std::panic::set_hook(hook);
        assert_eq!(decided, Decision::Deny(ENGINE_FAILED.to_owned()));
        assert_eq!(notified, NotifyDecision::Drop);
    }

    /// Real time, not a paused clock: the workspace tokio is built without
    /// `test-util`, so the test pays the timeout it asserts.
    #[tokio::test]
    async fn an_engine_that_never_answers_denies() {
        struct Never;
        impl PolicyEngine for Never {
            fn name(&self) -> &str {
                "never"
            }
            fn decide<'a>(&'a self, _s: &'a Subject, _o: &'a Operation<'a>) -> DecisionFuture<'a> {
                Box::pin(std::future::pending())
            }
            fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
                NotifyDecision::Deliver
            }
        }
        assert_eq!(
            decide(&Never, &subject(), &Operation::new("5.6.1")).await,
            Decision::Deny(ENGINE_TIMED_OUT.to_owned())
        );
    }

    #[test]
    fn the_subject_never_prints_its_header_values() {
        let printed = format!("{:?}", subject());
        assert!(printed.contains("X-Subject"), "{printed}");
        assert!(
            !printed.contains("a-token-shaped-string"),
            "a header value reached a log line: {printed}"
        );
    }

    #[test]
    fn an_operation_never_prints_the_payload() {
        let body = json!({"id": "urn:ngsi-ld:Vehicle:1", "plate": "BB-123-XY"});
        let printed = format!(
            "{:?}",
            Operation {
                body: Some(&body),
                ..Operation::new("5.6.1")
            }
        );
        assert!(!printed.contains("BB-123-XY"), "{printed}");
    }

    /// T-3552: the policy survivors of mutants run 38073511508.
    #[test]
    fn the_timeout_takes_a_positive_integer_or_falls_back() {
        assert_eq!(timeout_ms(None), 250);
        assert_eq!(timeout_ms(Some("0")), 250, "zero would deny every request");
        assert_eq!(timeout_ms(Some("x")), 250);
        assert_eq!(timeout_ms(Some("1")), 1);
        assert_eq!(timeout_ms(Some("300")), 300);
    }

    #[test]
    fn a_single_string_counts_as_a_one_element_list() {
        let doc = json!({"type": "T", "scope": "/a"});
        assert_eq!(entity_types(&doc), ["T"]);
        assert_eq!(StoredImage::of(&doc).scope, ["/a"]);
        let listed = json!({"type": ["T", "U"], "scope": ["/a", "/b"]});
        assert_eq!(entity_types(&listed), ["T", "U"]);
        assert_eq!(StoredImage::of(&listed).scope, ["/a", "/b"]);
    }

    #[test]
    fn an_operation_logs_its_shape_and_never_its_payload() {
        let op = Operation::new("5.6.1");
        let shown = format!("{op:?}");
        assert!(shown.starts_with("Operation {"), "{shown}");
        assert!(shown.contains("clause: \"5.6.1\""), "{shown}");
        assert!(shown.contains("ids: 0"), "{shown}");
    }

    #[test]
    fn a_rule_is_conditional_on_any_one_of_its_conditions() {
        let plain = rule("Car", &["speed"]);
        assert!(!plain.is_conditional());
        let q = Rule {
            q: Some(Box::new(antares_ql::parse_q("speed>1").expect("q"))),
            ..plain.clone()
        };
        let scope = Rule {
            scope_q: Some("/a".to_owned()),
            ..plain.clone()
        };
        assert!(q.is_conditional() && scope.is_conditional());
    }

    #[test]
    fn shown_nothing_admits_no_attribute() {
        let nothing = Shown::nothing();
        assert_eq!(nothing.only, Some(Default::default()));
        assert!(!nothing.allows(&iri("name")));
    }

    #[test]
    fn a_write_is_refused_only_for_the_attribute_its_type_omits() {
        let ctx = ctx();
        let f = Filter {
            omit_by_type: BTreeMap::from([(iri("User"), vec![iri("status")])]),
            ..Filter::default()
        };
        let user = json!({"id": "urn:x", "type": [iri("User")]});
        assert!(f.admits_write(&user, &[iri("name").as_str()], &ctx));
        assert!(!f.admits_write(&user, &[iri("status").as_str()], &ctx));
    }

    /// T-3552 (mutants run 38081990103): the image of a stored Entity names
    /// its Attributes, never what frames, dates or annotates it.
    #[test]
    fn a_stored_image_names_attributes_only() {
        let doc = json!({"id": "urn:ngsi-ld:Car:1", "type": [iri("Car")], "scope": ["/a"],
                         "createdAt": "t", "modifiedAt": "t", "deletedAt": "t",
                         "@context": [], "__tenant": "acme",
                         (iri("speed")): [{"type": "Property", "value": 1}]});
        let image = StoredImage::of(&doc);
        assert_eq!(image.types, vec![iri("Car")]);
        assert_eq!(image.scope, vec!["/a".to_owned()]);
        assert_eq!(image.attribute_names, vec![iri("speed")]);
    }

    #[test]
    fn mapping_the_names_maps_both_sets() {
        let shown = Shown {
            only: Some(std::collections::BTreeSet::from([Cow::Borrowed("a")])),
            omitted: std::collections::BTreeSet::from([Cow::Borrowed("b")]),
        };
        let mapped = shown.map_names(str::to_uppercase);
        assert_eq!(
            mapped.only,
            Some(std::collections::BTreeSet::from([Cow::Borrowed("A")]))
        );
        assert_eq!(
            mapped.omitted,
            std::collections::BTreeSet::from([Cow::Borrowed("B")])
        );
    }

    /// T-3729: a notification's shown set outlives its rules unchanged; an
    /// empty set would show the whole Entity.
    #[test]
    fn an_owned_shown_set_keeps_both_sets() {
        let shown = Shown {
            only: Some(std::collections::BTreeSet::from([Cow::Borrowed("a")])),
            omitted: std::collections::BTreeSet::from([Cow::Borrowed("b")]),
        };
        let owned = shown.clone().into_owned();
        assert_eq!(owned, shown);
        assert!(owned.allows("a") && !owned.allows("b") && !owned.allows("c"));
    }

    /// T-3729: only a rule with no condition, ids or id patterns admits by
    /// type alone, the one the store may page as a type selection.
    #[test]
    fn a_rule_admits_by_type_alone_only_without_condition_or_ids() {
        let typed = Rule {
            types: vec![iri("Car")],
            ..Rule::default()
        };
        assert!(typed.admits_by_type_alone());
        for narrowed in [
            Rule {
                ids: vec!["urn:ngsi-ld:Car:1".into()],
                ..typed.clone()
            },
            Rule {
                id_patterns: vec!["urn:.*".into()],
                ..typed.clone()
            },
            Rule {
                scope_q: Some("/a/#".into()),
                ..typed.clone()
            },
        ] {
            assert!(!narrowed.admits_by_type_alone(), "{narrowed:?}");
        }
    }

    /// T-3729: the broker's `__` members of a stored Subscription or
    /// Registration are no Attributes a condition reads.
    #[test]
    fn a_record_image_leaves_the_brokers_members_out() {
        let image = record_image(&json!({"id": "urn:ngsi-ld:Subscription:1",
            "type": "Subscription", "__tenant": "acme", "description": "d"}));
        let keys: Vec<&String> = image
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys.len(), 3, "{image}");
        assert!(keys.iter().all(|k| !k.contains("__")), "{image}");
    }

    #[test]
    fn a_filter_says_its_areas_its_rules_and_its_restriction() {
        let both = Filter {
            geo_q: Some(zone_a()),
            geo_also: vec![*zone_a()],
            ..Filter::default()
        };
        assert_eq!(both.areas().count(), 2);
        let also = Filter {
            geo_also: vec![*zone_a()],
            ..Filter::default()
        };
        assert_eq!(also.areas().count(), 1);
        assert!(!also.is_empty());

        assert!(!Filter::default().has_rules());
        assert!(zones().has_rules(), "rules alone");
        let omits = Filter {
            omit_by_type: BTreeMap::from([(iri("User"), vec![iri("status")])]),
            ..Filter::default()
        };
        assert!(omits.has_rules(), "per-type omits alone");

        let mut headers = HeaderMap::new();
        Filter::default().mark("me", &mut headers);
        assert!(headers.get(RESTRICTED_HEADER).is_none());
        assert!(headers.get("NGSILD-Warning").is_none());
        Filter {
            restricted: true,
            ..Filter::default()
        }
        .mark("me", &mut headers);
        assert_eq!(
            headers.get(RESTRICTED_HEADER).and_then(|v| v.to_str().ok()),
            Some("true")
        );
        assert!(headers.get("NGSILD-Warning").is_none(), "only when set");
    }

    /// G6: the request's window clamped to the narrowing's, written back
    /// into the parameters; nothing changes without windows, a missing or
    /// invalid window stays refused, and no overlap is a window of nothing.
    #[test]
    fn a_temporal_window_is_clamped_into_the_parameters() {
        let p = |pairs: &[(&str, &str)]| -> std::collections::HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        let t = |s: &str| TimeBound::At(s.parse().unwrap());
        let between = Filter {
            temporal: Box::new([Window {
                from: Some(t("2026-01-01T01:00:00Z")),
                to: Some(t("2026-01-01T02:00:00Z")),
            }]),
            ..Filter::default()
        };
        let odd = p(&[("timerel", "sideways")]);
        assert_eq!(
            Filter::default().narrow_window(&odd, true).unwrap(),
            odd,
            "no windows, no reading"
        );
        assert!(
            between.narrow_window(&p(&[]), true).is_err(),
            "5.7.4 still demands a window"
        );
        assert!(between.narrow_window(&odd, false).is_err());

        let out = between.narrow_window(&p(&[]), false).unwrap();
        assert_eq!(out["timerel"], "between");
        assert_eq!(out["timeAt"], "2026-01-01T01:00:00.000Z");
        assert_eq!(out["endTimeAt"], "2026-01-01T02:00:00.000Z");

        let out = between
            .narrow_window(
                &p(&[("timerel", "after"), ("timeAt", "2026-01-01T01:30:00Z")]),
                true,
            )
            .unwrap();
        assert_eq!(
            (out["timerel"].as_str(), out["timeAt"].as_str()),
            ("between", "2026-01-01T01:30:00Z")
        );

        let out = between
            .narrow_window(
                &p(&[("timerel", "before"), ("timeAt", "2025-01-01T00:00:00Z")]),
                true,
            )
            .unwrap();
        assert_eq!(
            out["timeAt"], out["endTimeAt"],
            "no overlap: between an instant and itself"
        );

        let open = Filter {
            temporal: Box::new([Window {
                from: None,
                to: Some(t("2026-01-01T02:00:00Z")),
            }]),
            ..Filter::default()
        };
        let out = open
            .narrow_window(
                &p(&[
                    ("timerel", "between"),
                    ("timeAt", "2025-01-01T00:00:00Z"),
                    ("endTimeAt", "2027-01-01T00:00:00Z"),
                ]),
                true,
            )
            .unwrap();
        assert_eq!(
            (out["timeAt"].as_str(), out["endTimeAt"].as_str()),
            ("2025-01-01T00:00:00Z", "2026-01-01T02:00:00.000Z")
        );
        let out = open.narrow_window(&p(&[]), false).unwrap();
        assert_eq!(out["timerel"], "before");
        assert!(!out.contains_key("endTimeAt"));
        let empty = Filter {
            temporal: Box::new([Window::default()]),
            ..Filter::default()
        };
        assert_eq!(
            empty.narrow_window(&p(&[]), false).unwrap(),
            p(&[]),
            "an unbounded window narrows nothing"
        );

        // a relative start rounds up to the millisecond, an end down
        let slide = Filter {
            temporal: Box::new([Window {
                from: Some(TimeBound::Ago(chrono::Duration::nanoseconds(1))),
                to: Some(TimeBound::Ago(chrono::Duration::nanoseconds(1))),
            }]),
            ..Filter::default()
        };
        let out = slide.narrow_window(&p(&[]), false).unwrap();
        assert!(antares_model::dt_key(&out["timeAt"]) >= antares_model::dt_key(&out["endTimeAt"]));

        // the windows are on observedAt: a request on another timeproperty
        // is answered nothing, even one whose own window overlaps
        let out = between
            .narrow_window(
                &p(&[
                    ("timerel", "after"),
                    ("timeAt", "2026-01-01T01:30:00Z"),
                    ("timeproperty", "modifiedAt"),
                ]),
                true,
            )
            .unwrap();
        assert_eq!(out["timeAt"], out["endTimeAt"], "{out:?}");
        assert_eq!(out["timeAt"], "2026-01-01T01:00:00.000Z", "{out:?}");
    }

    /// A bound renders to the millisecond 4.11 takes: a start rounds up
    /// (only ever by less than a millisecond), an end down, an exact
    /// millisecond stays; a window is half-open on those renderings.
    #[test]
    fn a_bound_rounds_only_inward_and_a_window_is_half_open() {
        let at = |ns: u32| {
            TimeBound::At(chrono::DateTime::from_timestamp(1_767_225_600, ns).expect("an instant"))
        };
        let now = chrono::Utc::now();
        for (ns, start, end) in [
            (0, "00.000", "00.000"),
            (500, "00.001", "00.000"),
            (999_000, "00.001", "00.000"),
            (1_000_500, "00.002", "00.001"),
        ] {
            assert_eq!(
                at(ns).render(now, true),
                format!("2026-01-01T00:00:{start}Z"),
                "start at {ns} ns"
            );
            assert_eq!(
                at(ns).render(now, false),
                format!("2026-01-01T00:00:{end}Z"),
                "end at {ns} ns"
            );
        }
        let w = Window {
            from: Some(at(0)),
            to: Some(at(2_000_000)),
        };
        assert!(
            !w.admits("2025-12-31T23:59:59.999Z", now),
            "before the start"
        );
        assert!(
            w.admits("2026-01-01T00:00:00.000Z", now),
            "the start is inside"
        );
        assert!(w.admits("2026-01-01T00:00:00.001Z", now), "inside");
        assert!(
            !w.admits("2026-01-01T00:00:00.002Z", now),
            "the end is outside"
        );
        assert!(!w.admits("2026-01-01T00:00:00.003Z", now), "after the end");
    }

    /// T-3668: an evolution is judged as the Entity it now is, per
    /// Attribute and `datasetId`, whatever order its instances are stored in.
    #[test]
    fn the_latest_image_keeps_the_latest_instance_per_dataset() {
        let i = |v: &str, at: &str, ds: Option<&str>| {
            let mut i = serde_json::json!({"type": "Property", "value": v, "observedAt": at});
            if let Some(ds) = ds {
                i["datasetId"] = ds.into();
            }
            i
        };
        let doc = serde_json::json!({
            "id": "urn:ngsi-ld:V:1",
            "type": ["V"],
            "zone": [
                i("A", "2026-01-01T00:00:02Z", None),
                i("B", "2026-01-01T00:00:01Z", None),
                i("C", "2026-01-01T00:00:01Z", Some("urn:ds:1")),
                i("D", "2026-01-01T00:00:03Z", Some("urn:ds:1")),
                i("E", "2026-01-01T00:00:02Z", None),
            ],
            "scope": [
                {"type": "Property", "value": ["/old"], "observedAt": "2026-01-01T00:00:01Z"},
                {"type": "Property", "value": ["/new"], "observedAt": "2026-01-01T00:00:02Z"},
            ],
            "modifiedAt": "2026-01-01T00:00:03Z",
        });
        let img = latest_image(&doc);
        let zone: Vec<&str> = img["zone"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i["value"].as_str())
            .collect();
        assert_eq!(zone, ["E", "D"], "a tie goes to the later one");
        assert_eq!(img["scope"], serde_json::json!(["/new"]));
        assert_eq!(img["type"], doc["type"]);
        assert_eq!(img["modifiedAt"], doc["modifiedAt"]);
        let plain = serde_json::json!({"id": "urn:ngsi-ld:V:1", "scope": "/a"});
        assert_eq!(latest_image(&plain), plain);
    }

    /// G8: the engine's warning travels as one `NGSILD-Warning` value per
    /// text, and a text that could not be one header line is refused.
    #[test]
    fn a_condition_holds_only_on_a_write_by_id_and_only_as_a_selection() {
        let zone = Filter {
            q: Some(Box::new(antares_ql::parse_q("zone==\"A\"").expect("q"))),
            ..Filter::default()
        };
        let conditioned = Decision::AllowIf(zone.clone());
        for clause in WRITE_BY_ID {
            assert_eq!(
                resolve(clause, conditioned.clone()),
                conditioned,
                "{clause}"
            );
            assert_eq!(
                resolve(clause, Decision::AllowIf(Filter::default())),
                Decision::Allow,
                "{clause}: an empty condition selects everything"
            );
        }
        for clause in ["5.6.1", "5.6.7", "5.6.16", "5.6.21", "5.7.1", "5.8.1"] {
            assert!(
                matches!(resolve(clause, conditioned.clone()), Decision::Deny(_)),
                "{clause}"
            );
        }
        for projecting in [
            Filter {
                pick: vec!["speed".into()],
                ..zone.clone()
            },
            Filter {
                omit: vec!["speed".into()],
                ..zone.clone()
            },
            Filter {
                temporal: Box::new([Window::default()]),
                ..zone.clone()
            },
            Filter {
                omit_by_type: BTreeMap::from([("Vehicle".into(), vec!["speed".into()])]),
                ..zone.clone()
            },
            Filter {
                rules: vec![Rule {
                    types: vec!["Vehicle".into()],
                    temporal: vec![Window::default()],
                    ..Rule::default()
                }],
                ..Filter::default()
            },
        ] {
            assert!(
                matches!(
                    resolve("5.6.2", Decision::AllowIf(projecting.clone())),
                    Decision::Deny(_)
                ),
                "{projecting:?}"
            );
        }

        let dc = "https://uri.etsi.org/ngsi-ld/default-context/";
        let doc = |z: &str| {
            serde_json::json!({"id": "urn:ngsi-ld:Vehicle:v", "type": [format!("{dc}Vehicle")],
                               format!("{dc}zone"): [{"type": "Property", "value": z}]})
        };
        let c = Condition::of(&zone).expect("a condition");
        assert!(c.holds(&doc("A")) && !c.holds(&doc("B")));
        assert!(Condition::pre(Some(&c), "urn:ngsi-ld:Vehicle:v", &doc("A")).is_ok());
        assert!(matches!(
            Condition::pre(Some(&c), "urn:ngsi-ld:Vehicle:v", &doc("B")),
            Err(antares_model::NgsiError::ResourceNotFound(m))
                if m == "entity urn:ngsi-ld:Vehicle:v not found"
        ));
        assert!(Condition::pre(None, "urn:ngsi-ld:Vehicle:v", &doc("B")).is_ok());
        let linked = Filter {
            q: Some(Box::new(
                antares_ql::parse_q("owner.zone==\"A\"").expect("q"),
            )),
            ..Filter::default()
        };
        assert!(
            !Condition::of(&linked)
                .expect("a condition")
                .holds(&doc("A")),
            "a linked path reads no other Entity inside the write, so it never holds"
        );
        let scoped = Filter {
            scope_q: Some("/a".to_owned()),
            ..Filter::default()
        };
        assert!(
            Condition::of(&scoped).is_some(),
            "a scopeQ alone selects: it is a condition"
        );
        let vehicles = Condition::of(&Filter {
            rules: vec![Rule {
                types: vec!["Vehicle".into()],
                ..Rule::default()
            }],
            ..Filter::default()
        })
        .expect("a type rule alone selects: it is a condition");
        let car = serde_json::json!({"id": "urn:ngsi-ld:Car:c", "type": [format!("{dc}Car")]});
        assert!(vehicles.holds(&doc("B")) && !vehicles.holds(&car));
        assert!(Condition::of(&Filter::default()).is_none());
        assert!(Condition::unforwarded(Some(&c), false).is_ok());
        assert!(Condition::unforwarded(None, true).is_ok());
        assert_eq!(
            Condition::unforwarded(Some(&c), true),
            Err(Denied(CONDITION_NOT_FORWARDED.into()))
        );
    }

    #[test]
    fn a_warning_is_one_header_line_or_refused() {
        let left_out = Warning::new("types left out by the filter: Building").unwrap();
        let mut headers = HeaderMap::new();
        Filter {
            warning: Some(left_out.clone()),
            ..Filter::default()
        }
        .mark("me", &mut headers);
        let sent: Vec<_> = headers
            .get_all("NGSILD-Warning")
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect();
        assert_eq!(sent, ["199 me \"types left out by the filter: Building\""]);

        for bad in [
            "a\nb",
            "a\rb",
            "tab\there",
            "say \"x\"",
            "back\\slash",
            "é",
            "",
        ] {
            assert!(Warning::new(bad).is_err(), "{bad:?}");
        }
        assert!(Warning::new(&"x".repeat(Warning::MAX_LEN)).is_ok());
        assert!(Warning::new(&"x".repeat(Warning::MAX_LEN + 1)).is_err());

        let other = Warning::new("other").unwrap();
        let joined = Warning::join(Some(&left_out), Some(other.clone())).unwrap();
        assert_eq!(joined.texts().len(), 2);
        assert_eq!(
            Warning::join(Some(&left_out), Some(left_out.clone())),
            Some(left_out.clone())
        );
        assert_eq!(
            Warning::join(None, Some(other.clone())),
            Some(other.clone())
        );
        assert_eq!(Warning::join(Some(&other), None), Some(other));
        let many = (0..10).fold(None, |acc: Option<Warning>, i| {
            Warning::join(acc.as_ref(), Some(Warning::new(&format!("w{i}")).unwrap()))
        });
        assert_eq!(many.unwrap().texts().len(), Warning::MAX_TEXTS);
    }

    #[test]
    fn only_a_pick_or_an_omit_hides_a_term() {
        let speed = iri("speed");
        assert!(!Filter::default().hides("speed", &speed));
        let pick = Filter {
            pick: vec!["status".into()],
            ..Filter::default()
        };
        assert!(pick.hides("speed", &speed));
        assert!(!pick.hides("status", &iri("status")));
        let omit = Filter {
            omit: vec!["speed".into()],
            ..Filter::default()
        };
        assert!(omit.hides("speed", &speed));
    }

    /// Refuses upserts, allows the rest; `short` answers one decision
    /// fewer than it was asked for.
    struct Batch {
        short: bool,
    }

    impl PolicyEngine for Batch {
        fn name(&self) -> &str {
            "batch"
        }
        fn decide<'a>(&'a self, _s: &'a Subject, o: &'a Operation<'a>) -> DecisionFuture<'a> {
            Box::pin(std::future::ready(if o.clause == "5.6.8" {
                Decision::Deny("no upserts".into())
            } else {
                Decision::Allow
            }))
        }
        fn decide_each<'a>(
            &'a self,
            s: &'a Subject,
            ops: &'a [Operation<'a>],
        ) -> DecisionsFuture<'a> {
            Box::pin(async move {
                let mut out = Vec::new();
                for op in ops {
                    out.push(self.decide(s, op).await);
                }
                if self.short {
                    out.pop();
                }
                out
            })
        }
        fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
            NotifyDecision::Deliver
        }
    }

    #[tokio::test]
    async fn a_batch_is_answered_member_by_member_and_a_short_answer_denies_all() {
        let ops = [Operation::new("5.6.7"), Operation::new("5.6.8")];
        let each = decide_each(&Batch { short: false }, &subject(), &ops).await;
        assert_eq!(
            each,
            vec![Decision::Allow, Decision::Deny("no upserts".into())]
        );
        let short = decide_each(&Batch { short: true }, &subject(), &ops).await;
        assert_eq!(short, vec![Decision::Deny(ENGINE_FAILED.to_owned()); 2]);
        let gated = gate_each(
            &Batch { short: false },
            &antares_model::TenantId::default(),
            &HeaderMap::new(),
            &ops,
        )
        .await;
        assert_eq!(
            gated,
            vec![Ok(Filter::default()), Err("no upserts".to_owned())]
        );
    }

    #[tokio::test]
    async fn the_pre_image_joins_stored_and_written_types_once_each() {
        let tenant = antares_model::TenantId::default();
        let id = "urn:ngsi-ld:Car:pre";
        let stored = json!({"id": id, "type": [iri("Car")],
                            (iri("speed")): [{"type": "Property", "value": 1}]});
        let body = json!({"type": [iri("Car"), iri("Vehicle")]});
        let bare = crate::state::AppState::new("pre-image".into());
        bare.store
            .create(&tenant, antares_store::Kind::Entity, id, stored)
            .await
            .expect("create");
        let none = pre_image(&bare, &tenant, id, Some(&body))
            .await
            .expect("read");
        assert!(
            none.types.is_empty() && none.stored.is_none(),
            "no engine, no read"
        );
        let st = bare.with_policy(std::sync::Arc::new(AllowAll));
        let image = pre_image(&st, &tenant, id, Some(&body))
            .await
            .expect("read");
        assert_eq!(image.types, vec![iri("Car"), iri("Vehicle")]);
        assert_eq!(
            image.stored.map(|s| s.attribute_names),
            Some(vec![iri("speed")])
        );
    }

    /// T-3702: a subject built inside a view's scope names the view, one
    /// built outside names none, and no request can carry the pair.
    #[tokio::test]
    async fn a_subject_names_the_view_it_came_through_and_nothing_else_can() {
        let tenant = antares_model::TenantId::default();
        let plain = subject_of(&tenant, &HeaderMap::new());
        assert_eq!(plain.view(), None);
        let inside = through_view("cars", async { subject_of(&tenant, &HeaderMap::new()) }).await;
        assert_eq!(inside.view(), Some("cars"));
        let nested = through_view(
            "cars",
            through_view("bikes", async { subject_of(&tenant, &HeaderMap::new()) }),
        )
        .await;
        assert_eq!(nested.view(), Some("bikes"), "the innermost view decides");
        assert!(
            axum::http::HeaderName::from_bytes(VIEW_PAIR.as_bytes()).is_err(),
            "{VIEW_PAIR} must not be a header name a client could send"
        );
    }

    /// T-3702: the view is stored with the subject (a notification is
    /// decided for it), and a condition never reads it as one of the
    /// creator's values.
    #[test]
    fn the_view_is_stored_with_the_subject_and_never_read_as_its_value() {
        let tenant = antares_model::TenantId::default();
        let subject = Subject {
            tenant: tenant.clone(),
            headers: vec![
                ("x-jc-subject".into(), "ana".into()),
                (VIEW_PAIR.into(), "ana".into()),
            ],
        };
        let mut doc = json!({"id": "urn:ngsi-ld:Subscription:1", "type": "Subscription"});
        if let (Some(o), Some(member)) = (doc.as_object_mut(), subject_member(&subject)) {
            o.insert(SUBJECT_MEMBER.into(), member);
        }
        let stored = stored_subject(&tenant, &doc);
        assert_eq!(stored.view(), Some("ana"));
        assert!(belongs_to(&doc, &subject));
        let through_other = Subject {
            tenant: tenant.clone(),
            headers: vec![
                ("x-jc-subject".into(), "ana".into()),
                (VIEW_PAIR.into(), "bikes".into()),
            ],
        };
        assert!(
            !belongs_to(&doc, &through_other),
            "another view, another creator"
        );
        let image = record_image(&doc);
        let values: Vec<&Value> = image
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, _)| k.ends_with("subject"))
            .flat_map(|(_, v)| v.as_array().into_iter().flatten())
            .collect();
        assert_eq!(values, [&json!({"value": "ana"})], "{image}");
    }

    /// T-5826: the store decides a view's reads only when every typed rule
    /// narrows by the same translatable `idPattern` set and nothing else;
    /// each guard of `shared_id_patterns` has its case.
    #[test]
    fn only_one_shared_translatable_pattern_set_is_left_to_the_store() {
        const TREES: &str =
            "^urn:ngsi-ld:Tree:[a-z0-9.-]+:praha-mesto:green-[a-z-]+-[0-9a-f-]{36}$";
        let tree = |patterns: &[&str]| Rule {
            id_patterns: patterns.iter().map(|p| (*p).to_owned()).collect(),
            ..rule("Tree", &["name"])
        };
        let untyped = Rule {
            types: Vec::new(),
            q: Some(Box::new(antares_ql::parse_q("height>5").expect("q"))),
            ..Rule::default()
        };
        let seeded = [tree(&[TREES]), untyped.clone()];
        assert_eq!(
            shared_id_patterns(&seeded),
            Some([TREES.to_owned()].as_slice())
        );
        let refused: [(&str, Vec<Rule>); 10] = [
            (
                "q",
                vec![Rule {
                    q: Some(Box::new(antares_ql::parse_q("height>5").expect("q"))),
                    ..tree(&[TREES])
                }],
            ),
            (
                "scopeQ",
                vec![Rule {
                    scope_q: Some("/praha".into()),
                    ..tree(&[TREES])
                }],
            ),
            (
                "geo_q",
                vec![Rule {
                    geo_q: Some(zone_a()),
                    ..tree(&[TREES])
                }],
            ),
            (
                "geo_also",
                vec![Rule {
                    geo_also: vec![*zone_a()],
                    ..tree(&[TREES])
                }],
            ),
            (
                "ids",
                vec![Rule {
                    ids: vec!["urn:ngsi-ld:Tree:1".into()],
                    ..tree(&[TREES])
                }],
            ),
            (
                "other patterns",
                vec![tree(&[TREES]), tree(&["^urn:ngsi-ld:Tree:other-[0-9]+$"])],
            ),
            ("no pattern after one", vec![tree(&[TREES]), tree(&[])]),
            ("no pattern before one", vec![tree(&[]), tree(&[TREES])]),
            ("no pattern", vec![tree(&[]), untyped]),
            ("untranslatable", vec![tree(&["urn:x:.*"])]),
        ];
        for (case, rules) in &refused {
            assert_eq!(shared_id_patterns(rules), None, "{case}");
        }
    }
}
