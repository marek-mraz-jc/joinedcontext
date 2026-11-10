// SPDX-License-Identifier: EUPL-1.2
//! The composition hook of endpoint views (ADR-0023, ADR-0027 section 5,
//! T-3494): a request served through a view runs the ordinary handler with
//! one more conjunct, the view's narrowing, composed with whatever the
//! installed engine answers (`effective = view ∧ engine`, both narrowing,
//! neither widening).
//!
//! The view travels on the [`AppState`](crate::AppState)
//! ([`AppState::with_view`](crate::AppState::with_view)), not on the task:
//! everything a request spawns, every in-process call and every join walk
//! clones that state, so no path can lose the view and serve wider than it.
//! `gate!` reads it after the engine has answered and before [`resolve`], so
//! a clause that cannot be narrowed (a write, a purge) is refused under a
//! view exactly as it is under a narrowing engine.
//!
//! Loading, validating and addressing views is `antares-ops`' (T-3496); this
//! module composes one, and holds the table of the views a pod serves
//! ([`ViewTable`]) with the store seam the loader writes through, so the
//! operation layer never names the store (threat F1).

use crate::policy::{resolve, Decision, Denied, Filter, Rule, Warning};
use antares_ql::geo::GeoQuery;
use antares_ql::QNode;
use std::collections::BTreeMap;
use std::sync::Arc;

/// How many rules a composed narrowing may hold: the pairs of a view's
/// rules and an engine's that share a type are bounded, so neither side can
/// make the other's allocation grow without limit. Pairs that share no type
/// compose to nothing and are never counted (T-4079).
pub const MAX_COMPOSED_RULES: usize = 4096;

/// The refusal when [`MAX_COMPOSED_RULES`] is reached. A bound on the
/// request, not a policy: answered 400 BadRequestData naming the bound,
/// never 403, which reads as "no policy grants this" (T-4079).
pub const TOO_MANY_RULES: &str = "the view and the policy compose to more than 4096 rules \
     (MAX_COMPOSED_RULES); name fewer types in the request";

/// A view as `gate!` applies it: the predicate `selector ∧ q ∧ scope_q ∧
/// geo_q` and the per-rule allowlists (ADR-0023 rule 11), compiled once.
/// Built only through [`CompiledView::new`], which refuses a view that
/// would serve everything.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledView {
    narrowing: Arc<Filter>,
    /// The view's id, recorded on what is made through it (a
    /// Subscription's `__view`, rule 9).
    id: String,
}

impl CompiledView {
    /// The view's narrowing. `selector` is its Entity types; every rule is
    /// kept to the types it shares with the selector, and a rule that shares
    /// none is dropped. A view with no selector, or with no rule left, is
    /// refused: a view that names nothing would otherwise read as a
    /// narrowing that narrows nothing.
    ///
    /// An Entity with several types is matched by a rule when one of its
    /// types is both in the selector and in the rule, which can show less
    /// than the two conditions read apart, never more.
    pub fn new(
        selector: &[String],
        q: Option<QNode>,
        scope_q: Option<String>,
        geo_q: Option<antares_ql::geo::GeoQuery>,
        rules: Vec<Rule>,
    ) -> Result<Self, String> {
        if selector.is_empty() {
            return Err("a view selects at least one Entity type".into());
        }
        let in_selector = |t: &String| {
            let iri = crate::repr::policy_context().expand_key(t);
            selector
                .iter()
                .any(|s| crate::repr::policy_context().expand_key(s) == iri)
        };
        let rules: Vec<Rule> = rules
            .into_iter()
            .map(|r| Rule {
                types: r.types.iter().filter(|t| in_selector(t)).cloned().collect(),
                ..r
            })
            .filter(|r| !r.types.is_empty())
            .collect();
        if rules.is_empty() {
            return Err(
                "a view shows its Entities through at least one rule of its selector's types"
                    .into(),
            );
        }
        Ok(Self {
            narrowing: Arc::new(Filter {
                q: q.map(Box::new),
                scope_q,
                geo_q: geo_q.map(Box::new),
                rules,
                ..Filter::default()
            }),
            id: String::new(),
        })
    }

    /// The view that selects every type and shows every Attribute
    /// (ADR-0023 rule 12): no rules, so the engine's answer stands, narrowed
    /// by the view's own conditions alone.
    #[must_use]
    pub fn every(
        q: Option<QNode>,
        scope_q: Option<String>,
        geo_q: Option<antares_ql::geo::GeoQuery>,
    ) -> Self {
        Self {
            narrowing: Arc::new(Filter {
                q: q.map(Box::new),
                scope_q,
                geo_q: geo_q.map(Box::new),
                ..Filter::default()
            }),
            id: String::new(),
        }
    }

    /// The view that shows nothing: one rule naming no type, which holds
    /// for no Entity. What a state that lost its request's view reads as.
    #[must_use]
    pub fn nothing() -> Arc<Self> {
        static NOTHING: std::sync::LazyLock<Arc<CompiledView>> = std::sync::LazyLock::new(|| {
            Arc::new(CompiledView {
                narrowing: Arc::new(Filter {
                    rules: vec![crate::policy::Rule::default()],
                    ..Filter::default()
                }),
                id: String::new(),
            })
        });
        NOTHING.clone()
    }

    /// Whether the view shows every name ([`Self::every`]): [`Self::new`]
    /// never compiles a view without rules.
    #[must_use]
    pub fn shows_every_name(&self) -> bool {
        self.narrowing.rules.is_empty()
    }

    /// The same view under the id the table serves it as.
    #[must_use]
    pub fn named(mut self, id: &str) -> Self {
        id.clone_into(&mut self.id);
        self
    }

    /// The id the table serves the view as; empty for a view never named.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The narrowing this view adds to every operation served through it.
    pub fn narrowing(&self) -> &Filter {
        &self.narrowing
    }

    /// The Entity types and the Attributes the view shows, each compacted
    /// under the core @context, sorted, once each: a name the core context
    /// does not define stays the IRI, which expands to itself.
    pub fn shown_names(&self) -> (Vec<String>, Vec<String>) {
        let ctx = crate::repr::policy_context();
        let compact = |names: &mut dyn Iterator<Item = &String>| {
            names
                .map(|n| ctx.compact_iri(n))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        };
        let rules = &self.narrowing.rules;
        (
            compact(&mut rules.iter().flat_map(|r| &r.types)),
            compact(&mut rules.iter().flat_map(|r| &r.attributes)),
        )
    }
}

/// List @contexts (13.3, CIM 009 5.13.3).
const LIST_CONTEXTS: &str = "5.13.3";

/// What `gate!` hands the handler: the engine's answer for `clause`
/// composed with the view the state carries, if it carries one. Without a
/// view the answer is the engine's, unchanged.
pub fn with_view(
    view: Option<&CompiledView>,
    clause: &str,
    engine: Result<Arc<Filter>, Denied>,
) -> Result<Arc<Filter>, Denied> {
    let Some(view) = view else {
        return engine;
    };
    if clause == LIST_CONTEXTS {
        // Rule 5: the handler lists the core @context alone under a view,
        // so the view has nothing of its own to narrow there.
        return engine;
    }
    if crate::policy::NAME_READS.contains(&clause) {
        // Serve @context (5.13.4) is offered through no view: refused here,
        // as it was before NAME_READS took a narrowing (T-4009).
        return Err(Denied(format!(
            "{clause} serves no stored @context through a view"
        )));
    }
    if crate::policy::WRITE_BY_ID.contains(&clause) && CHECKED_WRITES.contains(&clause) {
        // Rule 7 as below, the engine's answer the condition its write
        // holds the stored Entity to (`gate`), checked beside the view's;
        // a temporal one has no write guard and is refused as below.
        return engine;
    }
    if CHECKED_WRITES.contains(&clause) || SUBSCRIPTIONS.contains(&clause) {
        // Rule 7: the handler checks the view on the stored Entity before
        // and after the write (`WriteGuard`); the engine's own answer
        // still refuses a narrowed write, as it does without a view.
        return match resolve(clause, Decision::Filter(engine?)) {
            Decision::Deny(why) => Err(Denied(why)),
            // 5.8.4 lists the view's own and keeps the engine's selection
            // among them (T-3673)
            Decision::Filter(f) if crate::policy::RECORD_LISTS.contains(&clause) => Ok(f),
            _ => Ok(crate::policy::unnarrowed()),
        };
    }
    if crate::policy::RECORD_LISTS.contains(&clause) && !view.narrowing().is_empty() {
        // The view narrows Entities; read over a Registration or a
        // Context Source Subscription it would select records by what an
        // Entity carries. Refused, as before those lists took a narrowing.
        return Err(Denied(format!(
            "{clause} lists records an Entity view cannot narrow"
        )));
    }
    // An engine that narrows nothing (allow-all) leaves the view's own
    // narrowing: `compose(v, Filter::default())` is `v`, so the pairing is
    // skipped on the path every request of an allow-all broker takes
    // (T-3697, ADR-0027 section 6).
    // A query keeps a filter as it is (`resolve`), so there the view's
    // compiled narrowing itself is the answer, shared and not copied.
    let engine = engine?;
    let narrows_nothing = *engine == Filter::default();
    if narrows_nothing && crate::policy::FILTERABLE.contains(&clause) {
        return Ok(view.narrowing.clone());
    }
    paired(view, clause, engine)
}

/// The view's narrowing paired with an engine's that narrows, for `clause`.
/// An engine answers a caller with the same narrowing request after
/// request, and composing a large view costs tens of microseconds (benches
/// `views`, T-3778): the last answer is kept per thread and handed out again
/// while the same view, clause and engine narrowing come back. It is keyed
/// by the whole input (the view's `Arc` itself, held so its address cannot
/// be reused; the clause; the engine's `Filter`, the same `Arc` or an equal
/// value), so it never answers for another (O16). An engine that hands out
/// the same `Arc` again (T-6618) is found by pointer, without comparing the
/// rules. A filter unequal to itself (a NaN) misses.
/// The answer is the same `Arc` on a hit, so `repr::narrow_repr` finds its
/// reach by pointer as well.
fn paired(view: &CompiledView, clause: &str, engine: Arc<Filter>) -> Result<Arc<Filter>, Denied> {
    type Last = (
        Arc<Filter>,
        String,
        Arc<Filter>,
        Result<Arc<Filter>, Denied>,
    );
    thread_local! {
        static LAST: std::cell::RefCell<Option<Last>> = const { std::cell::RefCell::new(None) };
    }
    let hit = LAST.with_borrow(|last| {
        last.as_ref()
            .filter(|l| {
                Arc::ptr_eq(&l.0, &view.narrowing)
                    && l.1 == clause
                    && (Arc::ptr_eq(&l.2, &engine) || *l.2 == *engine)
            })
            .map(|l| l.3.clone())
    });
    if let Some(hit) = hit {
        return hit;
    }
    let key = engine.clone();
    let composed = if *engine == Filter::default() {
        Ok(Filter::clone(view.narrowing()))
    } else {
        compose(view.narrowing(), Arc::unwrap_or_clone(engine))
    };
    let answer =
        composed.and_then(
            |composed| match resolve(clause, Decision::Filter(Arc::new(composed))) {
                Decision::Allow => Ok(crate::policy::unnarrowed()),
                // `resolve` makes no condition of a filter
                Decision::Filter(f) => Ok(f),
                Decision::AllowIf(f) => Ok(Arc::new(f)),
                Decision::Deny(why) => Err(Denied(why)),
            },
        );
    LAST.set(Some((
        view.narrowing.clone(),
        clause.to_owned(),
        key,
        answer.clone(),
    )));
    answer
}

/// `a ∧ b` as one [`Filter`]: every member narrows and none widens. The
/// queries are conjoined on the AST, the `scopeQ`s intersected, the
/// projections intersected (`pick`) and joined (`omit`), and the rules
/// paired: an Entity a pair holds for matches a rule of each side, and shows
/// what both show.
pub fn compose(a: &Filter, b: Filter) -> Result<Filter, Denied> {
    let (geo_q, geo_also) = and_geo(a.areas(), b.areas());
    let rules = pair_rules(&a.rules, b.rules)?;
    let mut omit = b.omit;
    for name in &a.omit {
        if !omit.contains(name) {
            omit.push(name.clone());
        }
    }
    let mut omit_by_type = b.omit_by_type;
    for (t, attrs) in &a.omit_by_type {
        let into = omit_by_type.entry(t.clone()).or_default();
        for name in attrs {
            if !into.contains(name) {
                into.push(name.clone());
            }
        }
    }
    Ok(Filter {
        q: and_q(a.q.as_deref(), b.q.map(|q| *q)).map(Box::new),
        scope_q: and_scope(a.scope_q.as_deref(), b.scope_q)?,
        geo_q,
        geo_also,
        pick: and_pick(&a.pick, b.pick),
        omit,
        rules,
        omit_by_type,
        restricted: a.restricted || b.restricted,
        warning: Warning::join(a.warning.as_ref(), b.warning),
        temporal: [&a.temporal[..], &b.temporal[..]].concat().into(),
        metadata: crate::policy::Metadata::both(a.metadata.as_deref(), b.metadata),
    })
}

fn and_q(a: Option<&QNode>, b: Option<QNode>) -> Option<QNode> {
    match (a, b) {
        (Some(a), Some(b)) => Some(QNode::And(vec![a.clone(), b])),
        (Some(a), None) => Some(a.clone()),
        (None, b) => b,
    }
}

fn and_scope(a: Option<&str>, b: Option<String>) -> Result<Option<String>, Denied> {
    match (a, b) {
        (Some(a), Some(b)) => antares_ql::scope::intersect_scope_q(a, &b)
            .map(Some)
            .ok_or_else(|| Denied(crate::policy::SCOPE_NOT_NARROWABLE.to_owned())),
        (Some(a), None) => Ok(Some(a.to_owned())),
        (None, b) => Ok(b),
    }
}

/// Every area of both sides, the first in `geo_q` and the rest in
/// `geo_also`: each must hold, so none is dropped (dropping one widens).
fn and_geo<'g>(
    a: impl Iterator<Item = &'g GeoQuery>,
    b: impl Iterator<Item = &'g GeoQuery>,
) -> (Option<Box<GeoQuery>>, Vec<GeoQuery>) {
    let mut all = a.chain(b).cloned();
    (all.next().map(Box::new), all.collect())
}

/// Names compared as expanded IRIs, kept in `b`'s written form.
fn shared(a: &[String], b: Vec<String>) -> Vec<String> {
    let ctx = crate::repr::policy_context();
    let a: Vec<String> = a.iter().map(|n| ctx.expand_key(n)).collect();
    b.into_iter()
        .filter(|n| a.contains(&ctx.expand_key(n)))
        .collect()
}

/// An empty `pick` keeps everything, so it is the other side's; two lists
/// keep what both name. Two lists that share nothing keep the frame alone,
/// which `pick` cannot say with an empty list: the one member it keeps is
/// one no Entity carries.
fn and_pick(a: &[String], b: Vec<String>) -> Vec<String> {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b,
        (false, true) => a.to_vec(),
        (false, false) => {
            let both = shared(a, b);
            if both.is_empty() {
                vec![NOTHING.to_owned()]
            } else {
                both
            }
        }
    }
}

/// A rule's `ids` and `id_patterns`.
type IdNarrowing = (Vec<String>, Vec<String>);

/// The id narrowing of two rules that must both hold: one side's own when
/// the other narrows no id; else the explicit ids of each side the other
/// side selects, `None` when none is left (the pair holds for nothing).
/// Two pattern lists have no conjunction a single list can say, so they
/// refuse the composition rather than widen it.
fn and_ids(x: &Rule, y: &Rule) -> Result<Option<IdNarrowing>, Denied> {
    let narrows = |r: &Rule| !r.ids.is_empty() || !r.id_patterns.is_empty();
    if !narrows(x) {
        return Ok(Some((y.ids.clone(), y.id_patterns.clone())));
    }
    if !narrows(y) {
        return Ok(Some((x.ids.clone(), x.id_patterns.clone())));
    }
    if !x.id_patterns.is_empty() && !y.id_patterns.is_empty() {
        return Err(Denied(
            "the view and the policy both narrow ids by pattern".into(),
        ));
    }
    let mut ids: Vec<String> = x
        .ids
        .iter()
        .filter(|i| y.selects_id(i))
        .chain(y.ids.iter().filter(|i| x.selects_id(i)))
        .cloned()
        .collect();
    ids.sort();
    ids.dedup();
    Ok((!ids.is_empty()).then_some((ids, Vec::new())))
}

/// A member name no Entity carries: an expanded IRI under a scheme of its
/// own, so no `@context` can map a term onto it.
const NOTHING: &str = "urn:antares:views:nothing";

fn pair_rules(a: &[Rule], b: Vec<Rule>) -> Result<Vec<Rule>, Denied> {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => return Ok(b),
        (false, true) => return Ok(a.to_vec()),
        (false, false) => {}
    }
    // Only a pair that shares a type composes to anything, so `b` is
    // indexed by type and each rule of `a` meets only the rules it shares
    // one with: an engine's rule per requested type no longer multiplies by
    // every rule of the view (T-4079). The bound counts those pairs.
    let ctx = crate::repr::policy_context();
    let mut by_type: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, y) in b.iter().enumerate() {
        for t in &y.types {
            by_type.entry(ctx.expand_key(t)).or_default().push(index);
        }
    }
    let mut paired = 0usize;
    let mut out = Vec::new();
    for x in a {
        let mut meets: Vec<usize> = x
            .types
            .iter()
            .filter_map(|t| by_type.get(&ctx.expand_key(t)))
            .flatten()
            .copied()
            .collect();
        meets.sort_unstable();
        meets.dedup();
        paired = paired.saturating_add(meets.len());
        if paired > MAX_COMPOSED_RULES {
            return Err(Denied(TOO_MANY_RULES.to_owned()));
        }
        for y in meets.into_iter().filter_map(|index| b.get(index)) {
            let types = shared(&x.types, y.types.clone());
            let Some((ids, id_patterns)) = and_ids(x, y)? else {
                continue;
            };
            let (geo_q, geo_also) = and_geo(x.areas(), y.areas());
            out.push(Rule {
                types,
                q: and_q(x.q.as_deref(), y.q.as_deref().cloned()).map(Box::new),
                scope_q: and_scope(x.scope_q.as_deref(), y.scope_q.clone())?,
                geo_q,
                geo_also,
                ids,
                id_patterns,
                // a side that shows every Attribute leaves the other's list
                attributes: match (x.every_attribute, y.every_attribute) {
                    (true, _) => y.attributes.clone(),
                    (false, true) => x.attributes.clone(),
                    (false, false) => shared(&x.attributes, y.attributes.clone()),
                },
                every_attribute: x.every_attribute && y.every_attribute,
                temporal: [x.temporal.as_slice(), y.temporal.as_slice()].concat(),
            });
        }
    }
    // No pair shares a type: nothing is shown. An empty list would read as
    // "no rules", which shows everything; a rule naming no type holds for
    // nothing.
    if out.is_empty() {
        out.push(Rule::default());
    }
    Ok(out)
}

/// The reserved tenant the view documents live under (ADR-0012, ADR-0023
/// rule 10). No client may name it, and no NGSI-LD route reads the
/// document kind they are stored as.
pub const VIEW_TENANT: &str = "antares-views";

/// The reserved tenant the policy engine's rule documents live under
/// (ADR-0020 amendment), one document per Tenant, keyed by its name.
pub const POLICY_TENANT: &str = "antares-policy";

/// The reserved tenant the export grants of internal context sources live
/// under (ADR-0025 rule 5), one document per (target, source) pair.
pub const GRANT_TENANT: &str = "antares-grants";

/// What a surface crate attached to a loaded view, opaque to the broker.
#[derive(Clone)]
pub struct Attached(pub Arc<dyn std::any::Any + Send + Sync>);

impl std::fmt::Debug for Attached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Attached(..)")
    }
}

/// One view of the table.
#[derive(Debug, Clone)]
pub enum ViewEntry {
    /// A view that loaded: its narrowing, and what `/q/health` says of it.
    Loaded {
        /// The narrowing every operation through the view is composed with.
        view: Arc<CompiledView>,
        /// The one Tenant it serves.
        tenant: String,
        /// The operations it allows, as `operation-matrix.yaml` ids.
        operations: Vec<String>,
        /// What the loader found worth an operator's eye: a rule whose
        /// condition reads an Attribute it does not show (ADR-0023 rule
        /// 11), a temporal view keyed on a mutable member (rule 8).
        notes: Vec<String>,
        /// The compiled Mapping of a view of another model (gap G18), which
        /// only the view root reads; the broker carries it and never looks
        /// inside.
        mapping: Option<Attached>,
        /// The view's own `@context` as written, which a surface reads the
        /// NGSI-LD names a caller writes under (ADR-0028 rule 6).
        context: Option<Arc<serde_json::Map<String, serde_json::Value>>>,
        /// The view's domain-standard representations as written
        /// (`representations`, keyed by surface name), which that surface
        /// alone reads (T-4901).
        representations: Option<Arc<serde_json::Map<String, serde_json::Value>>>,
    },
    /// A stored view that failed validation: never served, named in
    /// `/q/health` with the reason.
    Invalid {
        /// Why it did not load.
        reason: String,
    },
}

/// The views this pod serves, swapped whole on every reload so a request
/// sees one table or the next, never half of each (ADR-0023 rule 10). Empty
/// until a view loads: no view means nothing served (rule M4).
#[derive(Debug, Default)]
pub struct ViewTable {
    entries: std::sync::RwLock<Arc<BTreeMap<String, ViewEntry>>>,
    /// The digest of the view documents the entries were loaded from
    /// (threat V26): pods over one store report the same value.
    revision: std::sync::atomic::AtomicU64,
    admin: std::sync::OnceLock<axum::Router>,
    /// Admin routes of other broker modules, merged beside the views'.
    more_admin: std::sync::Mutex<Vec<axum::Router>>,
    policy: std::sync::RwLock<serde_json::Value>,
    schemas: std::sync::RwLock<Arc<BTreeMap<String, Schema>>>,
    /// What every write is held to against its Tenant's schema (T-5912).
    pub schema_writes: crate::schema_writes::Hook,
}

/// The id prefix a Tenant's schema document is stored under, beside the
/// view documents in [`VIEW_TENANT`]. A view id is `[a-z0-9-]`, so no view
/// shares a key with a schema.
pub const SCHEMA_ID_PREFIX: &str = "schema:";

/// A Tenant's `LinkML` schema as written to `/q/schemas/{tenant}` (T-3628):
/// JSON the broker checked for size and well-formedness only. A surface
/// parses it and caches the parse per `revision`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    /// Changes whenever the bytes do: a digest of them, opaque.
    pub revision: u64,
    /// The document as written.
    pub bytes: Arc<[u8]>,
}

impl Schema {
    /// The schema of `bytes`, its revision their digest.
    #[must_use]
    pub fn new(bytes: &[u8]) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut h);
        Self {
            revision: h.finish(),
            bytes: bytes.into(),
        }
    }
}

impl ViewTable {
    /// The table as it is now.
    pub fn snapshot(&self) -> Arc<BTreeMap<String, ViewEntry>> {
        self.entries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Replace the whole table, loaded from the documents whose digest is
    /// `revision`.
    pub fn replace(&self, entries: BTreeMap<String, ViewEntry>, revision: u64) {
        *self
            .entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(entries);
        self.revision
            .store(revision, std::sync::atomic::Ordering::Release);
    }

    /// The revision `/q/health` reports as `viewsRevision`: sixteen hex
    /// digits, the same on every pod that loaded the same documents.
    pub fn revision(&self) -> String {
        format!(
            "{:016x}",
            self.revision.load(std::sync::atomic::Ordering::Acquire)
        )
    }

    /// Replace every Tenant's schema.
    pub fn replace_schemas(&self, schemas: BTreeMap<String, Schema>) {
        *self
            .schemas
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(schemas);
    }

    /// The schema of `tenant`, if one is stored.
    pub fn schema(&self, tenant: &str) -> Option<Schema> {
        self.schemas
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(tenant)
            .cloned()
    }

    /// What `/q/health` lists of the schemas: each Tenant's size and
    /// revision, never the document.
    pub fn schema_report(&self) -> serde_json::Value {
        let schemas = self
            .schemas
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        serde_json::Value::Object(
            schemas
                .iter()
                .map(|(tenant, s)| {
                    let v = serde_json::json!({
                        "bytes": s.bytes.len(),
                        "revision": format!("{:016x}", s.revision),
                    });
                    (tenant.clone(), v)
                })
                .collect(),
        )
    }

    /// Whether at least one view loaded: a surface that serves through views
    /// is closed until one has (rule M4).
    pub fn any_loaded(&self) -> bool {
        self.snapshot()
            .values()
            .any(|e| matches!(e, ViewEntry::Loaded { .. }))
    }

    /// What `/q/health` lists: each view, `loaded` with its tenant,
    /// operations and notes, or `invalid` with the reason.
    pub fn report(&self) -> serde_json::Value {
        let views = self.snapshot();
        serde_json::Value::Object(
            views
                .iter()
                .map(|(id, e)| {
                    let v = match e {
                        ViewEntry::Loaded {
                            tenant,
                            operations,
                            notes,
                            ..
                        } => serde_json::json!({
                            "state": "loaded",
                            "tenant": tenant,
                            "operations": operations.len(),
                            "notes": notes,
                        }),
                        ViewEntry::Invalid { reason } => {
                            serde_json::json!({"state": "invalid", "reason": reason})
                        }
                    };
                    (id.clone(), v)
                })
                .collect(),
        )
    }

    /// What `/q/health` lists of the policy rule documents (ADR-0020
    /// amendment): set by the loader on every reload, `null` until it runs
    /// with an engine that reads documents. Tenant and revision only.
    pub fn policy_report(&self) -> serde_json::Value {
        self.policy
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Replace what [`policy_report`](Self::policy_report) answers.
    pub fn set_policy_report(&self, report: serde_json::Value) {
        *self
            .policy
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = report;
    }

    /// The `/q/views` routes the loader serves, merged into the admin
    /// surface and so relative to `/q` (`/views`, `/views/{id}`). Set once,
    /// before the router is built.
    ///
    /// # Errors
    /// When routes were set already.
    pub fn set_admin(&self, routes: axum::Router) -> Result<(), String> {
        self.admin
            .set(routes)
            .map_err(|_| "the /q/views routes are set already".to_owned())
    }

    /// Add `routes` to the admin surface beside `/q/views` (relative to
    /// `/q`, behind the same credential). Before the router is built: it
    /// reads them once, when it mounts.
    pub fn add_admin(&self, routes: axum::Router) {
        self.more_admin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(routes);
    }

    /// The `/q/views` routes, if a loader set them, with every module's
    /// [`Self::add_admin`] routes.
    pub fn admin(&self) -> Option<axum::Router> {
        let more = self
            .more_admin
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let first = self.admin.get().cloned();
        if first.is_none() && more.is_empty() {
            return None;
        }
        Some(
            more.into_iter()
                .fold(first.unwrap_or_default(), axum::Router::merge),
        )
    }
}

/// The writes whose handlers check ADR-0023 rule 7 on the stored Entity
/// (the crate's write guard): Create Entity, Update and Append Attributes, Partial
/// Attribute Update, Delete Attribute, Delete Entity, Merge, Replace Entity,
/// Replace Attribute and Set Attribute Value, and the batch create, upsert,
/// update, merge and delete, member by member. Every other write is refused
/// under a view, as a narrowed write is ([`resolve`]).
pub const CHECKED_WRITES: [&str; 14] = [
    "5.6.1", "5.6.2", "5.6.3", "5.6.4", "5.6.5", "5.6.6", "5.6.17", "5.6.18", "5.6.19", "5.6.7",
    "5.6.8", "5.6.9", "5.6.20", "5.6.10",
];

/// The Subscription operations a view serves (rule 9, 5.8.1 to 5.8.5): run
/// as the root runs them, on the view's own Subscriptions alone, each
/// stored with the view's id in [`VIEW_MEMBER`].
pub const SUBSCRIPTIONS: [&str; 5] = ["5.8.1", "5.8.2", "5.8.3", "5.8.4", "5.8.5"];

/// The member a Subscription made through a view records the view's id in,
/// beside the subject (rule 9). Under the `__` prefix the broker reserves
/// (ADR-0020): never set by a client, never served.
pub const VIEW_MEMBER: &str = "__view";

/// Whether the stored Subscription `sub` is the view's own: made through
/// `view` when there is one, at the root when there is none.
pub(crate) fn owns(view: Option<&CompiledView>, sub: &serde_json::Value) -> bool {
    sub.get(VIEW_MEMBER).and_then(serde_json::Value::as_str) == view.map(CompiledView::id)
}

/// The view a stored Subscription was made through, as the table serves it
/// to `tenant` now: `None` for one made at the root, `Some(None)` when its
/// view is no longer loaded for that Tenant (a removed view stops delivery).
pub(crate) fn of_subscription(
    st: &crate::state::AppState,
    tenant: &antares_model::TenantId,
    sub: &serde_json::Value,
) -> Option<Option<Arc<CompiledView>>> {
    of_subscription_in(&st.views.snapshot(), tenant, sub).map(|v| v.cloned())
}

/// [`of_subscription`] in a table the caller already holds: the matcher
/// takes one snapshot per change instead of one per candidate (T-6371).
pub(crate) fn of_subscription_in<'a>(
    table: &'a BTreeMap<String, ViewEntry>,
    tenant: &antares_model::TenantId,
    sub: &serde_json::Value,
) -> Option<Option<&'a Arc<CompiledView>>> {
    let id = sub.get(VIEW_MEMBER)?.as_str().unwrap_or_default();
    Some(match table.get(id) {
        Some(ViewEntry::Loaded {
            view, tenant: t, ..
        }) if t == tenant.as_str() => Some(view),
        _ => None,
    })
}

/// `watchedAttributes` (expanded) as a Subscription through `view` keeps
/// them: the ones some rule of the view shows (rule 9); `Err` when none is.
pub(crate) fn watched_in(
    view: &CompiledView,
    watched: &[serde_json::Value],
) -> Result<Vec<serde_json::Value>, String> {
    let shown: Vec<String> = crate::repr::type_rules(view.narrowing())
        .map(|f| f.rules.iter().flat_map(|r| r.attributes.clone()).collect())
        .unwrap_or_default();
    let kept: Vec<serde_json::Value> = watched
        .iter()
        .filter(|w| {
            view.shows_every_name() || w.as_str().is_some_and(|w| shown.iter().any(|s| s == w))
        })
        .cloned()
        .collect();
    if kept.is_empty() {
        Err("watchedAttributes names no Attribute this view shows".into())
    } else {
        Ok(kept)
    }
}

/// Why a write through a view whose post-image the view would not select is
/// refused (rule 7). The caller saw the pre-image, so it learns nothing new.
pub const MOVES_OUT: &str = "the write would move the Entity out of this view";
/// Why a write naming an Attribute the view does not show is refused whole
/// (rule 7, never a silent drop).
pub const NOT_WRITABLE: &str = "the write names an Attribute this view does not let it write";
/// Why a Relationship write through a view is refused when its target is
/// not shown: hidden and absent answer alike (threat-model D7).
pub const TARGET_NOT_SHOWN: &str =
    "a Relationship written through this view must point at an Entity it shows";

/// ADR-0023 rule 7 for one write through a view: the stored Entity must be
/// in the view before the write (else it is not found, rule 3) and after it,
/// and every Attribute the write names must be one a rule holding for the
/// Entity shows, before and after. The checks run inside the store's own
/// read-modify-write, on the document it is about to commit, so no
/// concurrent write slips between check and commit. The names are judged on
/// the pre-image too, so a hidden Attribute answers the same 403 whether the
/// Entity holds it or not.
///
/// A refusal inside the store closure has only the closure's error type to
/// travel in, so the guard records it and aborts the commit, and the handler
/// turns the record into the 403 with [`WriteGuard::verdict`] right after
/// the store call.
#[derive(Debug)]
pub(crate) struct WriteGuard {
    /// The view's selection, as an engine's write condition is held.
    sel: crate::policy::Condition,
    /// The expanded names of the Attributes the write sets or removes.
    written: Vec<String>,
    refused: std::sync::OnceLock<&'static str>,
    /// The request's `If-Match` (T-3789), every field line joined.
    if_match: Option<String>,
    /// The stored Entity was in the view and `if_match` held for it.
    held: std::sync::atomic::AtomicBool,
}

impl WriteGuard {
    /// The guard of the view `st` serves through for a write naming
    /// `written` (expanded Attribute names), or `None` without a view.
    /// `scope` is the Entity's own member and no Attribute (4.18): the
    /// allowlist does not govern it, the post-image check does.
    pub(crate) fn of<'a>(
        st: &crate::state::AppState,
        written: impl IntoIterator<Item = &'a str>,
    ) -> Option<Self> {
        let view = st.view()?;
        let narrowing = view.narrowing();
        Some(Self {
            sel: crate::policy::Condition::new(narrowing),
            written: written.into_iter().map(str::to_owned).collect(),
            refused: std::sync::OnceLock::new(),
            if_match: None,
            held: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// The guard of a write to one Entity, which honours the request's
    /// `If-Match` (RFC 9110 13.1.1, T-3789). A batch names no one Entity
    /// and takes none.
    pub(crate) fn matching(mut self, headers: &axum::http::HeaderMap) -> Self {
        let lines: Vec<&str> = headers
            .get_all(axum::http::header::IF_MATCH)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect();
        self.if_match = (!lines.is_empty()).then(|| lines.join(","));
        self
    }

    /// The strong entity tag of what the view shows of the stored document
    /// `doc`: its frame and the Attributes a rule holding for it shows, so a
    /// write to a hidden Attribute leaves the tag as it was.
    // ponytail: std's SipHash over the serialized members, 64 bits; a
    // broker upgrade may change every tag, which a client meets as a 412
    pub(crate) fn tag(&self, doc: &serde_json::Value) -> String {
        use std::hash::{Hash, Hasher};
        let shown = match self.sel.rules.as_deref() {
            Some(rules) => rules.shown(doc, crate::repr::policy_context()),
            None => Some(crate::policy::Shown::default()),
        };
        let mut hasher = std::hash::DefaultHasher::new();
        for (k, v) in doc.as_object().into_iter().flatten() {
            let frame = matches!(k.as_str(), "id" | "type" | "scope");
            let attribute = !antares_model::is_meta(k)
                && !k.starts_with("__")
                && !k.starts_with('@')
                && shown.as_ref().is_some_and(|s| s.allows(k));
            if frame || attribute {
                k.hash(&mut hasher);
                hash_json(v, &mut hasher);
            }
        }
        format!("\"{:016x}\"", hasher.finish())
    }

    /// The precondition on the stored document: in the view, and the
    /// request's `If-Match` holds for it. Either failing aborts the write
    /// as not found; [`Self::settled`] answers a failed `If-Match` 412.
    fn condition(&self, id: &str, doc: &serde_json::Value) -> Result<(), antares_model::NgsiError> {
        if self.keeps(doc) {
            Ok(())
        } else {
            Err(antares_model::NgsiError::ResourceNotFound(format!(
                "entity {id} not found"
            )))
        }
    }

    /// Whether a write may touch the stored document `doc`: the view selects
    /// it and the request's `If-Match` holds for it.
    pub(crate) fn keeps(&self, doc: &serde_json::Value) -> bool {
        let holds = self.sees(doc)
            && self
                .if_match
                .as_deref()
                .is_none_or(|m| if_match_holds(m, &self.tag(doc)));
        if holds {
            self.held.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        holds
    }

    /// Whether the view selects the stored document `doc`. A linked path in
    /// the view's `q` never holds here, as in a rule (ADR-0023 rule 11): the
    /// check runs inside the store's write and reads no other Entity.
    pub(crate) fn sees(&self, doc: &serde_json::Value) -> bool {
        self.sel.holds(doc)
    }

    /// Why the view refuses `doc` with the written names on it, if it does.
    fn judge(&self, doc: &serde_json::Value) -> Option<&'static str> {
        self.judge_names(doc, self.written.iter().map(String::as_str).collect())
    }

    fn judge_names(&self, doc: &serde_json::Value, written: Vec<&str>) -> Option<&'static str> {
        if !self.sees(doc) {
            Some(MOVES_OUT)
        } else if !self
            .sel
            .rules
            .as_deref()
            .is_none_or(|r| r.admits_write(doc, &written, crate::repr::policy_context()))
        {
            Some(NOT_WRITABLE)
        } else {
            None
        }
    }

    fn refuse(&self, why: &'static str) -> antares_model::NgsiError {
        let _ = self.refused.set(why);
        antares_model::NgsiError::InternalError(why.into())
    }

    /// The pre-image check on the stored document: an Entity out of the
    /// view is not found, the answer an absent one gets; a name the view
    /// does not let the write set is refused (recorded).
    pub(crate) fn pre(
        &self,
        id: &str,
        doc: &serde_json::Value,
    ) -> Result<(), antares_model::NgsiError> {
        self.condition(id, doc)?;
        self.judge(doc).map_or(Ok(()), |why| Err(self.refuse(why)))
    }

    /// [`Self::pre`] for a write that replaces the whole Entity: every
    /// Attribute the stored Entity holds counts as written, since a replace
    /// deletes the ones it omits (threat-model D6).
    pub(crate) fn pre_whole(
        &self,
        id: &str,
        doc: &serde_json::Value,
    ) -> Result<(), antares_model::NgsiError> {
        let held = doc
            .as_object()
            .into_iter()
            .flat_map(|o| o.keys())
            .map(String::as_str)
            .filter(|k| !antares_model::is_meta(k) && !k.starts_with("__"));
        let names = self
            .written
            .iter()
            .map(String::as_str)
            .chain(held)
            .collect();
        self.condition(id, doc)?;
        self.judge_names(doc, names)
            .map_or(Ok(()), |why| Err(self.refuse(why)))
    }

    /// The post-image check on `doc` as the write leaves it. A refusal is
    /// recorded for [`Self::verdict`] and aborts the commit.
    pub(crate) fn post(&self, doc: &serde_json::Value) -> Result<(), antares_model::NgsiError> {
        match self.judge(doc) {
            None => Ok(()),
            Some(why) => Err(self.refuse(why)),
        }
    }

    /// A document no store closure holds yet, as a create writes it.
    pub(crate) fn admits(&self, doc: &serde_json::Value) -> Result<(), Denied> {
        match self.judge(doc) {
            None => Ok(()),
            Some(why) => Err(Denied(why.to_owned())),
        }
    }

    /// [`Self::verdict`] after the `If-Match` of the request: one that did
    /// not hold, on an Entity out of the view or absent alike, is the 412
    /// the handler answers (RFC 9110 13.1.1).
    pub(crate) fn settled(&self) -> crate::negotiate::ApiResult<()> {
        if self.if_match.is_some() && !self.held.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::negotiate::ApiError::Bare(
                axum::http::StatusCode::PRECONDITION_FAILED,
            ));
        }
        Ok(self.verdict()?)
    }

    /// The refusal [`Self::pre`] or [`Self::post`] recorded, as the 403 the
    /// handler answers.
    pub(crate) fn verdict(&self) -> Result<(), Denied> {
        match self.refused.get() {
            Some(why) => Err(Denied((*why).to_owned())),
            None => Ok(()),
        }
    }

    /// Every Relationship target an expanded fragment writes must be an
    /// Entity this view shows; an absent target answers as a hidden one.
    /// A view that selects every Entity hides none, so an absent target
    /// tells nothing and is written as at the root (T-3786). The targets are
    /// read together, in one transaction on Postgres (T-4085).
    pub(crate) async fn targets(
        &self,
        st: &crate::state::AppState,
        tenant: &antares_model::TenantId,
        fragment: &serde_json::Value,
    ) -> Result<(), Denied> {
        if self.sel.selects_every() {
            return Ok(());
        }
        let targets = relationship_targets(fragment);
        if targets.is_empty() {
            return Ok(());
        }
        // A store failure reads as absent, as a failed single read did.
        let found = st
            .store
            .get_entities(tenant, &targets)
            .await
            .unwrap_or_default();
        self.targets_in(fragment, &found)
    }

    /// [`Self::targets`] against Entities already read: `found` holds every
    /// target that exists, keyed by id.
    pub(crate) fn targets_in(
        &self,
        fragment: &serde_json::Value,
        found: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<(), Denied> {
        if self.sel.selects_every()
            || relationship_targets(fragment)
                .iter()
                .all(|t| found.get(*t).is_some_and(|d| self.sees(d)))
        {
            Ok(())
        } else {
            Err(Denied(TARGET_NOT_SHOWN.to_owned()))
        }
    }
}

/// RFC 9110 13.1.1: `*` holds for any current representation; otherwise one
/// listed entity tag must be strongly equal to `tag`, which is quoted, so an
/// unquoted or weak (`W/`) one never is.
fn if_match_holds(field: &str, tag: &str) -> bool {
    field
        .split(',')
        .map(str::trim)
        .any(|t| t == "*" || t == tag)
}

/// Rule 7 for an Entity a create writes, outside any store closure: in the
/// view, with Attributes it shows and Relationships to Entities it shows.
pub(crate) async fn admits_new(
    st: &crate::state::AppState,
    tenant: &antares_model::TenantId,
    doc: &serde_json::Value,
) -> Result<(), Denied> {
    let Some(g) = WriteGuard::of(
        st,
        crate::attrs::attr_iris_of(doc).iter().map(String::as_str),
    ) else {
        return Ok(());
    };
    g.admits(doc)?;
    g.targets(st, tenant, doc).await
}

/// The ids every Relationship and ListRelationship instance of an expanded
/// Entity or fragment points at.
pub(crate) fn relationship_targets(doc: &serde_json::Value) -> Vec<&str> {
    let instances = doc
        .as_object()
        .into_iter()
        .flat_map(|o| o.values())
        .filter_map(serde_json::Value::as_array)
        .flatten();
    let mut out = Vec::new();
    for inst in instances {
        for member in ["object", "objectList"] {
            match inst.get(member) {
                Some(serde_json::Value::String(id)) => out.push(id.as_str()),
                Some(serde_json::Value::Array(a)) => out.extend(a.iter().filter_map(|e| {
                    e.as_str()
                        .or_else(|| e.get("object").and_then(serde_json::Value::as_str))
                })),
                _ => {}
            }
        }
    }
    out
}

/// [`schema_admits`] for a document whose `type` names the Entity's types (a
/// create, a replace, a batch member): no store read.
pub(crate) fn schema_admits_typed(
    st: &crate::state::AppState,
    tenant: &antares_model::TenantId,
    doc: &serde_json::Value,
    whole: bool,
) -> Result<(), antares_model::NgsiError> {
    let hook = &st.views.schema_writes;
    let Some(check) = hook.check.get() else {
        return Ok(());
    };
    let mode = hook.mode();
    if mode == crate::schema_writes::Mode::Off {
        return Ok(());
    }
    let Some(schema) = st.views.schema(tenant.as_str()) else {
        return Ok(());
    };
    match check(schema.revision, &schema.bytes, doc, whole) {
        Ok(()) => Ok(()),
        Err(why) if mode == crate::schema_writes::Mode::Enforce => {
            Err(antares_model::NgsiError::BadRequestData(why))
        }
        Err(why) => {
            tracing::warn!(tenant = %tenant, refusal = %why, "a write breaks the Tenant's schema (reported, not refused)");
            Ok(())
        }
    }
}

/// Rule T-5912 for a write to the Entity `id`: `doc` (expanded) keeps to
/// the Tenant's schema. A fragment that names no `type` is judged by
/// `types` (the stored types the policy gate read) or, when the gate read
/// none, by the stored Entity's, read only when a check and a schema are
/// there to use them.
pub(crate) async fn schema_admits(
    st: &crate::state::AppState,
    tenant: &antares_model::TenantId,
    id: &str,
    doc: &serde_json::Value,
    types: &[String],
) -> Result<(), antares_model::NgsiError> {
    if doc.get("type").is_some() {
        return schema_admits_typed(st, tenant, doc, false);
    }
    let hook = &st.views.schema_writes;
    if hook.check.get().is_none()
        || hook.mode() == crate::schema_writes::Mode::Off
        || st.views.schema(tenant.as_str()).is_none()
    {
        return Ok(());
    }
    let types: Vec<serde_json::Value> = if types.is_empty() {
        let stored = st
            .store
            .get(tenant, antares_store::Kind::Entity, id)
            .await?;
        stored
            .as_ref()
            .and_then(|d| d.get("type"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
    } else {
        types
            .iter()
            .cloned()
            .map(serde_json::Value::String)
            .collect()
    };
    if types.is_empty() {
        return Ok(());
    }
    let mut typed = doc.clone();
    if let Some(o) = typed.as_object_mut() {
        o.insert("type".to_owned(), serde_json::Value::Array(types));
    }
    schema_admits_typed(st, tenant, &typed, false)
}

/// `v.to_string().hash(h)` without the String: the text streams into the
/// hasher, then the `0xff` that `str`'s Hash writes after it, so every
/// entity tag keeps its value (R1-016).
fn hash_json(v: &serde_json::Value, h: &mut impl std::hash::Hasher) {
    struct Into<'a, H>(&'a mut H);
    impl<H: std::hash::Hasher> std::io::Write for Into<'_, H> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.write(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    // writing a Value into a sink that never fails cannot fail
    let _ = serde_json::to_writer(Into(&mut *h), v);
    h.write_u8(0xff);
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{
        DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject, FILTERABLE,
    };
    use crate::AppState;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tower::ServiceExt;

    /// R1-016: the streamed hash is the one the String gave, so a tag a
    /// client holds from before still matches.
    #[test]
    fn the_streamed_hash_is_the_one_the_text_gave() {
        use std::hash::{Hash, Hasher};
        for v in [
            serde_json::json!({"type": "Property", "value": [1, "a\"b", {"x": null}]}),
            serde_json::json!("ü"),
            serde_json::json!(1.5),
        ] {
            let mut old = std::hash::DefaultHasher::new();
            v.to_string().hash(&mut old);
            let mut new = std::hash::DefaultHasher::new();
            hash_json(&v, &mut new);
            assert_eq!(old.finish(), new.finish(), "{v}");
        }
    }

    struct Narrowing(Filter);

    impl PolicyEngine for Narrowing {
        fn name(&self) -> &str {
            "narrowing"
        }
        fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
            let answer = if FILTERABLE.contains(&op.clause) {
                Decision::Filter(std::sync::Arc::new(self.0.clone()))
            } else {
                Decision::Allow
            };
            Box::pin(std::future::ready(answer))
        }
        fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
            NotifyDecision::Deliver
        }
    }

    fn rule(types: &[&str], q: Option<&str>, attributes: &[&str]) -> Rule {
        Rule {
            types: types.iter().map(|t| (*t).to_owned()).collect(),
            q: q.map(|q| Box::new(antares_ql::parse_q(q).expect("q"))),
            attributes: attributes.iter().map(|a| (*a).to_owned()).collect(),
            ..Rule::default()
        }
    }

    #[test]
    fn the_context_list_is_the_one_read_a_view_does_not_narrow() {
        let view = cars();
        assert_eq!(
            with_view(Some(&view), LIST_CONTEXTS, Ok(Arc::new(Filter::default()))),
            Ok(Arc::new(Filter::default()))
        );
        assert!(
            with_view(Some(&view), "5.13.4", Ok(Arc::new(Filter::default()))).is_err(),
            "serving a stored @context is refused under a view"
        );
    }

    /// T-3778: the pairing kept per thread is handed out again for the same
    /// view, clause and engine narrowing, and for nothing else (O16): another
    /// narrowing, another clause or a reloaded view is paired afresh.
    #[test]
    fn the_kept_pairing_answers_only_its_own_input() {
        let view = cars();
        let one = Filter {
            rules: vec![rule(&["Car"], Some("speed>10"), &["speed"])],
            ..Filter::default()
        };
        let two = Filter {
            rules: vec![rule(&["Car"], Some("speed>20"), &["speed"])],
            ..Filter::default()
        };
        let paired = |v: &CompiledView, clause: &str, f: &Filter| {
            with_view(Some(v), clause, Ok(Arc::new(f.clone())))
        };
        let first = paired(&view, "5.7.2", &one).expect("query");
        assert_eq!(
            *first,
            compose(view.narrowing(), one.clone()).expect("pair")
        );
        let again = paired(&view, "5.7.2", &one).expect("query");
        assert!(
            Arc::ptr_eq(&first, &again),
            "the same input is not paired twice"
        );
        let other = paired(&view, "5.7.2", &two).expect("query");
        assert_eq!(
            *other,
            compose(view.narrowing(), two.clone()).expect("pair")
        );
        assert_ne!(*other, *first);
        // a clause `resolve` refuses a filter on is refused, kept pairing or not
        assert!(paired(&view, "5.13.4", &one).is_err());
        assert!(paired(&view, "5.13.4", &one).is_err());
        assert_eq!(paired(&view, "5.7.2", &one).as_deref(), Ok(&*first));
        let reloaded = cars();
        let fresh = paired(&reloaded, "5.7.2", &one).expect("query");
        assert!(
            !Arc::ptr_eq(&fresh, &first),
            "a reloaded view is paired afresh"
        );
        assert_eq!(*fresh, *first);
    }

    /// T-3673: Query Subscriptions keeps the engine's selection among the
    /// view's own; the Registration and CSR Subscription lists are not
    /// narrowed by an Entity view at all.
    #[test]
    fn a_record_list_under_a_view_keeps_the_engines_selection_or_is_refused() {
        let view = cars();
        let mine = Filter {
            q: Some(Box::new(
                antares_ql::parse_q("subject==\"alice\"").expect("q"),
            )),
            ..Filter::default()
        };
        assert_eq!(
            with_view(Some(&view), "5.8.4", Ok(Arc::new(mine.clone()))),
            Ok(Arc::new(mine.clone()))
        );
        assert_eq!(
            with_view(Some(&view), "5.8.4", Ok(Arc::new(Filter::default()))),
            Ok(Arc::new(Filter::default()))
        );
        // a view that only selects, so nothing projects and only the rule
        // above refuses
        let fast = CompiledView::new(
            &["Car".into()],
            Some(antares_ql::parse_q("speed>50").expect("q")),
            None,
            None,
            vec![rule(&["Car"], None, &[])],
        )
        .expect("view");
        assert!(!fast.narrowing().projects());
        for v in [&view, &fast] {
            for clause in ["5.10.2", "5.11.5"] {
                assert!(with_view(Some(v), clause, Ok(Arc::new(Filter::default()))).is_err());
                assert!(with_view(Some(v), clause, Ok(Arc::new(mine.clone()))).is_err());
            }
        }
    }

    /// Cars only, showing `speed` and `colour`, never `secret`.
    fn cars() -> CompiledView {
        CompiledView::new(
            &["Car".into()],
            None,
            None,
            None,
            vec![rule(&["Car"], None, &["speed", "colour"])],
        )
        .expect("view")
    }

    async fn send(
        st: &AppState,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value, axum::http::HeaderMap) {
        let req = Request::builder().method(method).uri(uri);
        let req = match body {
            Some(v) => {
                let payload = v.to_string();
                req.header("Content-Type", "application/json")
                    .header("Content-Length", payload.len())
                    .body(Body::from(payload))
            }
            None => req.body(Body::empty()),
        }
        .expect("request");
        let resp = crate::router(st.clone())
            .oneshot(req)
            .await
            .expect("response");
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            headers,
        )
    }

    /// Five Cars (two with `secret`), two Bikes; seeded through a state
    /// without a view, which shares its store with every view state.
    async fn seeded(engine: Option<Filter>) -> AppState {
        let mut st = AppState::new("me".into());
        if let Some(f) = engine {
            st = st.with_policy(Arc::new(Narrowing(f)));
        }
        crate::wire(&mut st).await;
        for i in 0..5 {
            let mut car = json!({"id": format!("urn:ngsi-ld:Car:{i}"), "type": "Car",
                "speed": {"type": "Property", "value": i * 10},
                "colour": {"type": "Property", "value": if i % 2 == 0 { "red" } else { "blue" }}});
            if i < 2 {
                car["secret"] = json!({"type": "Property", "value": 7});
            }
            let (s, b, _) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(car)).await;
            assert_eq!(s, StatusCode::CREATED, "{s} {b}");
        }
        for i in 0..2 {
            let bike = json!({"id": format!("urn:ngsi-ld:Bike:{i}"), "type": "Bike",
                "speed": {"type": "Property", "value": 5}});
            let (s, b, _) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(bike)).await;
            assert_eq!(s, StatusCode::CREATED, "{s} {b}");
        }
        st
    }

    fn ids(body: &Value) -> Vec<String> {
        let mut v: Vec<String> = body
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| e["id"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// G8: a view alone sends no `NGSILD-Warning`; an engine's warning
    /// reaches every read it narrows, directly and through a view, once.
    #[tokio::test]
    async fn only_the_engine_warns_and_its_warning_survives_the_view() {
        let warnings = |h: &axum::http::HeaderMap| -> Vec<String> {
            h.get_all("NGSILD-Warning")
                .iter()
                .filter_map(|v| v.to_str().ok().map(str::to_owned))
                .collect()
        };
        let reads = [
            "/ngsi-ld/v1/entities?type=Car",
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:3",
            "/ngsi-ld/v1/types",
            "/ngsi-ld/v1/attributes",
        ];
        let st = seeded(None).await;
        let view = st.with_view(Arc::new(cars()));
        for uri in reads {
            let (s, body, headers) = send(&view, "GET", uri, None).await;
            assert_eq!(s, StatusCode::OK, "{uri}: {body}");
            assert!(warnings(&headers).is_empty(), "{uri}: a view alone");
        }

        let warned = Filter {
            warning: Some(Warning::new("types left out by the filter: Bike").expect("text")),
            ..Filter::default()
        };
        let st = seeded(Some(warned)).await;
        let view = st.with_view(Arc::new(cars()));
        for (who, st) in [("engine", &st), ("view and engine", &view)] {
            for uri in reads {
                let (s, body, headers) = send(st, "GET", uri, None).await;
                assert_eq!(s, StatusCode::OK, "{who} {uri}: {body}");
                assert_eq!(
                    warnings(&headers),
                    ["199 me \"types left out by the filter: Bike\""],
                    "{who} {uri}"
                );
            }
        }
    }

    fn next_link(h: &axum::http::HeaderMap) -> Option<String> {
        h.get_all("Link")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(", <"))
            .find(|l| l.contains("rel=\"next\""))
            .and_then(|l| l.trim_start_matches('<').split_once('>'))
            .map(|(target, _)| target.to_owned())
    }

    /// T-4907: an uncounted page's next Link carries a keyset cursor beside
    /// its offset; following it walks every Car of the view exactly once,
    /// and the cursor is refused on another view, another query, a counted
    /// or ordered read, or edited.
    #[tokio::test]
    async fn the_next_cursor_walks_the_view_and_is_bound_to_it() {
        let st = seeded(None).await;
        let view = st.with_view(Arc::new(cars()));
        for state in [&st, &view] {
            let (s, body, h) =
                send(state, "GET", "/ngsi-ld/v1/entities?type=Car&limit=2", None).await;
            assert_eq!(s, StatusCode::OK, "{body}");
            let mut seen = ids(&body);
            let mut next = next_link(&h);
            let first = next.clone().expect("2 of 5 Cars link the next page");
            assert!(
                first.contains("&cursor=") && first.contains("offset=2"),
                "{first}"
            );
            while let Some(path) = next {
                let (s, page, h) = send(state, "GET", &path, None).await;
                assert_eq!(s, StatusCode::OK, "{path}: {page}");
                assert!(ids(&page).len() <= 2, "{page}");
                seen.extend(ids(&page));
                next = next_link(&h);
            }
            seen.sort();
            let want: Vec<String> = (0..5).map(|i| format!("urn:ngsi-ld:Car:{i}")).collect();
            assert_eq!(seen, want);
            let other = if std::ptr::eq(state, &st) { &view } else { &st };
            let tampered = first.replace("&cursor=", "&cursor=00");
            for (to, path) in [
                (other, first.clone()),
                (state, first.replace("limit=2", "limit=3")),
                (state, format!("{first}&q=speed%3E1")),
                (state, format!("{first}&count=true")),
                (state, format!("{first}&orderBy=speed")),
                (state, tampered),
            ] {
                let (s, body, _) = send(to, "GET", &path, None).await;
                assert_eq!(s, StatusCode::BAD_REQUEST, "{path}: {body}");
            }
        }
        // a counted page keeps offset Links only
        let (_, _, h) = send(
            &view,
            "GET",
            "/ngsi-ld/v1/entities?type=Car&limit=2&count=true",
            None,
        )
        .await;
        let next = next_link(&h).expect("next");
        assert!(!next.contains("cursor="), "{next}");
    }

    /// Rule 1: `NGSILD-Results-Count` and the `Link` pages describe the
    /// narrowed set, the Cars, and nothing the view does not select.
    #[tokio::test]
    async fn count_and_pages_describe_the_view() {
        let st = seeded(None).await;
        let view = st.with_view(Arc::new(cars()));
        let uri = "/ngsi-ld/v1/entities?type=Car,Bike&count=true&limit=2";
        let (s, body, headers) = send(&view, "GET", uri, None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(headers["NGSILD-Results-Count"], "5");
        assert_eq!(ids(&body).len(), 2);
        let mut seen = ids(&body);
        let next_of = |h: &axum::http::HeaderMap| {
            h.get_all("Link")
                .iter()
                .filter_map(|v| v.to_str().ok())
                .flat_map(|v| v.split(", <"))
                .find(|l| l.contains("rel=\"next\""))
                .and_then(|l| l.trim_start_matches('<').split_once('>'))
                .map(|(target, _)| target)
                .map(str::to_owned)
        };
        let mut next = next_of(&headers);
        assert!(next.is_some(), "a first page of 2 out of 5 links the next");
        while let Some(path) = next {
            let (_, page, h) = send(&view, "GET", &path, None).await;
            seen.extend(ids(&page));
            next = next_of(&h);
        }
        seen.sort();
        assert_eq!(
            seen,
            (0..5)
                .map(|i| format!("urn:ngsi-ld:Car:{i}"))
                .collect::<Vec<_>>()
        );
    }

    /// Rule 2: `q` and `orderBy` over an Attribute the view hides answer as
    /// they do over Entities that never had it.
    #[tokio::test]
    async fn a_hidden_attribute_reads_as_absent() {
        let st = seeded(None).await;
        let view = st.with_view(Arc::new(cars()));
        for (q, expect) in [
            ("secret%3D%3D7", 0),
            ("secret", 0),
            ("!secret", 5),
            ("secret%3E1", 0),
        ] {
            let (s, body, _) = send(
                &view,
                "GET",
                &format!("/ngsi-ld/v1/entities?type=Car&q={q}"),
                None,
            )
            .await;
            assert_eq!(s, StatusCode::OK, "{q}: {body}");
            assert_eq!(ids(&body).len(), expect, "{q}: {body}");
        }
        let (_, body, _) = send(
            &view,
            "GET",
            "/ngsi-ld/v1/entities?type=Car&orderBy=secret,speed",
            None,
        )
        .await;
        let (_, plain, _) = send(
            &view,
            "GET",
            "/ngsi-ld/v1/entities?type=Car&orderBy=speed",
            None,
        )
        .await;
        let order = |b: &Value| {
            b.as_array()
                .expect("list")
                .iter()
                .map(|e| e["id"].clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(order(&body), order(&plain), "secret does not order");
        assert!(
            body.as_array()
                .expect("list")
                .iter()
                .all(|e| e.get("secret").is_none()),
            "{body}"
        );
    }

    /// The view applies with no engine and with `AllowAll`: the default
    /// engine behind a view serves only the view.
    #[tokio::test]
    async fn allow_all_behind_a_view_serves_only_the_view() {
        let st = seeded(None)
            .await
            .with_policy(Arc::new(crate::policy::AllowAll));
        let view = st.with_view(Arc::new(cars()));
        let (_, body, _) = send(&view, "GET", "/ngsi-ld/v1/entities?type=Bike", None).await;
        assert_eq!(ids(&body), Vec::<String>::new());
        let (s, _, _) = send(
            &view,
            "GET",
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Bike:0",
            None,
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "out of view reads as not found");
        let (s, body, _) = send(&view, "GET", "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:0", None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert!(body.get("secret").is_none(), "{body}");
        // without the view, the same state serves everything
        let (_, body, _) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Bike", None).await;
        assert_eq!(ids(&body).len(), 2);
    }

    /// An engine `Filter` and a view both narrow, neither widens: the engine
    /// keeps the slow Cars, the view keeps the Cars, the answer is both.
    #[tokio::test]
    async fn the_engine_and_the_view_both_narrow() {
        let slow = Filter {
            q: Some(Box::new(antares_ql::parse_q("speed<30").expect("q"))),
            pick: vec!["speed".into(), "secret".into()],
            ..Filter::default()
        };
        let st = seeded(Some(slow)).await;
        let view = st.with_view(Arc::new(cars()));
        let (_, body, _) = send(&view, "GET", "/ngsi-ld/v1/entities?type=Car,Bike", None).await;
        assert_eq!(
            ids(&body),
            vec![
                "urn:ngsi-ld:Car:0",
                "urn:ngsi-ld:Car:1",
                "urn:ngsi-ld:Car:2"
            ]
        );
        for e in body.as_array().expect("list") {
            assert!(e.get("speed").is_some(), "both show speed: {e}");
            assert!(e.get("colour").is_none(), "the engine hides colour: {e}");
            assert!(e.get("secret").is_none(), "the view hides secret: {e}");
        }
    }

    /// Under a view, a write [`CHECKED_WRITES`] names runs and is judged on
    /// the stored Entity (rule 7), a batch member by member; any other write
    /// is a narrowed write and refused, and an engine's narrowing still
    /// refuses a checked one.
    #[tokio::test]
    async fn a_write_under_a_view_is_checked_or_refused() {
        let st = seeded(None).await;
        let view = st.with_view(Arc::new(cars()));
        let car = json!({"id": "urn:ngsi-ld:Car:9", "type": "Car"});
        let (s, body, _) = send(&view, "POST", "/ngsi-ld/v1/entities", Some(car.clone())).await;
        assert_eq!(s, StatusCode::CREATED, "{body}");
        let bike = json!({"id": "urn:ngsi-ld:Bike:9", "type": "Bike"});
        let (s, body, _) = send(&view, "POST", "/ngsi-ld/v1/entities", Some(bike)).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
        assert!(body.to_string().contains(MOVES_OUT), "{body}");
        let batch = json!([{"id": "urn:ngsi-ld:Car:7", "type": "Car"}]);
        let (s, body, _) = send(
            &view,
            "POST",
            "/ngsi-ld/v1/entityOperations/upsert",
            Some(batch),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{body}");
        let batch = json!([
            {"id": "urn:ngsi-ld:Car:8", "type": "Car"},
            {"id": "urn:ngsi-ld:Bike:8", "type": "Bike"}
        ]);
        let (s, body, _) = send(
            &view,
            "POST",
            "/ngsi-ld/v1/entityOperations/create",
            Some(batch),
        )
        .await;
        assert_eq!(s, StatusCode::MULTI_STATUS, "{body}");
        assert_eq!(body["success"], json!(["urn:ngsi-ld:Car:8"]), "{body}");
        assert_eq!(body["errors"][0]["error"]["detail"], MOVES_OUT, "{body}");
        let ids = json!(["urn:ngsi-ld:Car:9", "urn:ngsi-ld:Bike:8"]);
        let (s, body, _) = send(
            &view,
            "POST",
            "/ngsi-ld/v1/entityOperations/delete",
            Some(ids),
        )
        .await;
        assert_eq!(s, StatusCode::MULTI_STATUS, "{body}");
        assert_eq!(body["success"], json!(["urn:ngsi-ld:Car:9"]), "{body}");
        let narrowed = Filter {
            pick: vec!["speed".into()],
            ..Filter::default()
        };
        assert!(with_view(Some(&cars()), "5.6.1", Ok(Arc::new(narrowed))).is_err());
        assert_eq!(
            with_view(Some(&cars()), "5.6.1", Ok(Arc::new(Filter::default()))),
            Ok(Arc::new(Filter::default()))
        );
    }

    /// `expiresAt` is an Entity member (4.22, Table 5.2.4-1), not an
    /// Attribute: a write through a view that carries it lands, and a hidden
    /// Attribute beside it is still refused.
    #[tokio::test]
    async fn an_entity_expires_at_passes_a_view() {
        let st = seeded(None).await;
        let view = st.with_view(Arc::new(cars()));
        let at = "2099-01-01T00:00:00Z";
        let batch = json!([{"id": "urn:ngsi-ld:Car:7", "type": "Car", "expiresAt": at}]);
        let (s, body, _) = send(
            &view,
            "POST",
            "/ngsi-ld/v1/entityOperations/upsert",
            Some(batch),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{body}");
        let car = json!({"id": "urn:ngsi-ld:Car:6", "type": "Car", "expiresAt": at});
        let (s, body, _) = send(&view, "POST", "/ngsi-ld/v1/entities", Some(car)).await;
        assert_eq!(s, StatusCode::CREATED, "{body}");
        let hidden = json!([{"id": "urn:ngsi-ld:Car:5", "type": "Car", "expiresAt": at,
            "secret": {"type": "Property", "value": 1}}]);
        let (s, body, _) = send(
            &view,
            "POST",
            "/ngsi-ld/v1/entityOperations/upsert",
            Some(hidden),
        )
        .await;
        assert_ne!(s, StatusCode::CREATED, "{body}");
    }

    /// Every write path a view checks (create, batch create/upsert/update,
    /// update and append Attributes, merge-patch, replace) as `(method, uri,
    /// body)`, each carrying `extra` beside a listed Attribute.
    fn write_paths(extra: &Value) -> Vec<(&'static str, String, Value)> {
        let with = |mut v: Value| {
            for (k, x) in extra.as_object().expect("members") {
                v[k] = x.clone();
            }
            v
        };
        let at = |id: &str| format!("/ngsi-ld/v1/entities/urn:ngsi-ld:Car:{id}");
        let speed = json!({"type": "Property", "value": 3});
        let entity = |id: &str| {
            with(json!({"id": format!("urn:ngsi-ld:Car:{id}"), "type": "Car", "speed": speed}))
        };
        vec![
            ("POST", "/ngsi-ld/v1/entities".into(), entity("20")),
            (
                "POST",
                "/ngsi-ld/v1/entityOperations/create".into(),
                json!([entity("21")]),
            ),
            (
                "POST",
                "/ngsi-ld/v1/entityOperations/upsert".into(),
                json!([entity("22")]),
            ),
            (
                "POST",
                "/ngsi-ld/v1/entityOperations/update".into(),
                json!([entity("0")]),
            ),
            (
                "PATCH",
                format!("{}/attrs", at("1")),
                with(json!({"speed": speed})),
            ),
            (
                "POST",
                format!("{}/attrs", at("2")),
                with(json!({"colour": {"type": "Property", "value": "green"}})),
            ),
            ("PATCH", at("3"), with(json!({"speed": speed}))),
            ("PUT", at("4"), entity("4")),
        ]
    }

    /// Reads one Car with its system members, outside any view.
    async fn stored(st: &AppState, id: &str) -> Value {
        let uri = format!("/ngsi-ld/v1/entities/urn:ngsi-ld:Car:{id}?options=sysAttrs");
        let (s, e, _) = send(st, "GET", &uri, None).await;
        assert_eq!(s, StatusCode::OK, "{uri}: {e}");
        e
    }

    /// The Car ids [`write_paths`] writes, and whether that path sets an
    /// Entity member: Update Attributes (5.6.2) changes existing Attributes
    /// only, so its `expiresAt` is no Attribute and lands nowhere.
    const WRITTEN: [(&str, bool); 8] = [
        ("20", true),
        ("21", true),
        ("22", true),
        ("0", true),
        ("1", false),
        ("2", true),
        ("3", true),
        ("4", true),
    ];

    /// An Entity `expiresAt` (4.22), a merge's NGSI-LD Null for it and an
    /// Attribute's own `expiresAt` land through a view that lists only the
    /// data Attributes, on every write path; the stored Entity carries the
    /// member, never an Attribute of that name.
    #[tokio::test]
    async fn every_write_path_carries_expires_at_through_a_view() {
        let at = "2099-01-01T00:00:00Z";
        let past = json!({"type": "Property", "value": 1, "expiresAt": "2000-01-01T00:00:00Z"});
        for extra in [json!({"expiresAt": at}), json!({"speed": past})] {
            let st = seeded(None).await;
            let view = st.with_view(Arc::new(cars()));
            for (method, uri, body) in write_paths(&extra) {
                let (s, out, _) = send(&view, method, &uri, Some(body)).await;
                assert!(
                    s.is_success() && s != StatusCode::MULTI_STATUS,
                    "{method} {uri} {extra}: {s} {out}"
                );
            }
            for (id, sets) in WRITTEN {
                let e = stored(&st, id).await;
                assert!(e.get(NGSI_EXPIRES).is_none(), "no Attribute expiresAt: {e}");
                if extra.get("expiresAt").is_some() {
                    assert_eq!(e.get("expiresAt").is_some(), sets, "{id}: {e}");
                } else {
                    assert!(
                        e.get("speed").is_none(),
                        "an expired instance reads as gone: {e}"
                    );
                }
            }
        }
        let st = seeded(None).await;
        let view = st.with_view(Arc::new(cars()));
        let uri = "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:3";
        let (s, out, _) = send(&view, "PATCH", uri, Some(json!({"expiresAt": at}))).await;
        assert!(s.is_success(), "{out}");
        assert_eq!(stored(&st, "3").await["expiresAt"], json!(at));
        let (s, out, _) = send(
            &view,
            "PATCH",
            uri,
            Some(json!({"expiresAt": "urn:ngsi-ld:null"})),
        )
        .await;
        assert!(s.is_success(), "merge removal: {out}");
        let e = stored(&st, "3").await;
        assert!(e.get("expiresAt").is_none(), "{e}");
    }

    const NGSI_EXPIRES: &str = "https://uri.etsi.org/ngsi-ld/default-context/expiresAt";

    /// System members never widen a view: `expiresAt` beside an Attribute
    /// the view does not list is refused on every write path.
    #[tokio::test]
    async fn a_system_member_never_lets_a_write_name_a_hidden_attribute() {
        let hidden = json!({"expiresAt": "2099-01-01T00:00:00Z",
            "secret": {"type": "Property", "value": 1}});
        let st = seeded(None).await;
        let view = st.with_view(Arc::new(cars()));
        for (method, uri, body) in write_paths(&hidden) {
            let (s, out, _) = send(&view, method, &uri, Some(body)).await;
            assert!(
                !s.is_success() || s == StatusCode::MULTI_STATUS,
                "{method} {uri}: {s} {out}"
            );
            assert!(
                out.to_string().contains(NOT_WRITABLE),
                "{method} {uri}: {out}"
            );
        }
    }

    /// `deletedAt` is the broker's alone (5.2.2: a producer's copy "shall"
    /// be ignored): a client sending it writes through a view on every path,
    /// and nothing of it is stored.
    #[tokio::test]
    async fn a_client_deleted_at_is_ignored_on_every_write_path() {
        let st = seeded(None).await;
        let view = st.with_view(Arc::new(cars()));
        for (method, uri, body) in write_paths(&json!({"deletedAt": "2000-01-01T00:00:00Z"})) {
            let (s, out, _) = send(&view, method, &uri, Some(body)).await;
            assert!(
                s.is_success() && s != StatusCode::MULTI_STATUS,
                "{method} {uri}: {s} {out}"
            );
        }
        for (id, _) in WRITTEN {
            let e = stored(&st, id).await;
            assert!(!e.to_string().contains("deletedAt"), "{id}: {e}");
        }
    }

    #[test]
    fn a_view_that_names_nothing_is_refused() {
        assert!(
            CompiledView::new(&[], None, None, None, vec![rule(&["Car"], None, &["a"])]).is_err()
        );
        assert!(CompiledView::new(&["Car".into()], None, None, None, vec![]).is_err());
        assert!(
            CompiledView::new(
                &["Car".into()],
                None,
                None,
                None,
                vec![rule(&["Bike"], None, &["a"])]
            )
            .is_err(),
            "a rule outside the selector shows nothing"
        );
    }

    /// Gap G2: both rules' id narrowings hold in the pair, and a pair no id
    /// is left for holds for nothing.
    #[test]
    fn a_pair_keeps_both_id_narrowings() {
        let by = |ids: &[&str], patterns: &[&str]| Rule {
            ids: ids.iter().map(|i| (*i).to_owned()).collect(),
            id_patterns: patterns.iter().map(|p| (*p).to_owned()).collect(),
            ..rule(&["Car"], None, &["speed"])
        };
        let ids_of = |a: Rule, b: Rule| {
            let pair = pair_rules(&[a], vec![b]).expect("pairs");
            (pair[0].ids.clone(), pair[0].id_patterns.clone())
        };
        let (c1, c2, c10) = ("urn:Car:1", "urn:Car:2", "urn:Car:10");
        let s = |v: &[&str]| v.iter().map(|i| (*i).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            ids_of(by(&[], &[]), by(&[c1], &["urn:Car:.*"])),
            (s(&[c1]), s(&["urn:Car:.*"]))
        );
        assert_eq!(ids_of(by(&[c2], &[]), by(&[], &[])), (s(&[c2]), s(&[])));
        assert_eq!(
            ids_of(by(&[c1, c2], &[]), by(&[c2, c10], &[])),
            (s(&[c2]), s(&[]))
        );
        assert_eq!(
            ids_of(by(&[], &["urn:Car:1.*"]), by(&[c1, c2, c10], &[])),
            (s(&[c1, c10]), s(&[])),
            "explicit ids through the other side's pattern"
        );
        assert_eq!(
            ids_of(by(&[c2], &[]), by(&[c1], &[])),
            (s(&[]), s(&[])),
            "nothing left: the pair holds for nothing"
        );
        assert_eq!(
            pair_rules(&[by(&[c2], &[])], vec![by(&[c1], &[])]),
            Ok(vec![Rule::default()])
        );
        assert!(
            pair_rules(&[by(&[], &["a"])], vec![by(&[], &["b"])]).is_err(),
            "two pattern lists refuse rather than widen"
        );
    }

    /// T-3697: the allow-all shortcut widens nothing. For every shape a
    /// view's narrowing takes, composing it with an engine that narrows
    /// nothing is the narrowing itself.
    #[test]
    fn a_view_composed_with_an_engine_that_narrows_nothing_is_the_view() {
        let area = antares_ql::geo::GeoQuery::from_params(
            &[
                ("georel".to_owned(), "near;maxDistance==10".to_owned()),
                ("geometry".to_owned(), "Point".to_owned()),
                ("coordinates".to_owned(), "[1,2]".to_owned()),
            ]
            .into_iter()
            .collect(),
        )
        .expect("geo")
        .expect("an area");
        let views = [
            cars(),
            CompiledView::new(
                &["Car".into(), "Bike".into()],
                Some(antares_ql::parse_q("speed>1").expect("q")),
                Some("/city/#".into()),
                Some(area),
                vec![
                    rule(&["Car"], Some("colour==\"red\""), &["speed"]),
                    Rule {
                        ids: vec!["urn:ngsi-ld:Bike:1".into()],
                        ..rule(&["Bike"], None, &[])
                    },
                ],
            )
            .expect("view"),
        ];
        for v in &views {
            assert_eq!(
                compose(v.narrowing(), Filter::default()),
                Ok(v.narrowing().clone())
            );
            assert_eq!(
                with_view(Some(v), "5.7.2", Ok(Arc::new(Filter::default()))),
                Ok(Arc::new(v.narrowing().clone()))
            );
        }
    }

    #[test]
    fn a_side_over_every_attribute_leaves_the_other_sides_list() {
        let every = Rule {
            every_attribute: true,
            ..rule(&["Car"], None, &[])
        };
        let both = pair_rules(
            std::slice::from_ref(&every),
            vec![rule(&["Car"], None, &["speed"])],
        )
        .expect("pairs");
        assert_eq!(both[0].attributes, vec!["speed"]);
        assert!(!both[0].every_attribute);
        let both =
            pair_rules(&[rule(&["Car"], None, &["colour"])], vec![every.clone()]).expect("pairs");
        assert_eq!(both[0].attributes, vec!["colour"]);
        assert!(!both[0].every_attribute);
        let both = pair_rules(std::slice::from_ref(&every), vec![every.clone()]).expect("pairs");
        assert!(both[0].every_attribute && both[0].attributes.is_empty());
    }

    #[test]
    fn rules_pair_and_disjoint_types_show_nothing() {
        let view = vec![rule(&["Car"], Some("speed>1"), &["speed", "colour"])];
        let engine = vec![rule(
            &["Car", "Bike"],
            Some("colour==\"red\""),
            &["speed", "secret"],
        )];
        let both = pair_rules(&view, engine).expect("pairs");
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].types, vec!["Car"]);
        assert_eq!(both[0].attributes, vec!["speed"]);
        assert!(matches!(both[0].q.as_deref(), Some(QNode::And(v)) if v.len() == 2));

        let none = pair_rules(&view, vec![rule(&["Bike"], None, &["speed"])]).expect("pairs");
        assert_eq!(
            none,
            vec![Rule::default()],
            "a rule with no type holds for nothing"
        );
        let doc = json!({"id": "urn:ngsi-ld:Car:1", "type": "Car", "speed": {"type": "Property", "value": 9}});
        let f = Filter {
            rules: none,
            ..Filter::default()
        };
        assert!(f.shown(&doc, crate::repr::policy_context()).is_none());
    }

    #[test]
    fn neither_side_widens() {
        let a = Filter {
            pick: vec!["speed".into()],
            omit: vec!["x".into()],
            scope_q: Some("/a".into()),
            ..Filter::default()
        };
        let b = Filter {
            pick: vec!["colour".into()],
            omit: vec!["y".into()],
            scope_q: Some("/a/b".into()),
            restricted: true,
            ..Filter::default()
        };
        let f = compose(&a, b).expect("composes");
        assert_eq!(f.pick, vec![NOTHING], "disjoint picks keep the frame alone");
        assert_eq!(f.omit, vec!["y", "x"]);
        assert!(f.scope_q.is_some() && f.restricted);
        assert_eq!(
            compose(&Filter::default(), Filter::default()).expect("empty"),
            Filter::default()
        );
    }

    /// A `within` box from longitude `west` to `east`, 1 degree either side
    /// of the equator.
    fn band(west: f64, east: f64) -> antares_ql::geo::GeoQuery {
        let params = std::collections::HashMap::from([
            ("georel".to_owned(), "within".to_owned()),
            ("geometry".to_owned(), "Polygon".to_owned()),
            (
                "coordinates".to_owned(),
                json!([[
                    [west, -1.0],
                    [east, -1.0],
                    [east, 1.0],
                    [west, 1.0],
                    [west, -1.0]
                ]])
                .to_string(),
            ),
        ]);
        antares_ql::geo::GeoQuery::from_params(&params)
            .expect("parses")
            .expect("present")
    }

    /// A Car at longitude `lon` in the internal form `Rule::holds` reads.
    fn car_at(lon: f64) -> Value {
        let ctx = crate::repr::policy_context();
        json!({"id": "urn:ngsi-ld:Car:x", "type": [ctx.expand_key("Car")],
               (antares_ql::geo::LOCATION_IRI): [{"type": "GeoProperty",
                   "value": {"type": "Point", "coordinates": [lon, 0.0]}}]})
    }

    /// ADR-0023 rule 1: a view's area and an engine's area at the same level
    /// compose, and every one holds. Dropping either would widen.
    #[test]
    fn two_areas_compose_and_both_hold() {
        let ctx = crate::repr::policy_context();
        let car = ctx.expand_key("Car");
        let a = Filter {
            geo_q: Some(Box::new(band(0.0, 2.0))),
            rules: vec![Rule {
                geo_q: Some(Box::new(band(0.0, 2.0))),
                ..rule(&[&car], None, &["speed"])
            }],
            ..Filter::default()
        };
        let b = Filter {
            geo_q: Some(Box::new(band(1.0, 3.0))),
            rules: vec![Rule {
                geo_q: Some(Box::new(band(1.0, 3.0))),
                ..rule(&[&car], None, &["speed"])
            }],
            ..Filter::default()
        };
        let both = compose(&a, b).expect("two areas compose");
        assert_eq!(both.areas().count(), 2, "{both:?}");
        assert_eq!(both.rules.len(), 1);
        let pair = &both.rules[0];
        assert_eq!(pair.areas().count(), 2);
        assert!(pair.holds(&car_at(1.5), ctx), "inside both");
        assert!(!pair.holds(&car_at(0.5), ctx), "inside the view's only");
        assert!(!pair.holds(&car_at(2.5), ctx), "inside the engine's only");
        // three levels deep: a composed narrowing composes again
        let again = compose(
            &both,
            Filter {
                geo_q: Some(Box::new(band(1.4, 1.6))),
                ..Filter::default()
            },
        )
        .expect("three areas compose");
        assert_eq!(again.areas().count(), 3);
    }

    #[test]
    fn too_many_rules_are_refused() {
        let many: Vec<Rule> = (0..65).map(|_| rule(&["Car"], None, &["a"])).collect();
        assert!(pair_rules(&many, many.clone()).is_err());
    }

    /// A view's area and an engine's area both narrow a query, its count
    /// and its pages; the request's own area still applies on top.
    #[tokio::test]
    async fn a_view_area_and_an_engine_area_both_narrow_the_query() {
        let mut st = AppState::new("me".into()).with_policy(Arc::new(Narrowing(Filter {
            geo_q: Some(Box::new(band(-0.5, 1.5))),
            ..Filter::default()
        })));
        crate::wire(&mut st).await;
        for lon in 0..4 {
            let car = json!({"id": format!("urn:ngsi-ld:Car:at{lon}"), "type": "Car",
                "speed": {"type": "Property", "value": lon},
                "location": {"type": "GeoProperty",
                             "value": {"type": "Point", "coordinates": [lon, 0]}}});
            let (s, b, _) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(car)).await;
            assert_eq!(s, StatusCode::CREATED, "{s} {b}");
        }
        let view = CompiledView::new(
            &["Car".into()],
            None,
            None,
            Some(band(0.5, 3.5)),
            vec![rule(&["Car"], None, &["speed", "location"])],
        )
        .expect("view");
        let view = st.with_view(Arc::new(view));
        let (s, body, headers) = send(
            &view,
            "GET",
            "/ngsi-ld/v1/entities?type=Car&count=true&limit=1",
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(ids(&body), vec!["urn:ngsi-ld:Car:at1"], "only inside both");
        assert_eq!(
            headers
                .get("NGSILD-Results-Count")
                .and_then(|v| v.to_str().ok()),
            Some("1")
        );
        let (_, body, _) = send(
            &view,
            "GET",
            "/ngsi-ld/v1/entities?type=Car&limit=1&offset=1",
            None,
        )
        .await;
        assert_eq!(ids(&body), Vec::<String>::new(), "no second page");
        let (_, body, _) = send(
            &view,
            "GET",
            "/ngsi-ld/v1/entities?type=Car&georel=within&geometry=Polygon&coordinates=%5B%5B%5B1.5%2C-1%5D%2C%5B3.5%2C-1%5D%2C%5B3.5%2C1%5D%2C%5B1.5%2C1%5D%2C%5B1.5%2C-1%5D%5D%5D",
            None,
        )
        .await;
        assert_eq!(
            ids(&body),
            Vec::<String>::new(),
            "the request's area on top"
        );
    }

    #[test]
    fn without_a_view_the_engines_answer_is_unchanged() {
        let f = Filter {
            pick: vec!["a".into()],
            ..Filter::default()
        };
        assert_eq!(
            with_view(None, "5.6.1", Ok(Arc::new(f.clone()))),
            Ok(Arc::new(f))
        );
        assert_eq!(
            with_view(Some(&cars()), "5.7.2", Err(Denied("no".into()))),
            Err(Denied("no".into()))
        );
    }

    /// T-4079: the export asks a view for all 21 types of a space whose
    /// policy grants 9. The engine answers one rule per requested type and
    /// the view holds hundreds; only the pairs that share a type compose, so
    /// the request is served with the 9 granted types instead of refused.
    #[test]
    fn twenty_one_requested_types_compose_per_type_not_as_a_product() {
        let granted = [
            "Vehicle",
            "GtfsStop",
            "TransitRoute",
            "GtfsStation",
            "GtfsAgency",
            "GtfsRoute",
            "GtfsShape",
            "Alert",
            "OffStreetParking",
        ];
        let requested: Vec<String> = granted
            .iter()
            .map(|t| (*t).to_owned())
            .chain((0..12).map(|i| format!("Other{i}")))
            .collect();
        let view: Vec<Rule> = (0..300)
            .map(|i| rule(&[granted[i % granted.len()]], None, &[]))
            .collect();
        let engine: Vec<Rule> = requested
            .iter()
            .map(|t| rule(&[t.as_str()], None, &[]))
            .collect();
        assert!(
            view.len() * engine.len() > MAX_COMPOSED_RULES,
            "the product alone was refused"
        );
        let pairs = pair_rules(&view, engine).expect("composes");
        assert_eq!(
            pairs.len(),
            300,
            "one pair per view rule, each with its own type"
        );
        let mut types: Vec<&str> = pairs
            .iter()
            .flat_map(|r| &r.types)
            .map(String::as_str)
            .collect();
        types.sort_unstable();
        types.dedup();
        let mut want = granted.to_vec();
        want.sort_unstable();
        assert_eq!(
            types, want,
            "the bound never widens: the granted types alone"
        );
    }

    /// T-4079: the bound still holds for pairs that do share a type, at
    /// exactly MAX_COMPOSED_RULES, and its refusal names it.
    #[test]
    fn the_bound_counts_pairs_that_share_a_type() {
        let view: Vec<Rule> = (0..MAX_COMPOSED_RULES)
            .map(|_| rule(&["Car"], None, &[]))
            .collect();
        assert_eq!(
            pair_rules(&view, vec![rule(&["Car"], None, &[])])
                .expect("at the bound")
                .len(),
            MAX_COMPOSED_RULES
        );
        let two = vec![
            rule(&["Car"], Some("speed>1"), &[]),
            rule(&["Car"], Some("speed<9"), &[]),
        ];
        assert_eq!(
            pair_rules(&view, two),
            Err(Denied(TOO_MANY_RULES.to_owned()))
        );
        assert!(TOO_MANY_RULES.contains(&MAX_COMPOSED_RULES.to_string()));
    }

    /// T-4079: a bound on the request answers 400 BadRequestData naming it,
    /// never the 403 a policy refusal is.
    #[tokio::test]
    async fn the_bound_answers_400_naming_it() {
        use axum::response::IntoResponse;
        let response =
            crate::negotiate::ApiError::from(Denied(TOO_MANY_RULES.to_owned())).into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = serde_json::from_slice(
            &response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes(),
        )
        .expect("json");
        assert_eq!(
            body["type"],
            "https://uri.etsi.org/ngsi-ld/errors/BadRequestData"
        );
        assert!(body["detail"]
            .as_str()
            .expect("detail")
            .contains("MAX_COMPOSED_RULES"));
        let other = crate::negotiate::ApiError::from(Denied("no".into())).into_response();
        assert_eq!(
            other.status(),
            StatusCode::FORBIDDEN,
            "a policy refusal stays 403"
        );
    }

    /// T-4455, T-4078: the gateway's read of a space with no selector names every type the
    /// space holds. Through a view of those 21 types whose engine grants 9 it is one request:
    /// the granted types' Entities, counted and paged by one cursor, and nothing of a type the
    /// engine does not grant, neither in the count nor in any page.
    #[tokio::test]
    async fn every_type_of_a_space_is_read_through_a_view_in_one_request() {
        let types: Vec<String> = (0..21).map(|i| format!("T{i:02}")).collect();
        let engine = Filter {
            rules: types
                .iter()
                .take(9)
                .map(|t| rule(&[t.as_str()], None, &["value"]))
                .collect(),
            ..Filter::default()
        };
        let st = seeded(Some(engine)).await;
        for t in &types {
            for n in 0..2 {
                let doc = json!({"id": format!("urn:ngsi-ld:{t}:{n}"), "type": t,
                    "value": {"type": "Property", "value": n},
                    "hidden": {"type": "Property", "value": n}});
                let (s, b, _) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(doc)).await;
                assert_eq!(s, StatusCode::CREATED, "{s} {b}");
            }
        }
        let rules = (0..300)
            .map(|i| rule(&[types[i % types.len()].as_str()], None, &["value"]))
            .collect();
        let view = st.with_view(Arc::new(
            CompiledView::new(&types, None, None, None, rules).expect("view"),
        ));
        let next_of = |h: &axum::http::HeaderMap| {
            h.get_all("Link")
                .iter()
                .filter_map(|v| v.to_str().ok())
                .flat_map(|v| v.split(", <"))
                .find(|l| l.contains("rel=\"next\""))
                .and_then(|l| l.trim_start_matches('<').split_once('>'))
                .map(|(target, _)| target.to_owned())
        };
        let mut next = Some(format!(
            "/ngsi-ld/v1/entities?type={}&count=true&limit=5",
            types.join(",")
        ));
        let (mut seen, mut pages) = (Vec::new(), 0);
        while let Some(path) = next {
            let (s, body, headers) = send(&view, "GET", &path, None).await;
            assert_eq!(s, StatusCode::OK, "{path}: {body}");
            if pages == 0 {
                assert_eq!(headers["NGSILD-Results-Count"], "18", "9 types, 2 each");
            }
            for entity in body.as_array().expect("an array") {
                assert!(entity.get("hidden").is_none(), "{entity}");
            }
            seen.extend(ids(&body));
            next = next_of(&headers);
            pages += 1;
        }
        assert_eq!(pages, 4, "18 Entities in pages of 5");
        seen.sort();
        let want: Vec<String> = types
            .iter()
            .take(9)
            .flat_map(|t| (0..2).map(move |n| format!("urn:ngsi-ld:{t}:{n}")))
            .collect();
        assert_eq!(seen, want, "each granted Entity once, no other");
    }
}
