// SPDX-License-Identifier: EUPL-1.2
//! The view table (ADR-0023 decision block and rules 8, 10 and 11,
//! surfaces-threat-model 9.6 rule M4 and V23).
//!
//! A [`View`] is configuration: written to `/q/views/{id}`, validated before
//! it is stored, kept as a document under the reserved tenant, and loaded
//! into the table every pod serves from. The table is rebuilt from the
//! store after every write on the pod that took it, and on every pod each
//! [`RELOAD_ENV`] interval, then swapped whole; no restart. A stored view
//! that no longer validates (written by another version, or by hand) is not
//! served and `/q/health` names it with the reason. A surface that serves
//! through views is closed while no view is loaded.
//!
//! No code path builds a view that serves everything: `View` has no
//! `Default`, a view selects at least one type and shows its Entities
//! through at least one rule, and no environment variable makes one.
//!
//! ```compile_fail
//! let everything = antares_ops::views::View::default();
//! ```

use crate::mapping::Mapping;
use crate::OperationId;
use antares_api::policy::Rule;
use antares_api::views::{CompiledView, Schema, ViewEntry, SCHEMA_ID_PREFIX};
use antares_api::AppState;
use antares_jsonld::Context;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Weak};

mod preview;
pub use preview::MAX_PREVIEW_LIMIT;
pub mod erasure;
pub mod quality;
pub mod retention;
pub mod rule_documents;

/// How often, in milliseconds, every pod rebuilds its table from the store
/// (default 5000): the bound on how long a view written on one pod takes to
/// open, change or close on another.
pub const RELOAD_ENV: &str = "ANTARES_VIEWS_RELOAD_MS";

/// The largest `LinkML` schema document `/q/schemas/{tenant}` accepts, in
/// bytes. No default: unset, the broker takes no schema (T-3628).
pub const SCHEMA_MAX_ENV: &str = "ANTARES_SCHEMA_MAX_BYTES";

pub use antares_api::schema_writes::{Mode as SchemaWrites, SCHEMA_WRITES_ENV};

/// The platform's history retention (DM-97): attribute instances of an
/// Entity that no `historyRetention`, type rule or `*` rule of its Tenant
/// covers are deleted once older than this duration (`P28D`). Unset keeps
/// them.
pub const HISTORY_DEFAULT_ENV: &str = "ANTARES_RETENTION_HISTORY_DEFAULT";

/// How long audit rows (provenance `audit: true`, ADR-N-054 2.7-2.9) are
/// kept, whatever the history rules say (T-6337): `P2Y` unset. A duration
/// as the retention rules read it, or whole years (`P3Y`, 365 days each).
pub const AUDIT_RETENTION_ENV: &str = "ANTARES_RETENTION_AUDIT";

/// [`AUDIT_RETENTION_ENV`] unset: two years.
const AUDIT_RETENTION_DEFAULT: std::time::Duration = std::time::Duration::from_hours(2 * 365 * 24);

/// Every variable the view table reads. The broker accepts these as config
/// keys from here, so a new one is added in this crate alone (T-3766).
pub const ENV_KEYS: &[&str] = &[
    RELOAD_ENV,
    SCHEMA_MAX_ENV,
    HISTORY_DEFAULT_ENV,
    AUDIT_RETENTION_ENV,
    antares_api::schema_writes::SCHEMA_WRITES_ENV,
    crate::view_cache::VIEW_IDLE_ENV,
];

/// The largest view document `/q/views` accepts, in bytes.
pub const MAX_VIEW_BYTES: usize = 64 * 1024;

/// The most rules one view holds (V23: the predicate is bounded at load;
/// every request through the view evaluates each of them).
pub const MAX_RULES: usize = 64;

/// The one name that is no name: a view of every type and every
/// Attribute writes it alone (ADR-0023 rule 12).
pub const EVERY: &str = "*";

/// The most names in one list of a view: its selector, a rule's types or
/// Attributes, its operations.
pub const MAX_NAMES: usize = 256;

/// The longest `q` or `scopeQ` a view or a rule carries, in bytes.
pub const MAX_QUERY_BYTES: usize = 4096;

/// A view as written to `/q/views/{id}` (ADR-0023 decision block). Names
/// in `selector`, `rules[].types` and `rules[].attributes` are expanded
/// through the view's own `@context` (an inline object, over the core
/// context) at load. Every `q` is read against the core context: a
/// condition is the policy's, and a caller's `@context` must not change
/// what it means (ADR-0023 rule 11).
///
/// ```
/// let view = antares_ops::views::View::parse(br#"{
///   "id": "cars", "tenant": "city", "selector": ["Car"],
///   "rules": [{"types": ["Car"], "attributes": ["speed"]}],
///   "operations": ["retrieve_entity", "query_entities"]
/// }"#).expect("a view");
/// assert!(view.load().is_ok());
/// ```
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct View {
    /// Its name, `[a-z0-9-]{1,63}`, which is also its path segment.
    pub id: String,
    /// The one Tenant it serves.
    pub tenant: String,
    /// Expands the view's type and Attribute names.
    #[serde(rename = "@context", default)]
    pub context: Option<serde_json::Map<String, Value>>,
    /// The Entity types it selects, at least one.
    pub selector: Vec<String>,
    /// A query every Entity it serves satisfies (4.9).
    #[serde(default)]
    pub q: Option<String>,
    /// A scope query every Entity it serves satisfies (4.19).
    #[serde(default)]
    pub scope_q: Option<String>,
    /// An area every Entity it serves lies in (4.10).
    #[serde(default)]
    pub geo_q: Option<GeoQ>,
    /// What each Entity shows, at least one rule (ADR-0023 rule 11).
    pub rules: Vec<ViewRule>,
    /// The operations it allows, as `operation-matrix.yaml` ids.
    pub operations: Vec<String>,
    /// The compiled Mapping IR of a view of another model (gap G18,
    /// `docs/analysis/view-mapping.md` section 3): the view serves its
    /// target class, selects its source class alone, and allows only
    /// [`crate::mapping::READS`].
    #[serde(default)]
    pub mapping: Option<Value>,
    /// How a domain-standard surface renders the view's Entities, by
    /// surface name: [`REPRESENTATIONS`] lists the names, and the surface
    /// reads its own member (the `ical` field mapping, T-4901).
    #[serde(default)]
    pub representations: Option<serde_json::Map<String, Value>>,
}

/// The surfaces a view's `representations` may name.
pub const REPRESENTATIONS: &[&str] = &[
    "ical", "feed", "ogc", "geojson", "csv", "gbfs", "sta", "odata",
];

pub use crate::outputs::MAX_OUTPUTS;

/// One allowlist of a [`View`]: the Entities of `types` for which the
/// condition holds show `attributes`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ViewRule {
    /// Entity types, at least one.
    pub types: Vec<String>,
    /// A condition on the Entity's own members.
    #[serde(default)]
    pub q: Option<String>,
    /// A scope condition.
    #[serde(default)]
    pub scope_q: Option<String>,
    /// An area condition.
    #[serde(default)]
    pub geo_q: Option<GeoQ>,
    /// The Attributes an Entity this rule holds for shows.
    pub attributes: Vec<String>,
}

/// A geoquery (4.10) as the request parameters of 7.2.5 name it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoQ {
    /// `near;maxDistance==…`, `within`, `intersects` and the others.
    pub georel: String,
    /// `Point`, `Polygon` and the other `GeoJSON` geometry types.
    pub geometry: String,
    /// The coordinates, as a JSON array or as its text.
    pub coordinates: Value,
    /// The `GeoProperty` compared, `location` when absent.
    #[serde(default)]
    pub geoproperty: Option<String>,
}

/// A view that passed validation.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The narrowing every operation through the view is composed with.
    pub view: Arc<CompiledView>,
    /// The one Tenant it serves.
    pub tenant: String,
    /// The operations it allows.
    pub operations: Vec<OperationId>,
    /// What an operator should know: rules whose condition reads an
    /// Attribute they do not show, a temporal view keyed on a mutable
    /// member.
    pub notes: Vec<String>,
    /// The compiled Mapping of a view of another model.
    pub mapping: Option<Arc<crate::mapping::Mapping>>,
    /// The view's own `@context`, as written.
    pub context: Option<Arc<serde_json::Map<String, Value>>>,
    /// The view's `representations`, as written.
    pub representations: Option<Arc<serde_json::Map<String, Value>>>,
}

impl View {
    /// A view from the bytes of its document: at most [`MAX_VIEW_BYTES`],
    /// and no member the view does not define.
    ///
    /// ```
    /// assert!(antares_ops::views::View::parse(b"{}").is_err());
    /// ```
    ///
    /// # Errors
    /// Why the bytes are not a view.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_VIEW_BYTES {
            return Err(format!("a view document is at most {MAX_VIEW_BYTES} bytes"));
        }
        serde_json::from_slice(bytes).map_err(|e| format!("not a view: {e}"))
    }

    /// Whether this is ADR-0023 rule 12's view of every type and every
    /// Attribute: `selector ["*"]` and the one rule `{types ["*"],
    /// attributes ["*"]}` with no condition of its own. `*` anywhere else is
    /// an error, before any name is expanded, so no `@context` turns it into
    /// a name.
    fn is_every(&self) -> Result<bool, String> {
        let star = |names: &[String]| names.iter().any(|n| n == EVERY);
        let only = |names: &[String]| names.len() == 1 && star(names);
        if !star(&self.selector)
            && !self
                .rules
                .iter()
                .any(|r| star(&r.types) || star(&r.attributes))
        {
            return Ok(false);
        }
        match self.rules.as_slice() {
            [r] if only(&self.selector)
                && only(&r.types)
                && only(&r.attributes)
                && r.q.is_none()
                && r.scope_q.is_none()
                && r.geo_q.is_none() =>
            {
                Ok(true)
            }
            _ => Err(format!(
                "`{EVERY}` stands alone (ADR-0023 rule 12): selector [\"{EVERY}\"] and the one \
                 rule {{\"types\": [\"{EVERY}\"], \"attributes\": [\"{EVERY}\"]}}"
            )),
        }
    }

    /// Validate the view and compile it (ADR-0023 rule 10): its id, its
    /// tenant, its names expanded, every query parsed within its bounds,
    /// every operation a row of the matrix.
    ///
    /// ```
    /// let view = antares_ops::views::View::parse(br#"{
    ///   "id": "cars", "tenant": "city", "selector": [],
    ///   "rules": [{"types": ["Car"], "attributes": ["speed"]}],
    ///   "operations": ["query_entities"]
    /// }"#).expect("a view document");
    /// assert!(view.load().is_err(), "a view selects at least one type");
    /// ```
    ///
    /// # Errors
    /// The first reason the view cannot be served.
    pub fn load(&self) -> Result<Loaded, String> {
        if !valid_id(&self.id) {
            return Err("a view id is 1 to 63 of [a-z0-9-]".into());
        }
        let tenant = antares_model::TenantId::new(&self.tenant)
            .map_err(|_| format!("{:?} is not a tenant a client can name", self.tenant))?;
        let mut ctx = antares_jsonld::core_context();
        if let Some(local) = &self.context {
            ctx.merge_object(local)
                .map_err(|e| format!("the view's @context: {e}"))?;
        }
        let (q, scope, geo) = (
            parse_q("q", self.q.as_deref())?,
            scope_q("scopeQ", self.scope_q.as_deref())?,
            geo_q("geoQ", self.geo_q.as_ref())?,
        );
        let operations = operations(&self.operations)?;
        let (view, mut notes, selector) = if self.is_every()? {
            (
                CompiledView::every(q, scope, geo),
                Vec::new(),
                vec![EVERY.to_owned()],
            )
        } else {
            let selector = expand_all("selector", &self.selector, &ctx)?;
            if self.rules.len() > MAX_RULES {
                return Err(format!("a view holds at most {MAX_RULES} rules"));
            }
            let rules = self
                .rules
                .iter()
                .enumerate()
                .map(|(i, r)| compile_rule(i, r, &ctx))
                .collect::<Result<Vec<_>, _>>()?;
            let notes = disclosures(&rules);
            (
                CompiledView::new(&selector, q, scope, geo, rules)?,
                notes,
                selector,
            )
        };
        let mapping = match &self.mapping {
            None => None,
            Some(ir) => {
                let mut mapping = mapped(ir, &ctx, self.context.as_ref(), &selector, &operations)?;
                notes.extend(hidden_slots_note(&mut mapping, &view));
                Some(Arc::new(mapping))
            }
        };
        if let Some(named) = &self.representations {
            if let Some((name, _)) = named.iter().find(|(name, member)| {
                // each is a list of named outputs: `feed`, `ogc`, `geojson` and `csv` check theirs when
                // it serves, `ical` (T-5208) refuses more than MAX_OUTPUTS here
                !REPRESENTATIONS.contains(&name.as_str())
                    || member
                        .as_array()
                        .is_none_or(|outputs| *name == "ical" && outputs.len() > MAX_OUTPUTS)
            }) {
                return Err(format!(
                    "representations: {name:?} is not one of {REPRESENTATIONS:?} with a list of outputs (ical: at most {MAX_OUTPUTS})"
                ));
            }
        }
        let view = view.named(&self.id);
        let mut loaded = Loaded {
            view: Arc::new(view),
            tenant: tenant.as_str().to_owned(),
            operations,
            notes,
            mapping,
            context: self.context.clone().map(Arc::new),
            representations: self.representations.clone().map(Arc::new),
        };
        if self.q.is_some()
            && loaded
                .operations
                .iter()
                .any(|op| op.id().starts_with("temporal"))
        {
            // ADR-0023 rule 8: membership is the current state, so an Entity
            // that enters the view brings its earlier history along.
            loaded.notes.push(
                "the view serves history and its q reads Attributes that change: an Entity \
                 entering the view brings its earlier history along (ADR-0023 rule 8)"
                    .into(),
            );
        }
        Ok(loaded)
    }
}

/// The note of a mapped view whose rules hide what some slots read: such a
/// slot is never served nor read by a filter (T-5210).
fn hidden_slots_note(mapping: &mut Mapping, view: &CompiledView) -> Option<String> {
    let shown: BTreeSet<String> = view
        .narrowing()
        .rules
        .iter()
        .filter(|rule| rule.types.iter().any(|t| t == mapping.source_class()))
        .flat_map(|rule| rule.attributes.iter().cloned())
        .collect();
    let hidden = mapping.hide(&shown);
    (!hidden.is_empty()).then(|| {
        format!(
            "the mapping serves {} from source Attributes no rule of the view shows: they are \
             never served (a source Attribute renamed or removed in the model, or hidden by a \
             Policy)",
            hidden.join(", ")
        )
    })
}

/// Section 3 and 7: the IR compiles, the view selects its source class
/// alone (any other type would be served rebuilt as the target class), and
/// it allows only the reads a mapping inverts.
fn mapped(
    ir: &Value,
    ctx: &Context,
    own: Option<&serde_json::Map<String, Value>>,
    selector: &[String],
    operations: &[OperationId],
) -> Result<crate::mapping::Mapping, String> {
    let mapping = crate::mapping::Mapping::compile(ir, ctx, own)?;
    if selector != [mapping.source_class()] {
        return Err("a mapped view selects its mapping's sourceClass and nothing else".into());
    }
    if let Some(op) = operations
        .iter()
        .find(|op| !crate::mapping::READS.contains(op))
    {
        return Err(format!(
            "a mapped view is read only: {:?} is not one of the reads a mapping inverts",
            op.id()
        ));
    }
    Ok(mapping)
}

fn valid_id(id: &str) -> bool {
    (1..=63).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn expand_all(what: &str, names: &[String], ctx: &Context) -> Result<Vec<String>, String> {
    if names.is_empty() {
        return Err(format!("{what} names at least one"));
    }
    if names.len() > MAX_NAMES {
        return Err(format!("{what} names at most {MAX_NAMES}"));
    }
    names
        .iter()
        .map(|n| {
            let iri = ctx.expand_key(n);
            if antares_jsonld::context::is_absolute_iri(&iri) {
                Ok(iri)
            } else {
                Err(format!("{what}: {n:?} does not expand to an IRI"))
            }
        })
        .collect()
}

fn compile_rule(i: usize, r: &ViewRule, ctx: &Context) -> Result<Rule, String> {
    let at = |what: &str| format!("rules[{i}].{what}");
    let attributes = if r.attributes.is_empty() {
        Vec::new()
    } else {
        expand_all(&at("attributes"), &r.attributes, ctx)?
    };
    Ok(Rule {
        types: expand_all(&at("types"), &r.types, ctx)?,
        q: parse_q(&at("q"), r.q.as_deref())?.map(Box::new),
        scope_q: scope_q(&at("scopeQ"), r.scope_q.as_deref())?,
        geo_q: geo_q(&at("geoQ"), r.geo_q.as_ref())?.map(Box::new),
        attributes,
        // a view rule has one area, names no ids and no windows; an engine's
        // narrow temporal reads
        ..Rule::default()
    })
}

fn parse_q(what: &str, q: Option<&str>) -> Result<Option<antares_ql::QNode>, String> {
    let Some(q) = q else { return Ok(None) };
    if q.len() > MAX_QUERY_BYTES {
        return Err(format!("{what} is at most {MAX_QUERY_BYTES} bytes"));
    }
    antares_ql::parse_q(q)
        .map(Some)
        .map_err(|e| format!("{what}: {e}"))
}

fn scope_q(what: &str, s: Option<&str>) -> Result<Option<String>, String> {
    match s {
        Some(s) if s.is_empty() || s.len() > MAX_QUERY_BYTES => {
            Err(format!("{what} is 1 to {MAX_QUERY_BYTES} bytes"))
        }
        other => Ok(other.map(str::to_owned)),
    }
}

fn geo_q(what: &str, g: Option<&GeoQ>) -> Result<Option<antares_ql::geo::GeoQuery>, String> {
    let Some(g) = g else { return Ok(None) };
    let coordinates = match &g.coordinates {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let mut params = HashMap::from([
        ("georel".to_owned(), g.georel.clone()),
        ("geometry".to_owned(), g.geometry.clone()),
        ("coordinates".to_owned(), coordinates),
    ]);
    if let Some(p) = &g.geoproperty {
        params.insert("geoproperty".to_owned(), p.clone());
    }
    antares_ql::geo::GeoQuery::from_params(&params).map_err(|e| format!("{what}: {e}"))
}

fn operations(ids: &[String]) -> Result<Vec<OperationId>, String> {
    if ids.is_empty() || ids.len() > MAX_NAMES {
        return Err(format!("operations names 1 to {MAX_NAMES} operations"));
    }
    let mut out = Vec::new();
    for id in ids {
        let op = OperationId::from_id(id)
            .ok_or_else(|| format!("operations: {id:?} is not a row of the operation matrix"))?;
        if !out.contains(&op) {
            out.push(op);
        }
    }
    Ok(out)
}

/// ADR-0023 rule 11: the attribute set reveals which rule matched, so the
/// loader names every rule whose condition reads what it does not show.
fn disclosures(rules: &[Rule]) -> Vec<String> {
    let core = antares_jsonld::core_context();
    rules
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let read = r.discloses(&core);
            (!read.is_empty()).then(|| {
                format!(
                    "rules[{i}] reads {} and does not show it: the Attributes it shows \
                     disclose that its condition held",
                    read.join(", ")
                )
            })
        })
        .collect()
}

/// The table entry of one stored document.
fn entry(doc: &Value) -> ViewEntry {
    let loaded = serde_json::from_value::<View>(doc.clone())
        .map_err(|e| format!("not a view: {e}"))
        .and_then(|v| v.load());
    match loaded {
        Ok(l) => ViewEntry::Loaded {
            view: l.view,
            tenant: l.tenant,
            operations: l.operations.iter().map(|op| op.id().to_owned()).collect(),
            notes: l.notes,
            mapping: l
                .mapping
                .map(|m| antares_api::views::Attached(m as Arc<dyn std::any::Any + Send + Sync>)),
            context: l.context,
            representations: l.representations,
        },
        Err(reason) => ViewEntry::Invalid { reason },
    }
}

/// Rebuilds one pod's table from the store. One reload at a time, so a
/// slow reload that read the store before a write cannot swap in its older
/// table after the write's own reload.
struct Reloader {
    state: AppState,
    /// Each stored document, its digest and what was made of it, so a reload
    /// fetches only the documents whose digest changed and an unchanged view
    /// is not validated (and its notes not logged) again (T-4354).
    cache: tokio::sync::Mutex<HashMap<String, Held>>,
    /// The bodies reloads fetched since the pod started.
    fetched: std::sync::atomic::AtomicU64,
    /// The same for the policy engine's rule documents, per Tenant.
    policy: tokio::sync::Mutex<rule_documents::Cache>,
    /// [`SCHEMA_MAX_ENV`]; `None` refuses every schema write.
    schema_max: Option<usize>,
    /// Every Tenant's retention rules, as the last reload found them.
    retention: std::sync::RwLock<Arc<retention::Rules>>,
    /// What this pod's sweeps deleted since it started, by Tenant, type and kind.
    deleted: std::sync::Mutex<BTreeMap<(String, String, &'static str), u64>>,
    /// Which sweep last ran each retention step, for the next one's order.
    turns: std::sync::Mutex<retention::Turns>,
    /// Every Tenant's `maxAge` quality rules, as the last reload found them.
    quality: std::sync::RwLock<Arc<quality::Rules>>,
    /// The platform's history retention for what no Entity, type or Tenant
    /// rules ([`HISTORY_DEFAULT_ENV`], DM-97); `None` keeps it.
    platform_history: Option<std::time::Duration>,
    /// How long audit rows are kept ([`AUDIT_RETENTION_ENV`]).
    audit_retention: std::time::Duration,
}

/// One stored document of the view tenant and what the reloader made of it.
struct Held {
    digest: u64,
    doc: Value,
    made: Made,
}

enum Made {
    /// An erasure record (T-4665): kept, read by `erasure`, made into nothing.
    Erasure,
    Retention(String),
    Quality(String),
    Schema(String, Option<Schema>),
    View(ViewEntry),
}

impl Held {
    fn new(id: &str, digest: u64, doc: Value) -> Self {
        let made = if id.starts_with(erasure::ERASURE_ID_PREFIX) {
            Made::Erasure
        } else if let Some(tenant) = id.strip_prefix(retention::RETENTION_ID_PREFIX) {
            Made::Retention(tenant.to_owned())
        } else if let Some(tenant) = id.strip_prefix(quality::QUALITY_ID_PREFIX) {
            Made::Quality(tenant.to_owned())
        } else if let Some(tenant) = id.strip_prefix(SCHEMA_ID_PREFIX) {
            let schema = doc
                .get("text")
                .and_then(Value::as_str)
                .map(|text| Schema::new(text.as_bytes()));
            if schema.is_none() {
                tracing::warn!(tenant, "a stored schema document has no text; skipped");
            }
            Made::Schema(tenant.to_owned(), schema)
        } else {
            let e = entry(&doc);
            log_entry(id, &e);
            Made::View(e)
        };
        Self { digest, doc, made }
    }
}

impl Reloader {
    async fn reload(&self) -> Result<(), String> {
        let mut cache = self.cache.lock().await;
        let listed = self.state.view_digests().await?;
        let stale: Vec<&str> = listed
            .iter()
            .filter(|(id, d)| cache.get(id).is_none_or(|h| h.digest != *d))
            .map(|(id, _)| id.as_str())
            .collect();
        // Nothing moved and nothing went: every table already holds what
        // the store holds, so none is rebuilt (F-R2ANT3-015).
        if stale.is_empty() && listed.len() == cache.len() {
            drop(cache);
            return rule_documents::reload(&self.state, &mut *self.policy.lock().await).await;
        }
        // Only the bodies whose digest moved, the first reload's being all of them. A body
        // written between the two reads is held under the older digest and fetched again on
        // the next reload; one deleted between them is in neither, and goes.
        let mut fresh: HashMap<String, Value> = HashMap::with_capacity(stale.len());
        for id in stale {
            fresh.extend(
                self.state
                    .view_doc(id)
                    .await?
                    .map(|doc| (id.to_owned(), doc)),
            );
        }
        let fetched = u64::try_from(fresh.len()).unwrap_or(u64::MAX);
        self.fetched
            .fetch_add(fetched, std::sync::atomic::Ordering::Relaxed);
        count_doc_reads("reload", fetched);
        let next: HashMap<String, Held> = listed
            .into_iter()
            .filter_map(|(id, digest)| {
                let held = match fresh.remove(&id) {
                    Some(doc) => Held::new(&id, digest, doc),
                    None => cache.remove(&id)?,
                };
                Some((id, held))
            })
            .collect();
        let mut views = BTreeMap::new();
        let mut schemas = BTreeMap::new();
        let mut retention_docs = Vec::new();
        let mut quality_docs = Vec::new();
        for (id, held) in &next {
            match &held.made {
                Made::Retention(t) => retention_docs.push((t.as_str(), &held.doc)),
                Made::Quality(t) => quality_docs.push((t.as_str(), &held.doc)),
                Made::Schema(t, Some(schema)) => {
                    schemas.insert(t.clone(), schema.clone());
                }
                Made::Erasure | Made::Schema(_, None) => {}
                Made::View(e) => {
                    views.insert(id, (&held.doc, e));
                }
            }
        }
        self.state.views.replace(
            views
                .iter()
                .map(|(id, (_, e))| ((*id).clone(), (*e).clone()))
                .collect(),
            revision(views.iter().map(|(id, (doc, _))| (id.as_str(), *doc))),
        );
        self.state.views.replace_schemas(schemas);
        let retention = Arc::new(retention::load(retention_docs.into_iter()));
        *self
            .state
            .history_off
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            retention::history_off(&retention);
        self.state
            .temporal
            .set_short_history(retention::short_history(&retention));
        *self
            .retention
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = retention;
        *self
            .quality
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Arc::new(quality::load(quality_docs.into_iter()));
        *cache = next;
        drop(cache);
        rule_documents::reload(&self.state, &mut *self.policy.lock().await).await
    }
}

/// Count `n` reads of one stored document by its id on
/// `antares_views_doc_reads_total`, by who read it (T-5501): `reload` (a body
/// whose digest moved), `admin` (a GET on `/q/views`, `/q/schemas`,
/// `/q/retention`, `/q/quality` or `/q/policy`, or a PUT on `/q/policy`,
/// which reads the revision it replaces) or `grant` (an export grant
/// a delivery checks).
pub(crate) fn count_doc_reads(by: &'static str, n: u64) {
    metrics::counter!("antares_views_doc_reads_total", "by" => by).increment(n);
}

fn log_entry(id: &str, e: &ViewEntry) {
    match e {
        ViewEntry::Loaded { notes, .. } => {
            for note in notes {
                tracing::warn!(view = id, note = note.as_str(), "view loaded with a note");
            }
        }
        ViewEntry::Invalid { reason } => {
            tracing::warn!(view = id, reason = reason.as_str(), "view not served");
        }
    }
}

/// The digest of the view documents, in id order (threat V26): a pure
/// function of what the store holds, so pods over one store agree without
/// coordination. FNV-1a 64 is specified, so two builds of the broker agree
/// too; std's `DefaultHasher` promises no stable output across releases.
fn revision<'a>(docs: impl Iterator<Item = (&'a str, &'a Value)>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (id, doc) in docs {
        // A 0 byte ends each part: `("ab", doc)` and `("a", "b" + doc)` differ.
        for b in id
            .bytes()
            .chain([0])
            .chain(doc.to_string().into_bytes())
            .chain([0])
        {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// [`SCHEMA_MAX_ENV`]: `None` unset; a value that is no positive byte count
/// is an error, so a typo never silently refuses every schema.
fn schema_max(raw: Option<String>) -> Result<Option<usize>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    match raw.parse::<usize>() {
        Ok(n) if n > 0 => Ok(Some(n)),
        _ => Err(format!(
            "{SCHEMA_MAX_ENV}={raw:?} is not a positive byte count"
        )),
    }
}

/// [`HISTORY_DEFAULT_ENV`]: `None` unset; a value that is no duration is an
/// error, so a typo never silently keeps or deletes everything.
fn platform_history(raw: Option<String>) -> Result<Option<std::time::Duration>, String> {
    raw.map(|raw| {
        retention::parse_duration(raw.trim()).map_err(|e| format!("{HISTORY_DEFAULT_ENV}: {e}"))
    })
    .transpose()
}

/// [`AUDIT_RETENTION_ENV`]: two years unset; a value that is no duration
/// is an error, so a typo never deletes audit rows early.
fn audit_retention(raw: Option<String>) -> Result<std::time::Duration, String> {
    let Some(raw) = raw else {
        return Ok(AUDIT_RETENTION_DEFAULT);
    };
    let raw = raw.trim();
    let years = raw
        .strip_prefix('P')
        .and_then(|r| r.strip_suffix('Y'))
        .map(|n| match n.parse::<u64>() {
            Ok(n @ 1..=10) => Ok(std::time::Duration::from_hours(n.saturating_mul(365 * 24))),
            _ => Err(format!("{raw:?} is not one to ten years")),
        });
    years
        .unwrap_or_else(|| retention::parse_duration(raw))
        .map_err(|e| format!("{AUDIT_RETENTION_ENV}: {e}"))
}

/// Load the stored views into `state`'s table, serve `/q/views`,
/// `/q/schemas` and `/q/policy` on its admin surface, and reload the table
/// every [`RELOAD_ENV`] milliseconds. Call it once, after the policy engine
/// is attached (the rule documents go to the engine of the `state` given
/// here) and before the router is built, inside a tokio runtime.
///
/// # Errors
/// The store's, or routes set already.
pub async fn install(state: &AppState) -> Result<Reload, String> {
    state
        .views
        .schema_writes
        .set_mode(schema_writes(std::env::var(SCHEMA_WRITES_ENV).ok())?);
    crate::view_cache::configure()?;
    install_with(
        state,
        platform_history(std::env::var(HISTORY_DEFAULT_ENV).ok())?,
    )
    .await
}

/// [`SCHEMA_WRITES_ENV`]: `report` unset, fatal at startup when it names
/// no mode (T-5912).
fn schema_writes(value: Option<String>) -> Result<SchemaWrites, String> {
    match value {
        None => Ok(SchemaWrites::Report),
        Some(v) => SchemaWrites::parse(&v)
            .ok_or_else(|| format!("{SCHEMA_WRITES_ENV} is enforce, report or off")),
    }
}

/// [`install`] with the platform's history retention given, not read from
/// [`HISTORY_DEFAULT_ENV`].
pub(crate) async fn install_with(
    state: &AppState,
    platform_history: Option<std::time::Duration>,
) -> Result<Reload, String> {
    let reloader = Arc::new(Reloader {
        state: state.clone(),
        cache: tokio::sync::Mutex::new(HashMap::new()),
        fetched: std::sync::atomic::AtomicU64::new(0),
        policy: tokio::sync::Mutex::new(rule_documents::Cache::new()),
        schema_max: schema_max(std::env::var(SCHEMA_MAX_ENV).ok())?,
        retention: std::sync::RwLock::default(),
        deleted: std::sync::Mutex::default(),
        turns: std::sync::Mutex::default(),
        quality: std::sync::RwLock::default(),
        platform_history,
        audit_retention: audit_retention(std::env::var(AUDIT_RETENTION_ENV).ok())?,
    });
    reloader.reload().await?;
    // DM-127: what was erased before a restore is gone before this pod serves
    erasure::reapply(&reloader).await;
    // The routes live in the table the reloader fills; a strong handle there
    // would keep the state alive through itself.
    state.views.set_admin(admin(&Arc::downgrade(&reloader)))?;
    let every = std::env::var(RELOAD_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(5000);
    let sweeping = reloader.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(retention::SWEEP_EVERY);
        // The first sweep a minute after start, not at once: an immediate one
        // scheduled late would race the first writes (and the tests' own sweeps).
        tick.reset();
        // A sweep that outlasts its minute waits for the next one instead of
        // the missed ticks firing back to back (T-4084).
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let now = chrono::Utc::now();
            retention::sweep(&sweeping, now).await;
            quality::sweep(&sweeping, now).await;
        }
    });
    let looping = reloader.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(every));
        tick.tick().await;
        loop {
            tick.tick().await;
            looping.reload_or_log().await;
        }
    });
    Ok(Reload(reloader))
}

impl Reloader {
    fn quality_rules(&self) -> Arc<quality::Rules> {
        self.quality
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn retention_rules(&self) -> Arc<retention::Rules> {
        self.retention
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn reload_or_log(&self) {
        if let Err(e) = self.reload().await {
            tracing::warn!(
                error = e.as_str(),
                "view table reload failed; the table is unchanged"
            );
        }
    }
}

/// Rebuilds the table [`install`] loaded, on demand: the interval and every
/// `/q/views` write already do; this is for whoever changed the store
/// another way.
pub struct Reload(Arc<Reloader>);

impl Reload {
    /// One retention sweep at `now` over the rules the last reload found
    /// (the loop [`install`] starts runs it every minute).
    pub async fn sweep_retention(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Vec<retention::Deleted> {
        retention::sweep(&self.0, now).await
    }

    /// [`Self::sweep_retention`] that starts no statement after `budget`.
    pub async fn sweep_retention_within(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        budget: std::time::Duration,
    ) -> Vec<retention::Deleted> {
        retention::sweep_within(&self.0, now, budget).await
    }

    /// One quality sweep at `now` over the rules the last reload found (the
    /// minute loop [`install`] starts runs it after the retention sweep).
    pub async fn sweep_quality(&self, now: chrono::DateTime<chrono::Utc>) -> Vec<quality::Changed> {
        quality::sweep(&self.0, now).await
    }

    /// Rebuild the table from the store now.
    ///
    /// # Errors
    /// The store's; the table is then unchanged.
    pub async fn now(&self) -> Result<(), String> {
        self.0.reload().await
    }
}

/// `/q/views`: the table (GET), one view's document (GET, PUT, DELETE), and
/// what a draft would serve (`POST /q/views/{id}/preview`);
/// `/q/schemas/{tenant}`: one Tenant's `LinkML` schema (GET, PUT, DELETE);
/// and `/q/policy` beside them.
fn admin(r: &Weak<Reloader>) -> Router {
    let (list, one, put, del) = (r.clone(), r.clone(), r.clone(), r.clone());
    let (s_get, s_put, s_del) = (r.clone(), r.clone(), r.clone());
    // Bytes past the bound are refused unread; one byte more reaches
    // `put_schema`, which answers with a problem document.
    let limit = r
        .upgrade()
        .and_then(|r| r.schema_max)
        .map_or(MAX_VIEW_BYTES, |n| n.saturating_add(1));
    Router::new()
        .route(
            "/schemas/{tenant}",
            get(move |Path(t): Path<String>| serve(s_get.clone(), |r| get_schema(r, t)))
                .put(move |Path(t): Path<String>, body: Bytes| {
                    serve(s_put.clone(), |r| put_schema(r, t, body))
                })
                .delete(move |Path(t): Path<String>| serve(s_del.clone(), |r| delete_schema(r, t)))
                .layer(DefaultBodyLimit::max(limit)),
        )
        .route(
            "/views",
            get(move || {
                serve(list.clone(), |r| async move {
                    Ok(axum::Json(r.state.views.report()).into_response())
                })
            }),
        )
        .route(
            "/views/{id}",
            get(move |Path(id): Path<String>| serve(one.clone(), |r| get_one(r, id)))
                .put(move |Path(id): Path<String>, body: Bytes| {
                    serve(put.clone(), |r| put_one(r, id, body))
                })
                .delete(move |Path(id): Path<String>| serve(del.clone(), |r| delete_one(r, id))),
        )
        .merge(preview::routes(r))
        .merge(rule_documents::routes(r))
        .merge(retention::routes(r))
        .merge(erasure::routes(r))
        .merge(quality::routes(r))
}

/// Run `f` with the reloader, or answer 503 once the runtime that ran
/// [`install`] has stopped and taken the reloader with it.
async fn serve<F, Fut>(r: Weak<Reloader>, f: F) -> Response
where
    F: FnOnce(Arc<Reloader>) -> Fut,
    Fut: std::future::Future<Output = Result<Response, String>>,
{
    let Some(r) = r.upgrade() else {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "InternalError",
            "the view table is not running",
        );
    };
    f(r).await.unwrap_or_else(|e| inner(&e))
}

async fn get_one(r: Arc<Reloader>, id: String) -> Result<Response, String> {
    // A schema shares the store with the views; only a view id reads here.
    if !valid_id(&id) {
        return Ok(not_found());
    }
    count_doc_reads("admin", 1);
    Ok(match r.state.view_doc(&id).await? {
        Some(doc) => axum::Json(doc).into_response(),
        None => not_found(),
    })
}

async fn delete_one(r: Arc<Reloader>, id: String) -> Result<Response, String> {
    if !valid_id(&id) || !r.state.delete_view_doc(&id).await? {
        return Ok(not_found());
    }
    r.reload().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// The document of view `id` and the view it loads, or the first reason it
/// cannot be served.
fn candidate(id: &str, body: &[u8]) -> Result<(Value, Loaded), String> {
    let doc = serde_json::from_slice::<Value>(body).map_err(|e| format!("not a view: {e}"))?;
    let view: View = serde_json::from_value(doc.clone()).map_err(|e| format!("not a view: {e}"))?;
    if view.id != id {
        return Err("the document's id names another view than the path".into());
    }
    view.load().map(|l| (doc, l))
}

/// Validate, store, and answer once this pod's table holds the write.
async fn put_one(r: Arc<Reloader>, id: String, body: Bytes) -> Result<Response, String> {
    if body.len() > MAX_VIEW_BYTES {
        return Ok(problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            "BadRequestData",
            &format!("a view document is at most {MAX_VIEW_BYTES} bytes"),
        ));
    }
    let doc = match candidate(&id, &body) {
        Ok((doc, _)) => doc,
        Err(why) => return Ok(problem(StatusCode::BAD_REQUEST, "BadRequestData", &why)),
    };
    store(&r, &id, doc).await
}

/// The key of `tenant`'s schema, `None` when the path names no Tenant a
/// client can.
fn schema_key(tenant: &str) -> Option<String> {
    tenant_key(SCHEMA_ID_PREFIX, tenant)
}

fn bad_tenant() -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "BadRequestData",
        "the path names no tenant a client can",
    )
}

async fn get_schema(r: Arc<Reloader>, tenant: String) -> Result<Response, String> {
    stored_json(&r, schema_key(&tenant), no_schema).await
}

/// The JSON document stored under `key` as it was written, or `missing()`; a path that names no
/// tenant (`key` is `None`) is `bad_tenant`. Schemas and retention documents are read this way.
async fn stored_json(
    r: &Reloader,
    key: Option<String>,
    missing: impl FnOnce() -> Response,
) -> Result<Response, String> {
    let Some(key) = key else {
        return Ok(bad_tenant());
    };
    count_doc_reads("admin", 1);
    Ok(
        match r
            .state
            .view_doc(&key)
            .await?
            .as_ref()
            .and_then(|d| d.get("text"))
            .and_then(Value::as_str)
        {
            Some(text) => (
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                text.to_owned(),
            )
                .into_response(),
            None => missing(),
        },
    )
}

async fn delete_schema(r: Arc<Reloader>, tenant: String) -> Result<Response, String> {
    remove_stored(&r, schema_key(&tenant), no_schema).await
}

/// Store `doc` under `key` and answer once this pod's table holds it: 201 for a new document,
/// 204 for one it replaced.
async fn store(r: &Reloader, key: &str, doc: Value) -> Result<Response, String> {
    let replaced = r.state.put_view_doc(key, doc).await?;
    r.reload().await?;
    Ok(if replaced {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    }
    .into_response())
}

/// Delete the document stored under `key` and reload, or `missing()`; a path that names no tenant
/// (`key` is `None`) is `bad_tenant`. Schemas, quality and retention documents are deleted this way.
async fn remove_stored(
    r: &Reloader,
    key: Option<String>,
    missing: impl FnOnce() -> Response,
) -> Result<Response, String> {
    let Some(key) = key else {
        return Ok(bad_tenant());
    };
    if !r.state.delete_view_doc(&key).await? {
        return Ok(missing());
    }
    r.reload().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// The key `prefix{tenant}` of a Tenant's stored document, `None` when the path names no Tenant a
/// client can.
fn tenant_key(prefix: &str, tenant: &str) -> Option<String> {
    antares_model::TenantId::new(tenant)
        .ok()
        .map(|t| format!("{prefix}{}", t.as_str()))
}

/// A per-tenant document kept as the text it was written in (quality, retention): its key prefix,
/// the noun its answers use, its size cap and the parser a PUT must pass.
struct TextDoc {
    prefix: &'static str,
    what: &'static str,
    max: usize,
    check: fn(&[u8]) -> Result<(), String>,
}

impl TextDoc {
    fn key(&self, tenant: &str) -> Option<String> {
        tenant_key(self.prefix, tenant)
    }

    fn missing(&self) -> Response {
        problem(
            StatusCode::NOT_FOUND,
            "ResourceNotFound",
            &format!("no {} document for this tenant", self.what),
        )
    }

    /// The answer a PUT of `body` is refused with, if it is.
    fn refused(&self, body: &[u8]) -> Option<Response> {
        if body.len() > self.max {
            return Some(problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                "BadRequestData",
                &format!("a {} document is at most {} bytes", self.what, self.max),
            ));
        }
        (self.check)(body)
            .err()
            .map(|why| problem(StatusCode::BAD_REQUEST, "BadRequestData", &why))
    }

    async fn put(&self, r: &Reloader, tenant: &str, body: &[u8]) -> Result<Response, String> {
        let Some(key) = self.key(tenant) else {
            return Ok(bad_tenant());
        };
        if let Some(refusal) = self.refused(body) {
            return Ok(refusal);
        }
        let text = String::from_utf8_lossy(body).into_owned();
        store(r, &key, serde_json::json!({"id": key, "text": text})).await
    }

    /// GET the document as written, PUT one, DELETE it.
    fn handlers(&'static self, weak: &Weak<Reloader>) -> axum::routing::MethodRouter {
        let (read, write, remove) = (weak.clone(), weak.clone(), weak.clone());
        get(move |Path(t): Path<String>| {
            serve(read.clone(), move |r| async move {
                stored_json(&r, self.key(&t), || self.missing()).await
            })
        })
        .put(move |Path(t): Path<String>, body: Bytes| {
            serve(write.clone(), move |r| async move {
                self.put(&r, &t, &body).await
            })
        })
        .delete(move |Path(t): Path<String>| {
            serve(remove.clone(), move |r| async move {
                remove_stored(&r, self.key(&t), || self.missing()).await
            })
        })
    }
}

/// The rules of the stored `what` documents among `docs`, by Tenant: a stored document that no
/// longer parses is skipped with a warning, so the sweep acts on nothing for that Tenant rather
/// than guess.
fn tenant_rules<'a, R>(
    docs: impl Iterator<Item = (&'a str, &'a Value)>,
    parse: fn(&[u8]) -> Result<R, String>,
    what: &str,
) -> Vec<(antares_model::TenantId, R)> {
    let mut rules = Vec::new();
    for (tenant, doc) in docs {
        let body = doc.get("text").and_then(Value::as_str).unwrap_or_default();
        match antares_model::TenantId::new(tenant)
            .map_err(|e| e.to_string())
            .and_then(|t| Ok((t, parse(body.as_bytes())?)))
        {
            Ok(rule) => rules.push(rule),
            Err(e) => tracing::warn!(tenant, error = %e, "{what} document skipped"),
        }
    }
    rules
}

/// Bound, check, store, and answer once this pod's table holds the write.
async fn put_schema(r: Arc<Reloader>, tenant: String, body: Bytes) -> Result<Response, String> {
    let Some(max) = r.schema_max else {
        return Ok(problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "OperationNotSupported",
            &format!("{SCHEMA_MAX_ENV} is not set: this broker takes no schema"),
        ));
    };
    if body.len() > max {
        return Ok(problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            "BadRequestData",
            &format!("a schema document is at most {max} bytes"),
        ));
    }
    let Some(key) = schema_key(&tenant) else {
        return Ok(bad_tenant());
    };
    let text = match std::str::from_utf8(&body)
        .map_err(|e| e.to_string())
        .and_then(|t| {
            serde_json::from_str::<serde::de::IgnoredAny>(t)
                .map(|_| t)
                .map_err(|e| e.to_string())
        }) {
        Ok(t) => t,
        Err(e) => {
            return Ok(problem(
                StatusCode::BAD_REQUEST,
                "BadRequestData",
                &format!("not JSON: {e}"),
            ))
        }
    };
    if let Some(slot) = member_slot(text) {
        return Ok(problem(
            StatusCode::BAD_REQUEST,
            "BadRequestData",
            &format!(
                "slot {slot:?} is a member of an NGSI-LD Attribute, never an Attribute \
                 name: no Entity can carry it; give the slot another name, one that says what it holds (pm10, not value)"
            ),
        ));
    }
    store(&r, &key, serde_json::json!({"id": key, "text": text})).await
}

/// Members of an Attribute instance an Entity also carries, or that the
/// platform's core model (`ngsi-ld-core.linkml.yaml`, imported by every
/// compiled data model) declares as slots of every Entity: a surface reads
/// them as the Entity's own, so they stay allowed (T-3956).
const ENTITY_SLOTS: [&str; 6] = [
    "type",
    "observedAt",
    "createdAt",
    "modifiedAt",
    "deletedAt",
    "expiresAt",
];

/// The first slot of a `LinkML` schema named after a member of an Attribute
/// instance (`value`, `object`, `unitCode`, ...): no Entity can carry it as
/// an Attribute, the expansion reads the key as that member. The schema's own
/// slots, each class's slots and slot usages (T-3956).
fn member_slot(text: &str) -> Option<String> {
    let doc: serde_json::Value = serde_json::from_str(text).ok()?;
    let keys = |v: Option<&serde_json::Value>| {
        v.and_then(serde_json::Value::as_object)
            .into_iter()
            .flat_map(|o| o.keys().cloned())
            .collect::<Vec<_>>()
    };
    let mut names = keys(doc.get("slots"));
    for class in doc
        .get("classes")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flat_map(serde_json::Map::values)
    {
        names.extend(keys(class.get("slot_usage")));
        names.extend(
            class
                .get("slots")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned),
        );
    }
    names.into_iter().find(|n| {
        antares_jsonld::RESERVED_MEMBERS.contains(&n.as_str())
            && !ENTITY_SLOTS.contains(&n.as_str())
    })
}

fn no_schema() -> Response {
    problem(
        StatusCode::NOT_FOUND,
        "ResourceNotFound",
        "no schema for this tenant",
    )
}

fn not_found() -> Response {
    problem(StatusCode::NOT_FOUND, "ResourceNotFound", "no such view")
}

fn inner(e: &str) -> Response {
    tracing::warn!(error = e, "the view store failed");
    problem(
        StatusCode::INTERNAL_SERVER_ERROR,
        "InternalError",
        "the view store failed",
    )
}

fn problem(status: StatusCode, title: &str, detail: &str) -> Response {
    let body = serde_json::json!({
        "type": format!("https://uri.etsi.org/ngsi-ld/errors/{title}"),
        "title": title,
        "status": status.as_u16(),
        "detail": detail,
    });
    (status, axum::Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// T-5912: unset reports, a mode name sets it, anything else is fatal.
    #[test]
    fn the_schema_writes_key_names_a_mode_or_fails_startup() {
        assert_eq!(schema_writes(None), Ok(SchemaWrites::Report));
        assert_eq!(
            schema_writes(Some("enforce".into())),
            Ok(SchemaWrites::Enforce)
        );
        assert_eq!(schema_writes(Some("off".into())), Ok(SchemaWrites::Off));
        let refused = schema_writes(Some("on".into())).unwrap_err();
        assert!(refused.contains(SCHEMA_WRITES_ENV), "{refused}");
    }

    /// F-ANT4-029: the revision is FNV-1a 64 over the documents' bytes, the
    /// same number in every build and on every pod.
    #[test]
    fn the_revision_is_a_specified_hash_of_the_documents() {
        let doc = json!({"a": 1});
        assert_eq!(revision([("v1", &doc)].into_iter()), 0x8343_8375_8cd1_bbf2);
        let (ab, b) = (json!("b"), json!("ab"));
        assert_ne!(
            revision([("a", &ab)].into_iter()),
            revision([("", &b)].into_iter())
        );
    }

    fn base() -> Value {
        json!({
            "id": "v", "tenant": "city", "selector": ["Car"],
            "rules": [{"types": ["Car"], "attributes": ["speed"]}],
            "operations": ["query_entities"]
        })
    }

    fn load(v: &Value) -> Result<Loaded, String> {
        View::parse(v.to_string().as_bytes()).and_then(|v| v.load())
    }

    fn with(member: &str, value: Value) -> Value {
        let mut v = base();
        v[member] = value;
        v
    }

    fn mapped() -> Value {
        let mut v = with(
            "mapping",
            json!({"version": 2, "sourceClass": "Car", "targetClass": "Vehicle",
                   "slots": [{"target": "velocity", "source": "speed", "kind": "rename"}]}),
        );
        v["operations"] = json!([
            "retrieve_entity",
            "query_entities",
            "retrieve_attribute_value"
        ]);
        v
    }

    #[test]
    fn a_mapped_view_serves_the_temporal_and_discovery_reads() {
        let mut v = mapped();
        v["operations"] = json!(crate::mapping::READS
            .iter()
            .map(|op| op.id())
            .collect::<Vec<_>>());
        let loaded = load(&v).map(|l| l.operations.len());
        assert_eq!(loaded, Ok(11));
    }

    #[test]
    fn a_view_keeps_the_representations_it_names_and_refuses_another_name() {
        let ical = json!({"ical": [{"name": "events", "sources": [{"preset": "sdm-event"}]}], "feed": [{"name": "news"}]});
        let loaded = load(&with("representations", ical.clone())).expect("loads");
        assert_eq!(
            loaded
                .representations
                .as_deref()
                .cloned()
                .map(Value::Object),
            Some(ical)
        );
        assert!(load(&base()).is_ok_and(|l| l.representations.is_none()));
        let many: Vec<Value> = (0..=MAX_OUTPUTS)
            .map(|i| json!({"name": format!("o{i}")}))
            .collect();
        assert!(load(&with("representations", json!({"feed": many.clone()}))).is_ok());
        for bad in [
            json!({"vcard": []}),
            json!({"ical": {"preset": "sdm-event"}}),
            json!({"feed": {}}),
            json!([]),
        ] {
            assert!(
                load(&with("representations", bad.clone())).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_mapped_view_loads_with_its_mapping() {
        let loaded = load(&mapped());
        assert_eq!(
            loaded
                .ok()
                .and_then(|l| l.mapping.map(|m| m.source_class().to_owned())),
            Some("https://uri.etsi.org/ngsi-ld/default-context/Car".to_owned())
        );
        assert!(load(&base()).is_ok_and(|l| l.mapping.is_none()));
    }

    #[test]
    fn a_mapped_view_is_read_only_selects_its_source_class_alone_and_runs_a_valid_ir() {
        for op in [
            "create_entity",
            "create_subscription",
            "batch_upsert",
            "query_entities_post",
            "temporal_query_post",
            "temporal_upsert",
            "create_temporal_entity_map",
        ] {
            let mut v = mapped();
            v["operations"] = json!(["query_entities", op]);
            let e = load(&v).err().unwrap_or_default();
            assert!(e.contains("read only"), "{op}: {e}");
        }
        for selector in [json!(["Truck"]), json!(["Car", "Truck"])] {
            let mut v = mapped();
            v["selector"] = selector.clone();
            v["rules"] = json!([{"types": ["Car", "Truck"], "attributes": ["speed"]}]);
            let e = load(&v).err().unwrap_or_default();
            assert!(
                e.contains("sourceClass and nothing else"),
                "{selector}: {e}"
            );
        }
        let mut v = mapped();
        v["mapping"]["version"] = json!(1);
        assert!(load(&v).err().unwrap_or_default().contains("version 1"));
    }

    #[test]
    fn a_slot_whose_source_the_rules_do_not_show_is_noted_and_not_served() {
        assert!(load(&mapped()).is_ok_and(|l| l.notes.is_empty()));
        // the model renamed `speed`: the rules show the new name only
        let mut v = mapped();
        v["rules"] = json!([{"types": ["Car"], "attributes": ["velocityKmh"]}]);
        let loaded = load(&v).expect("a hidden slot does not refuse the view");
        assert_eq!(loaded.notes.len(), 1, "{:?}", loaded.notes);
        assert!(
            loaded
                .notes
                .iter()
                .any(|n| n.contains("serves velocity from")),
            "{:?}",
            loaded.notes
        );
        let sort = [("orderBy".to_owned(), "velocity".to_owned())];
        assert_eq!(
            loaded.mapping.as_deref().map(|m| m.invert(&sort).err()),
            Some(Some(crate::mapping::NOT_FILTERABLE)),
            "no sort reads a hidden slot"
        );
    }

    #[test]
    fn a_view_of_everything_takes_no_mapping() {
        let mut v = mapped();
        v["selector"] = json!([EVERY]);
        v["rules"] = json!([{"types": [EVERY], "attributes": [EVERY]}]);
        let e = load(&v).err().unwrap_or_default();
        assert!(e.contains("sourceClass and nothing else"), "{e}");
    }

    #[test]
    fn an_id_is_one_to_63_of_lowercase_digits_and_dashes() {
        for ok in ["a", "a-1", &"z".repeat(63)] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in ["", &"z".repeat(64), "A", "a_b", "a.b", "a/b", "é"] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn every_bound_holds_at_load() {
        let many = |n: usize| Value::Array(vec![json!("Car"); n]);
        assert!(load(&with("selector", many(MAX_NAMES))).is_ok());
        let e = load(&with("selector", many(MAX_NAMES + 1)))
            .err()
            .unwrap_or_default();
        assert!(e.contains("at most 256"), "{e}");
        let ops = Value::Array(vec![json!("query_entities"); MAX_NAMES + 1]);
        assert!(load(&with("operations", ops)).is_err());
        let long = "/a".repeat(MAX_QUERY_BYTES);
        assert!(load(&with("scopeQ", json!(long))).is_err());
        assert!(load(&with("scopeQ", json!("/a/#"))).is_ok());
        let big = vec![b' '; MAX_VIEW_BYTES + 1];
        assert!(View::parse(&big)
            .err()
            .unwrap_or_default()
            .contains("at most"));
    }

    #[test]
    fn an_operation_named_twice_is_allowed_once() {
        let loaded = load(&with(
            "operations",
            json!(["query_entities", "query_entities"]),
        ));
        assert_eq!(
            loaded.map(|l| l.operations),
            Ok(vec![OperationId::QueryEntities])
        );
    }

    #[test]
    fn a_geoq_takes_its_coordinates_as_json_or_text_and_names_its_geoproperty() {
        for coordinates in [
            json!([[[0, 0], [1, 0], [1, 1], [0, 0]]]),
            json!("[[[0,0],[1,0],[1,1],[0,0]]]"),
        ] {
            let g = json!({"georel": "within", "geometry": "Polygon",
                           "coordinates": coordinates, "geoproperty": "position"});
            assert!(load(&with("geoQ", g)).is_ok());
        }
    }

    #[test]
    fn a_scoped_context_in_the_view_is_refused() {
        let ctx = json!({"Car": {"@id": "https://x.example/Car", "@context": {}}});
        let e = load(&with("@context", ctx)).err().unwrap_or_default();
        assert!(e.starts_with("the view's @context"), "{e}");
    }

    #[test]
    fn the_schema_bound_is_a_positive_byte_count_or_unset() {
        assert_eq!(schema_max(None), Ok(None));
        assert_eq!(schema_max(Some("64".into())), Ok(Some(64)));
        for bad in ["0", "-1", "64k", ""] {
            let e = schema_max(Some(bad.into())).err().unwrap_or_default();
            assert!(e.starts_with(SCHEMA_MAX_ENV), "{bad}: {e}");
        }
    }

    /// DM-97: the platform default is a duration or unset; anything else stops the start.
    #[test]
    fn the_platform_history_is_a_duration_or_unset() {
        assert_eq!(platform_history(None), Ok(None));
        assert_eq!(
            platform_history(Some(" P28D ".into())),
            Ok(Some(std::time::Duration::from_hours(28 * 24)))
        );
        for bad in ["28", "forever", "PT30S"] {
            let err = platform_history(Some(bad.into())).expect_err(bad);
            assert!(err.starts_with(HISTORY_DEFAULT_ENV), "{err}");
        }
    }

    /// T-6337: audit rows keep two years unset, a duration or whole years
    /// when set, and anything else stops the start.
    #[test]
    fn the_audit_retention_is_two_years_unless_set() {
        let day = std::time::Duration::from_hours(24);
        assert_eq!(audit_retention(None), Ok(day * 730));
        assert_eq!(audit_retention(Some(" P3Y ".into())), Ok(day * 1095));
        assert_eq!(audit_retention(Some("P400D".into())), Ok(day * 400));
        for bad in ["2y", "P0Y", "P11Y", "PY", "forever", "PT30S"] {
            let err = audit_retention(Some(bad.into())).expect_err(bad);
            assert!(err.starts_with(AUDIT_RETENTION_ENV), "{err}");
        }
    }

    #[tokio::test]
    async fn without_a_bound_no_schema_is_taken_or_stored() {
        let r = Arc::new(Reloader {
            state: AppState::new("antares-test".into()),
            cache: tokio::sync::Mutex::new(HashMap::new()),
            fetched: std::sync::atomic::AtomicU64::new(0),
            policy: tokio::sync::Mutex::new(rule_documents::Cache::new()),
            schema_max: None,
            retention: std::sync::RwLock::default(),
            deleted: std::sync::Mutex::default(),
            turns: std::sync::Mutex::default(),
            quality: std::sync::RwLock::default(),
            platform_history: None,
            audit_retention: AUDIT_RETENTION_DEFAULT,
        });
        let put = put_schema(r.clone(), "city".into(), Bytes::from_static(b"{}")).await;
        assert_eq!(put.map(|p| p.status()), Ok(StatusCode::SERVICE_UNAVAILABLE));
        assert_eq!(r.state.view_doc("schema:city").await, Ok(None));
    }

    /// T-4354: the 5 s reload lists digests and fetches a body only when
    /// its digest moved, and keeps the schema it already read.
    #[tokio::test]
    async fn an_unchanged_reload_fetches_no_bodies() {
        let r = Reloader {
            state: AppState::new("antares-test".into()),
            cache: tokio::sync::Mutex::new(HashMap::new()),
            fetched: std::sync::atomic::AtomicU64::new(0),
            policy: tokio::sync::Mutex::new(rule_documents::Cache::new()),
            schema_max: None,
            retention: std::sync::RwLock::default(),
            deleted: std::sync::Mutex::default(),
            turns: std::sync::Mutex::default(),
            quality: std::sync::RwLock::default(),
            platform_history: None,
            audit_retention: AUDIT_RETENTION_DEFAULT,
        };
        let fetched = || r.fetched.load(std::sync::atomic::Ordering::Relaxed);
        r.state.put_view_doc("v", base()).await.expect("view");
        let schema = json!({"id": "schema:city", "text": "{\"id\": \"https://e.org/s\"}"});
        r.state
            .put_view_doc("schema:city", schema)
            .await
            .expect("schema");
        r.reload().await.expect("first");
        assert_eq!(fetched(), 2);
        let held = r.state.views.schema("city").expect("schema").bytes;
        let table = r.state.views.snapshot();
        r.reload().await.expect("unchanged");
        assert_eq!(fetched(), 2, "an unchanged reload fetches nothing");
        // F-R2ANT3-015: and rebuilds no table
        assert!(
            Arc::ptr_eq(&table, &r.state.views.snapshot()),
            "the view table is kept"
        );
        let kept = r.state.views.schema("city").expect("schema").bytes;
        assert!(Arc::ptr_eq(&held, &kept), "the schema read once is kept");
        let mut changed = base();
        changed["selector"] = json!(["Bus"]);
        r.state.put_view_doc("v", changed).await.expect("view");
        r.reload().await.expect("changed");
        assert_eq!(fetched(), 3, "only the changed body");
        r.state.delete_view_doc("v").await.expect("gone");
        r.reload().await.expect("deleted");
        assert_eq!(fetched(), 3);
        assert!(r.cache.lock().await.get("v").is_none());
    }

    #[test]
    fn a_stored_document_that_is_no_view_is_invalid_with_the_reason() {
        let mut doc = base();
        doc["allowAll"] = json!(true);
        let shown = format!("{:?}", entry(&doc));
        assert!(
            shown.starts_with("Invalid { reason: \"not a view: unknown field `allowAll`"),
            "{shown}"
        );
    }
}
