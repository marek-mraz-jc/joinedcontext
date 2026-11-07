// SPDX-License-Identifier: EUPL-1.2
//! Representation transforms (6.3.7, 4.5.4, concise, sysAttrs, attrs
//! projection, lang filter) — applied on the INTERNAL expanded form before
//! compaction.

use crate::state::AppState;
use antares_jsonld::{compact_entity, compact_entity_shallow, Context};
use antares_model::NgsiError;
use antares_model::{is_meta, TenantId};
use antares_store::Kind;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct Repr {
    pub sys_attrs: bool,
    /// Whether `sysAttrs` also answers each instance's `modifiedBy`
    /// (DM-138): set from [`crate::provenance::shows_modified_by`], so a
    /// notification or any read without the gateway's grant never does.
    pub modified_by: bool,
    pub key_values: bool,
    pub concise: bool,
    /// expanded attribute IRIs to project (attrs=): entity meta stays.
    pub attrs: Option<Vec<String>>,
    /// pick= (4.21): STRICT projection — core members (id/type/scope/…) only
    /// survive when explicitly picked. Nodes may carry nested selections for
    /// linked entities.
    pub pick: Option<Vec<ProjNode>>,
    /// omit= (4.21): nodes WITHOUT children omit their head; nodes WITH
    /// children only constrain the linked entity below that head.
    pub omit: Option<Vec<ProjNode>>,
    /// The policy seam's own projection (ADR-0020), kept apart from the
    /// request's. A 4.21 projection belongs to ONE level — its nested
    /// selections describe the linked Entity below a head, so a bare `omit`
    /// name leaves that member alone on a joined document — while a
    /// narrowing is about what may be seen at all, and a member the subject
    /// may not see is no more visible one Relationship away. These travel
    /// down the join walk unchanged (`joined_repr`); `pick` and `omit` do
    /// not.
    pub policy_pick: Option<Vec<ProjNode>>,
    pub policy_omit: Option<Vec<ProjNode>>,
    /// The narrowing's row predicate, carried down the join walk like the
    /// policy projection: a Linked Entity is read the way the Linking Entity
    /// was (5.7.1.4), so one outside the narrowing is not embedded.
    pub policy_reach: Option<std::sync::Arc<PolicyReach>>,
    /// The narrowing's rules (ADR-0023 rule 11), names expanded
    /// ([`type_rules`]). Each document is judged by its own rules, so this
    /// too travels down the join walk unchanged.
    pub policy_types: Option<std::sync::Arc<crate::policy::Filter>>,
    /// The client's own `q`, apart from the engine's ([`note_reads`]): under
    /// rules it is evaluated on each Entity as it is shown, so an Attribute
    /// a matched rule does not show is absent to it (ADR-0023 rule 11, rule
    /// 2 per Entity). `None` unless `policy_types` is set.
    pub policy_client_q: Option<antares_ql::QNode>,
    pub lang: Option<String>,
    /// datasetId= instance filter; entry "@none" selects default instances.
    pub dataset_id: Option<Vec<String>>,
}

pub fn parse_repr(params: &HashMap<String, String>, ctx: &Context) -> Result<Repr, NgsiError> {
    let mut r = Repr::default();
    let mut format: Option<String> = None;
    if let Some(opts) = params.get("options") {
        for o in opts.split(',') {
            match o.trim() {
                "sysAttrs" => r.sys_attrs = true,
                "keyValues" | "simplified" => {
                    format.get_or_insert("simplified".into());
                }
                "concise" => {
                    format.get_or_insert("concise".into());
                }
                "normalized" => {
                    format.get_or_insert("normalized".into());
                }
                _ => {
                    return Err(NgsiError::InvalidRequest(format!(
                        "unsupported options value {o:?}"
                    )))
                }
            };
        }
    }
    // format wins over options on conflict (6.3.7)
    if let Some(f) = params.get("format") {
        match f.as_str() {
            "normalized" | "concise" | "simplified" | "keyValues" => {
                format = Some(if f == "keyValues" {
                    "simplified".into()
                } else {
                    f.clone()
                })
            }
            _ => {
                return Err(NgsiError::InvalidRequest(format!(
                    "unsupported format value {f:?}"
                )))
            }
        }
    }
    match format.as_deref() {
        Some("simplified") => r.key_values = true,
        Some("concise") => r.concise = true,
        _ => {}
    }
    check_projection_exclusive(params)?;
    if let Some(a) = params.get("attrs") {
        let mut list = Vec::new();
        for t in a.split(',') {
            let t = t.trim();
            if t.is_empty() || ENTITY_META.contains(&t) {
                return Err(NgsiError::BadRequestData(format!(
                    "invalid attribute name {t:?} in attrs"
                )));
            }
            list.push(ctx.expand_key(t));
        }
        r.attrs = Some(list);
    }
    if let Some(pck) = params.get("pick") {
        r.pick = Some(parse_projection(pck, ctx)?);
    }
    if let Some(o) = params.get("omit") {
        r.omit = Some(parse_projection(o, ctx)?);
    }
    r.lang = params.get("lang").cloned();
    if let Some(list) = params.get("datasetId") {
        let mut ids = Vec::new();
        for d in list.split(',').map(str::trim) {
            // Table 6.4.3.2-1: "Shall be valid URIs, "@none" for ... the default"
            if d != "@none" && antares_model::EntityId::new(d).is_err() {
                return Err(NgsiError::BadRequestData(format!(
                    "datasetId {d:?} is not a URI"
                )));
            }
            ids.push(d.to_owned());
        }
        r.dataset_id = Some(ids);
    }
    Ok(r)
}

/// 4.5.5.1: the datasetId "is of datatype URI, or equal to the JSON-LD
/// keyword `@none`", and "if no datasetId is provided, or `"datasetId":
/// "@none"` is supplied, it is considered as the default Attribute
/// instance". A default instance carries no datasetId of its own, so the
/// write paths that select ONE instance by the `?datasetId=` parameter
/// (5.6.5.4, 5.6.13.4) match `@none` against its absence. Instance members
/// reach the same rule one layer earlier, in 5.5.7 expansion.
pub fn target_dataset_id(params: &HashMap<String, String>) -> Option<&str> {
    params
        .get("datasetId")
        .map(String::as_str)
        .filter(|d| *d != "@none")
}

/// Narrow a representation by a policy decision (ADR-0020).
///
/// `omit` is appended to whatever the request asked to omit: removing more
/// members can only remove more. `pick` intersects with the request's,
/// because a pick that added a name would serve a member the request did
/// not ask for and the engine's narrowing would have widened the answer.
///
/// The Entity frame survives a policy pick. 6.5.3.1 makes `pick` reduce an
/// Entity "down to only contain the listed Entity members", and a client
/// that wants `id` back names it; but the engine is restricting what may be
/// seen rather than choosing a representation, and 5.2.4 makes `id` and
/// `type` the Entity — a document without them is not one.
pub fn narrow_projection(
    pick: &mut Option<Vec<ProjNode>>,
    omit: &mut Option<Vec<ProjNode>>,
    f: &crate::policy::Filter,
    ctx: &Context,
) -> Result<(), NgsiError> {
    if !f.omit.is_empty() {
        let mut nodes = policy_nodes(&f.omit);
        match omit {
            Some(own) => own.append(&mut nodes),
            None => *omit = Some(nodes),
        }
    }
    if !f.pick.is_empty() {
        let allowed = policy_nodes(&f.pick);
        let mut kept = match pick.take() {
            None => allowed,
            Some(own) => own
                .into_iter()
                .filter(|n| allowed.iter().any(|a| a.iri == n.iri))
                .collect(),
        };
        for frame in parse_projection("id,type", ctx)? {
            if !kept.iter().any(|n| n.raw == frame.raw) {
                kept.push(frame);
            }
        }
        *pick = Some(kept);
    }
    Ok(())
}

/// The same narrowing on a current-state [`Repr`], which is the one a join
/// walk descends. It goes into the policy fields rather than into the
/// request's own projection, so every document the answer carries is
/// narrowed and not only the one at the top: `?join=flat` returns Entities
/// reached over a Relationship, and a member the subject may not see is not
/// less hidden for being one hop away.
///
/// The Entity frame survives a policy pick — 5.2.4 makes `id` and `type`
/// the Entity, and a document without them is not one — which is why the
/// policy pick is applied to Attributes and never to the core members.
pub fn narrow_repr(r: &mut Repr, f: &std::sync::Arc<crate::policy::Filter>) {
    // A view or an engine answers every request with the same narrowing,
    // and the gate hands it out as one shared `Arc`: its reach is built
    // once and handed out again while that `Arc` comes back. The memo
    // holds the `Arc` itself, so its address cannot be reused by another
    // Filter while it is kept, and `ptr_eq` never answers for rules other
    // than its own (O16).
    type Reached = (
        std::sync::Arc<crate::policy::Filter>,
        Option<std::sync::Arc<PolicyReach>>,
        Option<std::sync::Arc<crate::policy::Filter>>,
    );
    thread_local! {
        static LAST: std::cell::RefCell<Option<Reached>> = const { std::cell::RefCell::new(None) };
    }
    let hit = LAST.with_borrow(|last| {
        last.as_ref()
            .filter(|l| std::sync::Arc::ptr_eq(&l.0, f))
            .map(|l| (l.1.clone(), l.2.clone()))
    });
    let (reach, rules) = hit.unwrap_or_else(|| {
        let built = reach_of(f);
        LAST.set(Some((f.clone(), built.0.clone(), built.1.clone())));
        built
    });
    fill(r, f, reach, rules);
}

/// The reach of `f` and its expanded rules, built afresh.
fn reach_of(
    f: &crate::policy::Filter,
) -> (
    Option<std::sync::Arc<PolicyReach>>,
    Option<std::sync::Arc<crate::policy::Filter>>,
) {
    let reach = crate::policy::Condition::of(f);
    let rules = reach.as_ref().and_then(|r| r.rules.clone());
    let reach = reach.map(std::sync::Arc::new);
    (reach, rules)
}

fn fill(
    r: &mut Repr,
    f: &crate::policy::Filter,
    reach: Option<std::sync::Arc<PolicyReach>>,
    rules: Option<std::sync::Arc<crate::policy::Filter>>,
) {
    if reach.is_some() {
        r.policy_reach = reach;
    }
    if !f.omit.is_empty() {
        r.policy_omit = Some(policy_nodes(&f.omit));
    }
    if !f.pick.is_empty() {
        r.policy_pick = Some(policy_nodes(&f.pick));
    }
    r.policy_types = rules;
}

/// The narrowing's rules with every type and Attribute name expanded
/// against [`POLICY_CONTEXT`], the form an internal document's keys and
/// `type` values are in; `None` when the narrowing has none. Conditions
/// stay as written: [`shown`] reads them through the same context.
pub(crate) fn type_rules(
    f: &crate::policy::Filter,
) -> Option<std::sync::Arc<crate::policy::Filter>> {
    if !f.has_rules() {
        return None;
    }
    // The expansion is a function of the rules alone, and a view or an
    // engine answers every request with the same ones: the last expansion
    // is kept per thread and handed out again while its rules come back
    // (T-3667). It is keyed by the whole input, so it never answers for
    // rules other than its own (O16).
    thread_local! {
        static LAST: std::cell::RefCell<Option<(crate::policy::Filter, std::sync::Arc<crate::policy::Filter>)>> =
            const { std::cell::RefCell::new(None) };
    }
    let hit = LAST.with_borrow(|last| {
        last.as_ref()
            .filter(|l| l.0.rules == f.rules && l.0.omit_by_type == f.omit_by_type)
            .map(|l| l.1.clone())
    });
    if hit.is_some() {
        return hit;
    }
    let expand = |names: &[String]| -> Vec<String> {
        names.iter().map(|n| POLICY_CONTEXT.expand_key(n)).collect()
    };
    let expanded = std::sync::Arc::new(crate::policy::Filter {
        rules: f
            .rules
            .iter()
            .map(|r| crate::policy::Rule {
                types: expand(&r.types),
                attributes: expand(&r.attributes),
                q: r.q.clone(),
                scope_q: r.scope_q.clone(),
                geo_q: r.geo_q.clone(),
                geo_also: r.geo_also.clone(),
                ids: r.ids.clone(),
                id_patterns: r.id_patterns.clone(),
                every_attribute: r.every_attribute,
                temporal: r.temporal.clone(),
            })
            .collect(),
        omit_by_type: f
            .omit_by_type
            .iter()
            .map(|(t, attrs)| (POLICY_CONTEXT.expand_key(t), expand(attrs)))
            .collect(),
        ..crate::policy::Filter::default()
    });
    let key = crate::policy::Filter {
        rules: f.rules.clone(),
        omit_by_type: f.omit_by_type.clone(),
        ..crate::policy::Filter::default()
    };
    LAST.set(Some((key, expanded.clone())));
    Some(expanded)
}

/// What the expanded `rules` let the INTERNAL document `doc` show: every
/// member without rules, `None` when no rule holds for it.
pub(crate) fn shown<'a>(
    rules: Option<&'a crate::policy::Filter>,
    doc: &Value,
) -> Option<crate::policy::Shown<'a>> {
    match rules {
        None => Some(crate::policy::Shown::default()),
        Some(f) => f.shown(doc, &POLICY_CONTEXT),
    }
}

/// The client's own `q` as the query path reads it (4.9 `expandValues`
/// applied), from the request as sent.
pub(crate) fn client_q(
    params: &HashMap<String, String>,
    ctx: &Context,
) -> Result<Option<antares_ql::QNode>, NgsiError> {
    params
        .get("q")
        .map(|q| {
            Ok(antares_ql::eval::apply_expand_values(
                antares_ql::parse_q(q)?,
                antares_ql::eval::expansion_list(
                    params.get("expandValues").map(String::as_str),
                    params.get("jsonKeys").map(String::as_str),
                )
                .as_deref(),
                ctx,
            ))
        })
        .transpose()
}

/// [`narrowing`] holding the client's own `q` as well, for a caller that
/// runs a query under the narrowing ([`note_reads`]).
pub(crate) fn narrowing_reading(
    f: &crate::policy::Filter,
    client_params: &HashMap<String, String>,
    ctx: &Context,
) -> Result<Repr, NgsiError> {
    let mut r = narrowing(f);
    note_reads(&mut r, client_params, ctx)?;
    Ok(r)
}

/// Record the client's own `q` on a narrowed representation, before the
/// engine's is conjoined into the parameters; nothing to do without rules.
pub(crate) fn note_reads(
    r: &mut Repr,
    client_params: &HashMap<String, String>,
    ctx: &Context,
) -> Result<(), NgsiError> {
    if r.policy_types.is_some() {
        r.policy_client_q = client_q(client_params, ctx)?;
    }
    Ok(())
}

/// Order `docs` by what each one shows, so `orderBy` on an Attribute the
/// narrowing hides orders as if it were absent (ADR-0023 rule 2, 7.6.2.2)
/// and never by the hidden value. Without a projecting narrowing, `order`
/// runs on `docs` as they are.
pub(crate) fn order_as_shown<E>(
    docs: &mut Vec<Value>,
    narrow: Option<&Repr>,
    order: impl FnOnce(&mut Vec<Value>) -> Result<(), E>,
) -> Result<(), E> {
    let Some(r) = narrow
        .filter(|r| r.policy_pick.is_some() || r.policy_omit.is_some() || r.policy_types.is_some())
    else {
        return order(docs);
    };
    // the position travels under a key no expanded IRI or core member can
    // take, and comes back off the ordered views
    const AT: &str = " position";
    let mut views: Vec<Value> = docs
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let mut v = policy_view(d, r);
            if let Some(o) = v.as_object_mut() {
                o.insert(AT.to_owned(), Value::from(i));
            }
            v
        })
        .collect();
    order(&mut views)?;
    let mut slots: Vec<Option<Value>> = std::mem::take(docs).into_iter().map(Some).collect();
    *docs = views
        .iter()
        .filter_map(|v| v.get(AT).and_then(Value::as_u64))
        .filter_map(|i| slots.get_mut(usize::try_from(i).ok()?)?.take())
        .collect();
    Ok(())
}

/// The rows a narrowing admits (ADR-0020 `Filter::q`, `scope_q` and its
/// areas).
#[derive(Debug, Clone)]
pub struct PolicyReach {
    pub q: Option<antares_ql::QNode>,
    pub scope_q: Option<String>,
    /// Every area the narrowing holds; each must hold (ADR-0023 rule 1).
    pub areas: Vec<antares_ql::geo::GeoQuery>,
    /// The narrowing's expanded rules: an Entity none of them holds for is
    /// not admitted (ADR-0023 rule 11).
    pub rules: Option<std::sync::Arc<crate::policy::Filter>>,
}

/// Whether the narrowing admits the INTERNAL document `doc`; no narrowing
/// admits everything. The one check the retrieve answer and every joined
/// Entity go through, so a join cannot reach a row the retrieve hides.
pub(crate) async fn policy_admits(
    st: &AppState,
    tenant: &TenantId,
    ctx: &Context,
    reach: Option<&PolicyReach>,
    doc: &Value,
) -> bool {
    let Some(reach) = reach else {
        return true;
    };
    if shown(reach.rules.as_deref(), doc).is_none() {
        return false;
    }
    if let Some(scope) = &reach.scope_q {
        if !crate::scope_matches(scope, doc) {
            return false;
        }
    }
    if !reach.areas.iter().all(|geo| geo.matches(doc, ctx)) {
        return false;
    }
    match &reach.q {
        Some(ast) => {
            crate::notify::linked_eval(st, tenant, None, |l| {
                antares_ql::eval::eval_q(ast, doc, ctx, l)
            })
            .await
        }
        None => true,
    }
}

/// The INTERNAL document `doc` as the narrowing lets it be read: the
/// Entity frame and the Attributes the policy projection keeps.
pub(crate) fn policy_view(doc: &Value, r: &Repr) -> Value {
    if r.policy_pick.is_none() && r.policy_omit.is_none() && r.policy_types.is_none() {
        return doc.clone();
    }
    let shown = shown(r.policy_types.as_deref(), doc).unwrap_or_else(crate::policy::Shown::nothing);
    match doc.as_object() {
        Some(obj) => Value::Object(
            obj.iter()
                .filter(|(k, _)| {
                    is_meta(k)
                        || (policy_projected(r.policy_pick.as_deref(), r.policy_omit.as_deref(), k)
                            && shown.allows(k))
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        None => doc.clone(),
    }
}

/// The narrowing half of a representation alone, for a caller that holds
/// the Filter but builds no representation of its own.
pub fn narrowing(f: &crate::policy::Filter) -> Repr {
    let mut r = Repr::default();
    let (reach, rules) = reach_of(f);
    fill(&mut r, f, reach, rules);
    r
}

/// The same `pick`/`omit` names in the form an already-compacted document
/// carries them. The query path folds a policy projection into the
/// request's own representation, where the names are IRIs; a notification
/// is compacted by `notify::build_data` long before the seam sees it, so
/// there the names have to travel the other way — parsed against the same
/// `@context` the document was compacted with, then compacted back. An
/// engine writes one rule set either way, and a rule written as an IRI
/// (which is what ADR-0020 asks of an engine) removes the member it names
/// rather than silently matching nothing.
pub fn compacted_filter(f: &crate::policy::Filter, ctx: &Context) -> crate::policy::Filter {
    let names = |raw: &[String]| -> Vec<String> {
        policy_nodes(raw)
            .iter()
            .map(|n| ctx.compact_iri(&n.iri))
            .collect()
    };
    let by_type = |m: &std::collections::BTreeMap<String, Vec<String>>| {
        m.iter()
            .map(|(t, attrs)| (ctx.compact_iri(&POLICY_CONTEXT.expand_key(t)), names(attrs)))
            .collect()
    };
    // the rules' conditions read the stored Entity, not this compacted
    // one: the notification path evaluates them itself (`notify::shown_now`)
    crate::policy::Filter {
        pick: names(&f.pick),
        omit: names(&f.omit),
        rules: Vec::new(),
        omit_by_type: by_type(&f.omit_by_type),
        ..f.clone()
    }
}

/// The `@context` a policy name is read in. Deliberately NOT the request's:
/// [`antares_jsonld::Context::expand_key`] consults the term map before it
/// decides a name is already an IRI, so a caller that binds the term a rule
/// names — or binds the rule's own IRI as a term, which its own inline
/// `@context` is enough to do — would move the rule off its target and walk
/// out of the narrowing. A deployment's rule means the same thing whatever
/// the caller sends. ADR-0020 asks an engine to write its rules as IRIs and
/// those pass through unchanged; a short name is read here the way a name a
/// request does not define is read anywhere else.
/// [`POLICY_CONTEXT`], for the rule conditions read outside this module.
pub(crate) fn policy_context() -> &'static Context {
    &POLICY_CONTEXT
}

static POLICY_CONTEXT: std::sync::LazyLock<Context> =
    std::sync::LazyLock::new(antares_jsonld::core_context);

/// A policy's `pick`/`omit` names as projection nodes, expanded against
/// [`POLICY_CONTEXT`].
///
/// Deliberately NOT the 4.21 parser: 6.5.3.1 lets those members name `"id"`,
/// `"type"`, `"scope"` or one projected Attribute, and ADR-0020 asks an
/// engine to write its rules against IRIs — while 4.21 reads a dot as the
/// sub-attribute path separator, so `https://uri.etsi.org/…/colour` through
/// that grammar becomes the member `https://uri` and removes nothing. An
/// engine names one member; it is expanded, and both forms of the name are
/// kept so the projection matches a document whichever form its keys are in.
fn policy_nodes(names: &[String]) -> Vec<ProjNode> {
    names
        .iter()
        .map(|n| ProjNode {
            raw: n.clone(),
            iri: POLICY_CONTEXT.expand_key(n),
            children: None,
        })
        .collect()
}

/// Maximum `{…}` selection depth of a projection tree — the number of
/// Linked Entity hops it implies (5.7.1.4: must not exceed joinLevel).
pub fn proj_depth(nodes: &[ProjNode]) -> usize {
    nodes
        .iter()
        .map(|n| match &n.children {
            Some(c) => proj_depth(c).saturating_add(1),
            None => 0,
        })
        .max()
        .unwrap_or(0)
}

/// The entity-level members of 4.5.1: everything an Entity carries that is
/// NOT an Attribute. Every layer that has to tell the two apart reads this
/// one list — `attrs`/`pick`/`omit` validation and projection here, the
/// notification diff and tombstone in `notify`, the 4.3.6.8 amendment in
/// `conformance`, the registration-scope narrowing in `federation`. A layer
/// with its own copy is a layer that will disagree with the others about
/// what an attribute is.
pub(crate) const ENTITY_META: &[&str] = antares_model::ENTITY_META_KEYS;

/// One node of a 4.21 attribute-projection expression; `children` carries a
/// nested `{…}` selection (applied to linked entities on join).
#[derive(Debug, Clone)]
pub struct ProjNode {
    pub raw: String,
    pub iri: String,
    pub children: Option<Vec<ProjNode>>,
}

/// Parse + validate a pick=/omit= value (4.21) into a projection tree.
pub(crate) fn parse_projection(s: &str, ctx: &Context) -> Result<Vec<ProjNode>, NgsiError> {
    let bad = || NgsiError::BadRequestData(format!("invalid attribute projection {s:?} (4.21)"));
    // Each `{…}` level is one Linked Entity hop (5.7.1.4), so a selection
    // deeper than the joinLevel ceiling can never be satisfied. Bounding it
    // here, before the recursive descent below, is what keeps the recursion
    // finite: pick=/omit= reach this parser as plain STRINGS — from the URI,
    // or from inside a query body, where the JSON nesting wall never sees
    // their braces.
    if s.is_empty()
        || s.matches('{').count() != s.matches('}').count()
        || crate::bounds::json_depth(s.as_bytes()) > crate::bounds::MAX_JOIN_LEVEL
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_,.:{}#/%-+@|".contains(c))
    {
        return Err(bad());
    }
    fn split_top(s: &str) -> Option<Vec<&str>> {
        let mut out = Vec::new();
        let mut depth = 0usize;
        let mut start = 0usize;
        for (i, c) in s.char_indices() {
            match c {
                '{' => depth = depth.saturating_add(1),
                '}' => depth = depth.checked_sub(1)?,
                // 4.21 orOp = | / , — both split at the same depth
                ',' | '|' if depth == 0 => {
                    out.push(&s[start..i]);
                    // the separator is one byte
                    start = i.saturating_add(1);
                }
                _ => {}
            }
        }
        out.push(&s[start..]);
        Some(out)
    }
    fn parse_nodes(s: &str, ctx: &Context) -> Result<Vec<ProjNode>, NgsiError> {
        let bad = |m: &str| NgsiError::BadRequestData(format!("invalid attribute projection: {m}"));
        let parts = split_top(s).ok_or_else(|| bad("unbalanced braces"))?;
        let mut out: Vec<ProjNode> = Vec::new();
        for t in parts {
            let t = t.trim();
            if t.is_empty() {
                return Err(bad("empty projection member"));
            }
            let (head_part, children) = match t.split_once('{') {
                Some((head, rest)) => {
                    let inner = rest
                        .strip_suffix('}')
                        .ok_or_else(|| bad("unclosed brace"))?;
                    (head, Some(parse_nodes(inner, ctx)?))
                }
                None => (t, None),
            };
            // An IRI (a `:` before any `.`) is one Attribute name (5.5.7):
            // the dotted path is a short name's syntax only.
            let iri = match (head_part.find(':'), head_part.find('.')) {
                (Some(colon), Some(dot)) => colon < dot,
                (colon, _) => colon.is_some(),
            };
            let head = if iri {
                head_part
            } else {
                head_part.split('.').next().unwrap_or(head_part)
            };
            if head.is_empty() || (!iri && head_part.split('.').any(str::is_empty)) {
                return Err(bad("empty path segment"));
            }
            if !head
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || "_:@#".contains(c))
            {
                return Err(bad("projection member starts with a special character"));
            }
            if out.iter().any(|n| n.raw == head) {
                return Err(bad("duplicate projection member"));
            }
            out.push(ProjNode {
                raw: head.to_owned(),
                iri: ctx.expand_key(head),
                children,
            });
        }
        Ok(out)
    }
    parse_nodes(s, ctx)
}

/// Apply the representation to an internal doc, producing a new internal doc
/// 4.21 Projections. CIM 009 V1.9.1 4.21 and TS 104 175 7.3.2 state no rule
/// for combining them; the ETSI TPs 018_18, 020_22 and 028_08 refuse pick
/// with attrs, omit with attrs and one member in both pick and omit. Antares
/// refuses every pair (T-5496 audit: a stricter reading, kept), the one
/// reading, so an operation cannot accept a combination another rejects.
pub fn check_projection_exclusive(params: &HashMap<String, String>) -> Result<(), NgsiError> {
    let excl = ["pick", "omit", "attrs"]
        .iter()
        .filter(|k| params.contains_key(**k))
        .count();
    if excl > 1 {
        return Err(NgsiError::BadRequestData(
            "pick, omit and attrs cannot be combined in one request".into(),
        ));
    }
    Ok(())
}

/// ADR-0020: does an Attribute survive the policy's own projection? Both
/// forms of the name are tried, because a document reaches this function
/// expanded on the query path and compacted on the notification path, and a
/// rule names one Attribute either way.
pub fn policy_projected(pick: Option<&[ProjNode]>, omit: Option<&[ProjNode]>, k: &str) -> bool {
    let names = |n: &ProjNode| n.iri == *k || n.raw == *k;
    if let Some(pick) = pick {
        if !pick.iter().any(names) {
            return false;
        }
    }
    if let Some(omit) = omit {
        if omit.iter().any(names) {
            return false;
        }
    }
    true
}

/// 4.21: does a core Entity member (id, type, scope, the system temporal
/// Properties) survive the projection? `pick` constrains core members
/// strictly — only what is named survives; `omit` drops a named member only
/// when the node carries no children, because a node with children
/// constrains the linked Entity below the head, not the head itself. The
/// current-state and temporal representations project core members by the
/// same rule and differ only below it.
pub fn meta_projected(pick: Option<&[ProjNode]>, omit: Option<&[ProjNode]>, k: &str) -> bool {
    if let Some(pick) = pick {
        if !pick.iter().any(|n| n.raw == *k) {
            return false;
        }
    }
    if let Some(omit) = omit {
        if omit.iter().any(|n| n.raw == *k && n.children.is_none()) {
            return false;
        }
    }
    true
}

/// ready for compaction.
pub fn apply(doc: &Value, r: &Repr) -> Value {
    let Some(obj) = doc.as_object() else {
        return doc.clone();
    };
    // an Entity no rule holds for was left out before it got here; one that
    // arrives anyway shows its frame only
    let shown = shown(r.policy_types.as_deref(), doc).unwrap_or_else(crate::policy::Shown::nothing);
    let mut out = Map::new();
    for (k, v) in obj {
        let is_meta = ENTITY_META.contains(&k.as_str());
        if is_meta {
            match k.as_str() {
                // 6.3.11 Table 6.3.11-1: expiresAt is a system temporal
                // attribute — included only when options=sysAttrs.
                "createdAt" | "modifiedAt" | "expiresAt" if !r.sys_attrs => continue,
                _ => {}
            }
            // the policy pick never reaches a core member: 5.2.4 makes id
            // and type the Entity, so a narrowing that removed them would
            // answer something that is not one
            if !meta_projected(r.pick.as_deref(), r.omit.as_deref(), k)
                || !meta_projected(None, r.policy_omit.as_deref(), k)
            {
                continue;
            }
            out.insert(k.clone(), v.clone());
            continue;
        }
        if let Some(keep) = &r.attrs {
            if !keep.contains(k) {
                continue;
            }
        }
        if let Some(pick) = &r.pick {
            if !pick.iter().any(|n| n.iri == *k) {
                continue;
            }
        }
        if let Some(drop) = &r.omit {
            if drop.iter().any(|n| n.iri == *k && n.children.is_none()) {
                continue;
            }
        }
        if !policy_projected(r.policy_pick.as_deref(), r.policy_omit.as_deref(), k)
            || !shown.allows(k)
        {
            continue;
        }
        let raw: Vec<Value> = v.as_array().cloned().unwrap_or_else(|| vec![v.clone()]);
        let kept: Vec<&Value> = raw
            .iter()
            .filter(|inst| match (&r.dataset_id, inst.get("datasetId")) {
                (None, _) => true,
                (Some(want), Some(Value::String(have))) => want.iter().any(|w| w == have),
                (Some(want), None) => want.iter().any(|w| w == "@none"),
                _ => false,
            })
            .collect();
        let mut instances: Vec<Value> = kept
            .iter()
            .map(|inst| transform_instance(inst, r))
            .collect();
        if instances.is_empty() {
            continue;
        }
        if r.key_values {
            if instances.len() == 1 {
                // 4.5.4: a lone instance simplifies to its bare value
                out.extend(instances.pop().map(|one| (k.clone(), one)));
            } else {
                // 4.5.4 multi-attribute case: a "dataset" map holding one
                // key-value pair for each datasetId, "@none" for the default
                // instance. The pairing below is positional, so `kept` and
                // `instances` must both still be whole here.
                let mut ds = Map::new();
                for (orig, simple) in kept.iter().zip(instances.iter()) {
                    let key = orig
                        .get("datasetId")
                        .and_then(Value::as_str)
                        .unwrap_or("@none");
                    ds.insert(key.to_owned(), simple.clone());
                }
                out.insert(
                    k.clone(),
                    serde_json::json!({ "dataset": Value::Object(ds) }),
                );
            }
        } else {
            out.insert(k.clone(), Value::Array(instances));
        }
    }
    Value::Object(out)
}

fn transform_instance(inst: &Value, r: &Repr) -> Value {
    let Some(obj) = inst.as_object() else {
        return inst.clone();
    };
    // lang filter first: LanguageProperty → Property under a selected language
    let mut obj = obj.clone();
    if let Some(lang) = &r.lang {
        apply_lang(&mut obj, lang);
    }

    if r.key_values {
        return simplified_value(&obj);
    }

    let mut out = Map::new();
    for (k, v) in &obj {
        match k.as_str() {
            // 6.3.11: expiresAt shares the sysAttrs gate on attribute
            // instances (current-state and temporal alike).
            "createdAt" | "modifiedAt" | "expiresAt" if !r.sys_attrs => continue,
            "type" if r.concise => continue,
            antares_jsonld::compact::MODIFIED_BY_KEY => {
                if r.sys_attrs && r.modified_by {
                    out.insert(antares_jsonld::compact::MODIFIED_BY.into(), v.clone());
                }
                continue;
            }
            _ => {}
        }
        // sub-attributes recurse
        if let Some(arr) = v.as_array().filter(|_| !is_reserved_member(k)) {
            let subs: Vec<Value> = arr.iter().map(|i| transform_instance(i, r)).collect();
            out.insert(k.clone(), Value::Array(subs));
        } else {
            out.insert(k.clone(), v.clone());
        }
    }
    if r.concise {
        // bare-value collapse: a Property with only `value` collapses
        if out.len() == 1 {
            if let Some(v) = out.get("value") {
                return v.clone();
            }
        }
    }
    Value::Object(out)
}

/// 4.15 Language Filter on one attribute instance (5.7.2.5, and the `lang`
/// rows of Tables 6.18.3.2-1 / 6.19.3.1 for the temporal forms): a
/// LanguageProperty "shall be converted into a Property" holding the chosen
/// languageMap entry, with the non-reified `lang` member naming it.
pub(crate) fn apply_lang(obj: &mut Map<String, Value>, lang: &str) {
    if obj.get("type").and_then(Value::as_str) != Some("LanguageProperty") {
        return;
    }
    let Some(lm) = obj.get("languageMap").and_then(Value::as_object) else {
        return;
    };
    if let Some((chosen_lang, value)) = select_lang(lm, lang) {
        obj.remove("languageMap");
        obj.insert("type".into(), Value::String("Property".into()));
        obj.insert("value".into(), value);
        obj.insert("lang".into(), Value::String(chosen_lang));
    }
}

/// 4.15 Language Filter: pick one languageMap entry for a lang priority
/// list. Ranges are ordered by their q weights (RFC 3282, default 1, list
/// position breaking ties); tags compare case-insensitively (RFC 5646); a
/// range matches an exact tag, then a longer tag by prefix (fr → fr-CH),
/// then a shorter tag by truncation (fr-CH → fr). "*" — or no match at
/// all — "shall default to any supported language" (@none preferred).
fn select_lang(lm: &Map<String, Value>, lang: &str) -> Option<(String, Value)> {
    let mut ranges: Vec<(f64, usize, &str)> = lang
        .split(',')
        .enumerate()
        .filter_map(|(i, part)| {
            let mut it = part.trim().split(';');
            let tag = it.next()?.trim();
            if tag.is_empty() {
                return None;
            }
            let q = it
                .find_map(|p| {
                    p.trim()
                        .strip_prefix("q=")
                        .and_then(|v| v.parse::<f64>().ok())
                })
                .unwrap_or(1.0);
            Some((q, i, tag))
        })
        .collect();
    ranges.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    let ci = |k: &str, want: &str| k.eq_ignore_ascii_case(want);
    for (q, _, want) in &ranges {
        // q=0 = "not acceptable" (RFC 3282); "*" = any → the fallback below
        if *q <= 0.0 || *want == "*" {
            continue;
        }
        if let Some((k, v)) = lm.iter().find(|(k, _)| ci(k, want)) {
            return Some((k.clone(), v.clone()));
        }
        if let Some((k, v)) = lm.iter().find(|(k, _)| {
            k.len() > want.len()
                && k.as_bytes().get(want.len()) == Some(&b'-')
                && ci(&k[..want.len()], want)
        }) {
            return Some((k.clone(), v.clone()));
        }
        let mut w = *want;
        while let Some(cut) = w.rfind('-') {
            w = &w[..cut];
            if let Some((k, v)) = lm.iter().find(|(k, _)| ci(k, w)) {
                return Some((k.clone(), v.clone()));
            }
        }
    }
    // any: prefer @none, then first
    if let Some(v) = lm.get("@none") {
        return Some(("@none".to_owned(), v.clone()));
    }
    lm.iter().next().map(|(k, v)| (k.clone(), v.clone()))
}

fn is_reserved_member(k: &str) -> bool {
    matches!(
        k,
        "type"
            | "value"
            | "object"
            | "objectType"
            | "datasetId"
            | "observedAt"
            | "unitCode"
            | "lang"
            | "languageMap"
            | "vocab"
            | "json"
            | "valueList"
            | "objectList"
            | "createdAt"
            | "modifiedAt"
            | "deletedAt"
            | "instanceId"
            | "previousValue"
            | "previousObject"
            | "previousLanguageMap"
            | "previousJson"
            | "previousVocab"
    )
}

/// 4.5.4: the simplified (keyValues) value of one instance — bare value for
/// Property/GeoProperty, bare URI(s) for a Relationship, bare ordered arrays
/// for ListProperty/ListRelationship, but the single-key wrapper objects
/// {"languageMap": …} / {"json": …} / {"vocab": …} for the Language, Json
/// and Vocab subtypes (Examples 4–6).
fn simplified_value(obj: &Map<String, Value>) -> Value {
    for k in ["value", "object", "valueList", "objectList"] {
        if let Some(v) = obj.get(k) {
            return v.clone();
        }
    }
    for k in ["languageMap", "json", "vocab"] {
        if let Some(v) = obj.get(k) {
            return serde_json::json!({ k: v.clone() });
        }
    }
    Value::Object(obj.clone())
}

/// Compaction for a shaped doc under a representation: keyValues docs get
/// shallow key renaming only (values are already plain JSON).
pub fn compact_for(
    repr: &crate::repr::Repr,
    shaped: &Value,
    ctx: &antares_jsonld::Context,
) -> Value {
    if repr.key_values {
        compact_entity_shallow(shaped, ctx)
    } else {
        compact_entity(shaped, ctx)
    }
}

/// The child representation for a linked entity under `key` (4.21 nested
/// projections apply to the joined entity, not the relationship itself).
fn joined_repr(parent: &crate::repr::Repr, key_compact: &str, key_iri: &str) -> crate::repr::Repr {
    let mut r = crate::repr::Repr {
        sys_attrs: parent.sys_attrs,
        modified_by: parent.modified_by,
        key_values: parent.key_values,
        concise: parent.concise,
        lang: parent.lang.clone(),
        // ADR-0020: the narrowing is not a per-level projection — it
        // travels to every document the answer carries
        policy_pick: parent.policy_pick.clone(),
        policy_omit: parent.policy_omit.clone(),
        policy_reach: parent.policy_reach.clone(),
        policy_types: parent.policy_types.clone(),
        ..Default::default()
    };
    if let Some(pick) = &parent.pick {
        if let Some(n) = pick
            .iter()
            .find(|n| n.raw == key_compact || n.iri == key_iri)
        {
            r.pick = n.children.clone();
        }
    }
    if let Some(omit) = &parent.omit {
        if let Some(n) = omit
            .iter()
            .find(|n| (n.raw == key_compact || n.iri == key_iri) && n.children.is_some())
        {
            r.omit = n.children.clone();
        }
    }
    r
}

/// 4.5.23.1: "When retrieving Linked Entities, it is necessary to limit
/// retrieval to avoid cascades of an excessive length, duplicates or loops."
/// joinLevel bounds the DEPTH of the walk; this bounds its WIDTH — the total
/// number of Linked Entity reads a single request may buy, so that a densely
/// linked graph cannot turn one retrieval into an unbounded store scan.
pub(crate) const MAX_JOIN_LOOKUPS: usize = 1_000;

/// State of one Linked Entity Retrieval walk (4.5.23.1): the entity ids
/// already resolved — a loop or a duplicate is never walked a second time —
/// and the remaining lookup budget. `complete` goes false as soon as the walk
/// left something out, which the caller reports as an NGSILD-Warning.
struct JoinWalk {
    seen: std::collections::BTreeSet<String>,
    budget: usize,
    complete: bool,
    /// The request's headers when distributed operations are on: a target
    /// held by a registered source is then read through its registration.
    remote: Option<axum::http::HeaderMap>,
}

impl JoinWalk {
    /// The Linking Entity is already part of the response, so it counts as
    /// resolved before the walk starts — and so does every id the client
    /// passed in `containedBy`. `budget` is what is LEFT of the request's
    /// allowance: a page walks one entity at a time and each walk hands the
    /// remainder to the next, so the ceiling bounds the request rather than
    /// each of its entities.
    fn rooted(root: Option<&str>, contained_by: &[String], budget: usize) -> Self {
        let mut seen: std::collections::BTreeSet<String> = contained_by.iter().cloned().collect();
        if let Some(id) = root {
            seen.insert(id.to_owned());
        }
        JoinWalk {
            seen,
            budget,
            complete: true,
            remote: None,
        }
    }
}

/// Linked Entity Retrieval, inline form (4.5.23.2): embed each relationship
/// target under an "entity" member (normalized) or replace the object URI by
/// the linked entity representation (simplified). Operates on COMPACTED docs.
/// Returns false when 4.5.23.1 truncated the walk (loop, duplicate, budget).
pub async fn inline_join(
    st: &AppState,
    tenant: &TenantId,
    ctx: &antares_jsonld::Context,
    repr: &crate::repr::Repr,
    compacted: &mut Value,
    level: usize,
) -> bool {
    inline_join_beyond(
        st,
        tenant,
        ctx,
        repr,
        compacted,
        level,
        &[],
        &mut { MAX_JOIN_LOOKUPS },
        None,
    )
    .await
}

/// Same, continuing an Entity Graph the client is already holding: the
/// `containedBy` ids count as encountered (Table 6.4.3.2-1). `remote` (the
/// request's headers, `None` with distributed operations off) lets a target
/// held by a registered source join too.
#[allow(clippy::too_many_arguments)] // one param per piece of the traversal's state
pub async fn inline_join_beyond(
    st: &AppState,
    tenant: &TenantId,
    ctx: &antares_jsonld::Context,
    repr: &crate::repr::Repr,
    compacted: &mut Value,
    level: usize,
    contained_by: &[String],
    budget: &mut usize,
    remote: Option<&axum::http::HeaderMap>,
) -> bool {
    let mut walk = JoinWalk::rooted(
        compacted.get("id").and_then(Value::as_str),
        contained_by,
        *budget,
    );
    walk.remote = remote.cloned();
    inline_join_walk(st, tenant, ctx, repr, compacted, level, &mut walk).await;
    *budget = walk.budget;
    walk.complete
}

/// One place in a compacted document where Linked Entities join: the JSON
/// pointer of a Relationship instance (or, simplified, of its URI string),
/// the representation its targets take, how they are written, and their ids.
struct Slot {
    at: String,
    child: crate::repr::Repr,
    kind: SlotKind,
    targets: Vec<String>,
    /// The Relationship's objectType (4.5.23.1), as written.
    object_type: Option<String>,
}

enum SlotKind {
    /// 4.5.23.2 normalized: the targets go under "entity".
    Entity,
    /// 4.5.22.2: a ListRelationship's targets go under "entityList".
    EntityList,
    /// simplified: the URI string itself is replaced by the target.
    Replace,
}

/// A key as one JSON pointer segment (RFC 6901: `~` and `/` escaped).
fn segment(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// The join slots of one compacted document, in document order.
fn slots_of(
    ctx: &antares_jsonld::Context,
    repr: &crate::repr::Repr,
    doc: &Value,
    at: &str,
) -> Vec<Slot> {
    let mut out = Vec::new();
    let Some(obj) = doc.as_object() else {
        return out;
    };
    let metas = ["id", "type", "scope", "createdAt", "modifiedAt", "@context"];
    for (k, v) in obj {
        if metas.contains(&k.as_str()) {
            continue;
        }
        let child = joined_repr(repr, k, &ctx.expand_key(k));
        slots_in(repr, &child, v, format!("{at}/{}", segment(k)), &mut out);
    }
    out
}

fn slots_in(
    repr: &crate::repr::Repr,
    child: &crate::repr::Repr,
    v: &Value,
    at: String,
    out: &mut Vec<Slot>,
) {
    let slot = |kind, targets, object_type| Slot {
        at: at.clone(),
        child: child.clone(),
        kind,
        targets,
        object_type,
    };
    match v {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                slots_in(repr, child, item, format!("{at}/{i}"), out);
            }
        }
        Value::Object(_) if repr.key_values => {}
        Value::Object(inst) => {
            // 4.5.22.2: the compacted objectList carries {"object": URI}
            // entries (or bare URIs).
            if let Some(Value::Array(ol)) = inst.get("objectList") {
                let targets = ol
                    .iter()
                    .filter_map(|e| match e {
                        Value::String(id) => Some(id.clone()),
                        Value::Object(o) => {
                            o.get("object").and_then(Value::as_str).map(str::to_owned)
                        }
                        _ => None,
                    })
                    .collect();
                out.push(slot(SlotKind::EntityList, targets, None));
                return;
            }
            let targets = match inst.get("object") {
                Some(Value::String(id)) => vec![id.clone()],
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                _ => return,
            };
            let object_type = match inst.get("objectType") {
                Some(Value::String(t)) => Some(t.clone()),
                _ => None,
            };
            out.push(slot(SlotKind::Entity, targets, object_type));
        }
        Value::String(id) if repr.key_values => {
            out.push(slot(SlotKind::Replace, vec![id.clone()], None));
        }
        _ => {}
    }
}

/// TS 104 175 7.7.4 Depth of retrieval: the walk goes breadth-first, one
/// depth at a time, so the first time an Entity is met is at its shallowest
/// depth, and that is where its own Linked Entities expand; every later
/// encounter embeds it without walking it again (4.5.23.1). Depth-first, an
/// Entity met first deep spent its one expansion there and stayed bare at
/// depth 1. The lookup budget and the `seen` set are the same ones.
async fn inline_join_walk(
    st: &AppState,
    tenant: &TenantId,
    ctx: &antares_jsonld::Context,
    repr: &crate::repr::Repr,
    compacted: &mut Value,
    level: usize,
    walk: &mut JoinWalk,
) {
    let mut frontier = vec![(String::new(), repr.clone(), level)];
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for (at, repr, level) in frontier {
            let slots = match compacted.pointer(&at) {
                Some(doc) => slots_of(ctx, &repr, doc, &at),
                None => continue,
            };
            for slot in slots {
                let mut joined = Vec::new();
                for id in &slot.targets {
                    if let Some(found) =
                        lookup_joined(st, tenant, ctx, &slot, id, level, walk).await
                    {
                        joined.push(found);
                    }
                }
                if joined.is_empty() {
                    continue;
                }
                let Some(place) = compacted.pointer_mut(&slot.at) else {
                    continue;
                };
                let single = joined.len() == 1;
                let (docs, expand): (Vec<Value>, Vec<bool>) = joined.into_iter().unzip();
                let at = match slot.kind {
                    SlotKind::Replace => {
                        *place = docs.into_iter().next().unwrap_or(Value::Null);
                        vec![slot.at.clone()]
                    }
                    SlotKind::EntityList => {
                        let count = docs.len();
                        if let Some(inst) = place.as_object_mut() {
                            inst.insert("entityList".into(), Value::Array(docs));
                        }
                        (0..count)
                            .map(|i| format!("{}/entityList/{i}", slot.at))
                            .collect()
                    }
                    SlotKind::Entity if single => {
                        if let Some(inst) = place.as_object_mut() {
                            inst.insert(
                                "entity".into(),
                                docs.into_iter().next().unwrap_or(Value::Null),
                            );
                        }
                        vec![format!("{}/entity", slot.at)]
                    }
                    SlotKind::Entity => {
                        let count = docs.len();
                        if let Some(inst) = place.as_object_mut() {
                            inst.insert("entity".into(), Value::Array(docs));
                        }
                        (0..count)
                            .map(|i| format!("{}/entity/{i}", slot.at))
                            .collect()
                    }
                };
                for (child_at, expand) in at.into_iter().zip(expand) {
                    if expand {
                        next.push((child_at, slot.child.clone(), level.saturating_sub(1)));
                    }
                }
            }
        }
        frontier = next;
    }
}

/// One Linked Entity read under the walk's budget and the narrowing: the
/// compacted target, and whether its own links are expanded (the first time
/// it is met, while depth remains). A target outside the narrowing reads as
/// absent, like a dangling link.
async fn lookup_joined(
    st: &AppState,
    tenant: &TenantId,
    ctx: &antares_jsonld::Context,
    slot: &Slot,
    id: &str,
    level: usize,
    walk: &mut JoinWalk,
) -> Option<(Value, bool)> {
    if walk.budget == 0 {
        walk.complete = false;
        return None;
    }
    walk.budget = walk.budget.saturating_sub(1);
    let child = &slot.child;
    let local = st.store.get(tenant, Kind::Entity, id).await.ok().flatten();
    let target = match local {
        Some(t) => t,
        None => remote_target(st, tenant, ctx, slot, id, walk).await?,
    };
    if !policy_admits(st, tenant, ctx, child.policy_reach.as_deref(), &target).await {
        return None;
    }
    let shaped = apply(&target, child);
    let c = compact_for(child, &shaped, ctx);
    let mut expand = false;
    if level > 1 {
        if walk.seen.insert(id.to_owned()) {
            expand = true;
        } else {
            // 4.5.23.1: an already-resolved target is a loop or a duplicate —
            // it is still embedded, but its own links are not walked again.
            walk.complete = false;
        }
    }
    Some((c, expand))
}

/// 4.5.23.1 (TS 104 175 7.7.1): "Only Relationships targeting a locally
/// stored Entity or Relationships annotated with an objectType whose object
/// is an Internal Linked Entity are considered to be retrievable in this
/// manner." A target not stored here joins only when the Relationship names
/// its objectType and a registration holds that id under that type; it is
/// then read as a distributed Retrieve Entity.
async fn remote_target(
    st: &AppState,
    tenant: &TenantId,
    ctx: &antares_jsonld::Context,
    slot: &Slot,
    id: &str,
    walk: &JoinWalk,
) -> Option<Value> {
    let (headers, object_type) = (walk.remote.as_ref()?, slot.object_type.as_ref()?);
    let spec = crate::registry::CsrSpec {
        ids: Some(vec![id.to_owned()]),
        types: Some(vec![ctx.expand_key(object_type)]),
        ..Default::default()
    };
    let regs = crate::federation::matching_regs(st, tenant, &spec, ctx, headers)
        .await
        .ok()?;
    if regs.is_empty() {
        return None;
    }
    let mut warnings = Vec::new();
    let fed = crate::federation::fed_retrieve(
        st,
        tenant,
        headers,
        ctx,
        id,
        None,
        None,
        &mut warnings,
        &mut None,
    )
    .await
    .ok()?;
    let mut base = fed
        .iter()
        .find(|(aux, _)| !aux)
        .or_else(|| fed.first())
        .map(|(_, d)| d.clone())?;
    for aux_pass in [false, true] {
        for (aux, d) in &fed {
            if *aux == aux_pass {
                crate::federation::merge_docs(&mut base, d, *aux);
            }
        }
    }
    Some(base)
}

/// Linked Entity Retrieval, flattened form (4.5.23.3): collect targets with
/// the child representation that applies to each. The Linking Entity is
/// already in the flattened array, so 4.5.23.1 ("avoid ... duplicates or
/// loops") keeps it out of `out` even when a Relationship points back at it.
/// Returns false when the walk was truncated by the lookup budget.
pub async fn collect_flat(
    st: &AppState,
    tenant: &TenantId,
    ctx: &Context,
    repr: &crate::repr::Repr,
    internal_doc: &Value,
    level: usize,
    out: &mut std::collections::BTreeMap<String, (Value, crate::repr::Repr)>,
) -> bool {
    collect_flat_beyond(st, tenant, ctx, repr, internal_doc, level, out, &[], &mut {
        MAX_JOIN_LOOKUPS
    })
    .await
}

/// Same, continuing an Entity Graph the client is already holding: the
/// `containedBy` ids count as encountered (Table 6.4.3.2-1).
#[allow(clippy::too_many_arguments)] // one param per piece of the traversal's state
pub async fn collect_flat_beyond(
    st: &AppState,
    tenant: &TenantId,
    ctx: &Context,
    repr: &crate::repr::Repr,
    internal_doc: &Value,
    level: usize,
    out: &mut std::collections::BTreeMap<String, (Value, crate::repr::Repr)>,
    contained_by: &[String],
    budget: &mut usize,
) -> bool {
    let mut walk = JoinWalk::rooted(
        internal_doc.get("id").and_then(Value::as_str),
        contained_by,
        *budget,
    );
    walk.seen.extend(out.keys().cloned());
    collect_flat_walk(st, tenant, ctx, repr, internal_doc, level, out, &mut walk).await;
    *budget = walk.budget;
    walk.complete
}

/// Breadth-first like [`inline_join_walk`] (TS 104 175 7.7.4): every Entity
/// of one depth is collected before the next depth is read, so an Entity is
/// walked from its shallowest occurrence and its Linked Entities get the
/// depth that occurrence leaves.
#[allow(clippy::too_many_arguments)] // one param per piece of the traversal's state
async fn collect_flat_walk(
    st: &AppState,
    tenant: &TenantId,
    ctx: &Context,
    repr: &crate::repr::Repr,
    internal_doc: &Value,
    level: usize,
    out: &mut std::collections::BTreeMap<String, (Value, crate::repr::Repr)>,
    walk: &mut JoinWalk,
) {
    let mut frontier = vec![(internal_doc.clone(), repr.clone(), level)];
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for (doc, repr, level) in &frontier {
            let Some(obj) = doc.as_object() else {
                continue;
            };
            let shown = shown(repr.policy_types.as_deref(), doc)
                .unwrap_or_else(crate::policy::Shown::nothing);
            for (k, v) in obj {
                if is_meta(k) {
                    continue;
                }
                // only traverse relationships that survive THIS doc's projection
                if let Some(pick) = &repr.pick {
                    if !pick.iter().any(|n| n.iri == *k || n.raw == *k) {
                        continue;
                    }
                }
                if let Some(omit) = &repr.omit {
                    if omit
                        .iter()
                        .any(|n| (n.iri == *k || n.raw == *k) && n.children.is_none())
                    {
                        continue;
                    }
                }
                // a Relationship the subject may not see is not a road either
                if !policy_projected(repr.policy_pick.as_deref(), repr.policy_omit.as_deref(), k)
                    || !shown.allows(k)
                {
                    continue;
                }
                let Some(instances) = v.as_array() else {
                    continue;
                };
                let child = joined_repr(repr, k, k);
                for inst in instances {
                    // Relationship objects plus ListRelationship objectList targets
                    // (internal form stores bare URIs) — 4.5.23.3 appends both kinds
                    // of Linked Entities to the flattened array.
                    let targets: Vec<&str> = match (inst.get("object"), inst.get("objectList")) {
                        (Some(Value::String(id)), _) => vec![id.as_str()],
                        (Some(Value::Array(a)), _) => a.iter().filter_map(Value::as_str).collect(),
                        (None, Some(Value::Array(a))) => {
                            a.iter().filter_map(Value::as_str).collect()
                        }
                        _ => continue,
                    };
                    for id in targets {
                        if walk.seen.contains(id) {
                            continue;
                        }
                        if walk.budget == 0 {
                            walk.complete = false;
                            return;
                        }
                        walk.budget = walk.budget.saturating_sub(1);
                        let Some(target) =
                            st.store.get(tenant, Kind::Entity, id).await.ok().flatten()
                        else {
                            continue;
                        };
                        if !policy_admits(st, tenant, ctx, child.policy_reach.as_deref(), &target)
                            .await
                        {
                            continue;
                        }
                        walk.seen.insert(id.to_owned());
                        out.insert(id.to_owned(), (target.clone(), child.clone()));
                        if *level > 1 {
                            next.push((target, child.clone(), level.saturating_sub(1)));
                        }
                    }
                }
            }
        }
        frontier = next;
    }
}

/// 4.5.16.2 GeoJSON Feature, members per Table 5.2.29-1 (5.2.29 Feature):
/// id = entity id (URI), fixed type "Feature", geometry = the selected
/// GeoProperty's value or null (4.5.16.1: geometryProperty parameter,
/// default "location"), properties = the 5.2.31 FeatureProperties (entity
/// type + attributes). The @context member is added by respond() (6.3.6).
pub fn to_geojson_feature(entity: Value, geometry_property: Option<&String>) -> Value {
    let geometry = geometry_of(&entity, geometry_property.map(String::as_str));
    let id = entity.get("id").cloned().unwrap_or(Value::Null);
    let mut props = entity.as_object().cloned().unwrap_or_default();
    props.remove("id");
    let mut feature = Map::new();
    feature.insert("id".into(), id);
    feature.insert("type".into(), Value::String("Feature".into()));
    feature.insert("geometry".into(), geometry);
    feature.insert("properties".into(), Value::Object(props));
    Value::Object(feature)
}

/// 4.5.16.3 GeoJSON FeatureCollection, members per Table 5.2.30-1 (5.2.30
/// FeatureCollection): fixed type "FeatureCollection" + features array of
/// 4.5.16.2 Feature objects — empty array when no matches, no per-Feature
/// @context; the top-level @context is added by respond() (6.3.6).
pub fn to_geojson_collection(entities: Vec<Value>, geometry_property: Option<&String>) -> Value {
    let features: Vec<Value> = entities
        .into_iter()
        .map(|e| to_geojson_feature(e, geometry_property))
        .collect();
    serde_json::json!({"type": "FeatureCollection", "features": features})
}

/// The one geometry rule of every GeoJSON this broker writes (NGSI-LD geo+json, OGC items,
/// the `geojson` surface; T-5208): the GeoProperty `property` (default `location`), its default
/// instance, `null` unless it holds a valid geometry, rings wound as RFC 7946 3.1.6 asks. A
/// geometry across the antimeridian is served as stored, not cut (3.1.9 is a SHOULD; no
/// dataset of the platform crosses it).
#[must_use]
pub fn geometry_of(entity: &Value, property: Option<&str>) -> Value {
    entity
        .get(property.unwrap_or("location"))
        .map(geo_value_of)
        .map_or(Value::Null, rewound)
}

/// RFC 7946 3.1.6: a Polygon's exterior ring counterclockwise, its holes clockwise, in every
/// Polygon of a MultiPolygon and a GeometryCollection; other geometries, and a ring across the
/// antimeridian, as they are.
#[must_use]
pub fn rewound(mut geometry: Value) -> Value {
    /// Twice the signed planar area, `None` for a ring with an edge across the antimeridian,
    /// whose plane winding is not its winding on the globe.
    fn area(ring: &[Value]) -> Option<f64> {
        let point = |p: &Value| {
            (
                p.get(0).and_then(Value::as_f64),
                p.get(1).and_then(Value::as_f64),
            )
        };
        let mut sum = 0.0;
        for w in ring.windows(2) {
            if let (Some((Some(x0), Some(y0))), Some((Some(x1), Some(y1)))) =
                (w.first().map(point), w.get(1).map(point))
            {
                if (x1 - x0).abs() > 180.0 {
                    return None;
                }
                sum += x0 * y1 - x1 * y0;
            }
        }
        Some(sum)
    }
    fn polygon(rings: &mut Value) {
        for (i, ring) in rings.as_array_mut().into_iter().flatten().enumerate() {
            if let Some(points) = ring.as_array_mut() {
                if area(points).is_some_and(|a| (a > 0.0) == (i > 0)) {
                    points.reverse();
                }
            }
        }
    }
    match geometry.get("type").and_then(Value::as_str) {
        Some("Polygon") => {
            if let Some(rings) = geometry.get_mut("coordinates") {
                polygon(rings);
            }
        }
        Some("MultiPolygon") => {
            let all = geometry
                .get_mut("coordinates")
                .and_then(Value::as_array_mut);
            all.into_iter().flatten().for_each(polygon);
        }
        Some("GeometryCollection") => {
            if let Some(Value::Array(parts)) = geometry.get_mut("geometries") {
                for part in parts.iter_mut() {
                    *part = rewound(part.take());
                }
            }
        }
        _ => {}
    }
    geometry
}

/// 4.5.16.1: with multiple instances the default one (no datasetId) is
/// selected unless a datasetId filter already narrowed the set to one; a
/// missing GeoProperty or a value that "does not hold a valid GeoJSON
/// geometry object" yields null — "which is syntactically valid GeoJSON".
fn geo_value_of(attr: &Value) -> Value {
    let inst = match attr {
        Value::Array(a) => match a.iter().find(|i| i.get("datasetId").is_none()) {
            Some(default) => default.clone(),
            None => match a.as_slice() {
                [only] => only.clone(),
                _ => return Value::Null,
            },
        },
        other => other.clone(),
    };
    let v = inst.get("value").cloned().unwrap_or(inst);
    // 4.5.17.1: in the simplified representation a multi-instance GeoProperty
    // is the {"dataset": {…}} map — the default ("@none") instance is the
    // 4.5.16.1 selection.
    let v = match v.as_object() {
        Some(o) if o.len() == 1 && o.contains_key("dataset") => {
            o["dataset"].get("@none").cloned().unwrap_or(Value::Null)
        }
        _ => v,
    };
    match antares_jsonld::expand::validate_geojson("geometry", &v) {
        Ok(()) => v,
        Err(_) => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 4.5.4 Examples 1–16: the simplified value of one instance per
    /// attribute type — bare for Property/GeoProperty/Relationship/List*,
    /// wrapped single-key objects for Language/Json/Vocab subtypes.
    #[test]
    fn simplified_values_per_attribute_type() {
        let v = |j: Value| simplified_value(j.as_object().unwrap());
        assert_eq!(v(json!({"type": "Property", "value": 5})), json!(5));
        assert_eq!(
            v(json!({"type": "Relationship", "object": "urn:a"})),
            json!("urn:a")
        );
        assert_eq!(
            v(json!({"type": "ListProperty", "valueList": [1, 2]})),
            json!([1, 2])
        );
        assert_eq!(
            v(json!({"type": "ListRelationship", "objectList": ["urn:a"]})),
            json!(["urn:a"])
        );
        assert_eq!(
            v(json!({"type": "LanguageProperty", "languageMap": {"en": "hi"}})),
            json!({"languageMap": {"en": "hi"}})
        );
        assert_eq!(
            v(json!({"type": "JsonProperty", "json": {"k": 1}})),
            json!({"json": {"k": 1}})
        );
        assert_eq!(
            v(json!({"type": "VocabProperty", "vocab": "V"})),
            json!({"vocab": "V"})
        );
    }
}

#[cfg(test)]
mod clause_4_15 {
    use super::*;
    use serde_json::json;

    fn lm(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), json!(*v)))
            .collect()
    }

    /// 4.15 EXAMPLE 4: quality value ranking — entries are ordered by their
    /// q weight (default 1), not by list position.
    #[test]
    fn q_values_rank_the_priority_list() {
        let m = lm(&[("en", "red"), ("fr", "rouge")]);
        let (l, v) = select_lang(&m, "en;q=0.2,fr;q=0.9").expect("pick");
        assert_eq!((l.as_str(), &v), ("fr", &json!("rouge")), "fr outranks en");
        // default q=1: plain fr-CH beats fr;q=0.9
        let m = lm(&[("fr-CH", "rouge suisse"), ("fr", "rouge")]);
        let (l, _) = select_lang(&m, "fr-CH,fr;q=0.9").expect("pick");
        assert_eq!(l, "fr-CH");
        // wildcard with low q still yields a fallback when nothing else fits
        let m = lm(&[("de", "rot")]);
        let (l, _) = select_lang(&m, "fr;q=0.9,*;q=0.5").expect("pick");
        assert_eq!(l, "de");
    }

    /// RFC 5646 (via 4.15): language tags compare case-insensitively.
    #[test]
    fn langtags_compare_case_insensitively() {
        let m = lm(&[("en-US", "color")]);
        let (l, v) = select_lang(&m, "en-us").expect("pick");
        assert_eq!((l.as_str(), &v), ("en-US", &json!("color")));
        let m = lm(&[("fr", "rouge"), ("de", "rot")]);
        let (l, _) = select_lang(&m, "FR").expect("pick");
        assert_eq!(l, "fr");
    }

    /// RFC 5646 lookup (via 4.15): a shorter range matches a longer tag
    /// (lang=fr picks fr-CH) and a longer range truncates onto a shorter tag
    /// (lang=fr-CH picks fr) — the `lang` subproperty reports the ACTUAL tag.
    #[test]
    fn prefix_and_truncation_fallbacks() {
        // decoy `de` sorts first — a naive any-fallback would pick it
        let m = lm(&[("de", "rot"), ("fr-CH", "rouge suisse")]);
        let (l, _) = select_lang(&m, "fr").expect("pick");
        assert_eq!(l, "fr-CH", "range fr matches tag fr-CH, not the decoy");
        let m = lm(&[("de", "rot"), ("fr", "rouge")]);
        let (l, _) = select_lang(&m, "fr-CH").expect("pick");
        assert_eq!(l, "fr", "range fr-CH truncates onto tag fr, not the decoy");
        // an exact match still beats a prefix match at the same rank
        let m = lm(&[("fr-CH", "suisse"), ("fr", "rouge")]);
        let (l, _) = select_lang(&m, "fr").expect("pick");
        assert_eq!(l, "fr");
    }

    /// 4.15: "If the Context Broker cannot serve any matching language, it
    /// shall default to any supported language" — and the augmented `lang`
    /// subproperty carries the actually returned one.
    #[test]
    fn no_match_falls_back_to_any_supported_language() {
        let m = lm(&[("de", "rot")]);
        let (l, v) = select_lang(&m, "pt").expect("fallback");
        assert_eq!((l.as_str(), &v), ("de", &json!("rot")));
        // the transform augments with the actual language and converts the
        // LanguageProperty to a Property — languageMap must NOT survive
        let inst = json!({"type": "LanguageProperty",
            "languageMap": {"en": "red", "fr": "rouge"}});
        let r = Repr {
            lang: Some("fr".into()),
            ..Repr::default()
        };
        let out = transform_instance(&inst, &r);
        assert_eq!(out["type"], "Property");
        assert_eq!(out["value"], "rouge");
        assert_eq!(out["lang"], "fr");
        assert!(
            out.get("languageMap").is_none(),
            "languageMap must not remain after conversion"
        );
    }
}

#[cfg(test)]
mod clause_4_21 {
    use super::*;
    use antares_jsonld::Loader;

    /// 4.21: "either a comma or a pipe character can be used as alternative
    /// representations of the or operator" — including inside a nested
    /// LinkedEntityTerm (EXAMPLE 3).
    #[test]
    fn pipe_and_comma_are_both_or_operators() {
        let ctx = Loader::new().core();
        let comma = parse_projection("temperature,humidity", &ctx).expect("comma");
        let pipe = parse_projection("temperature|humidity", &ctx).expect("pipe");
        assert_eq!(comma.len(), 2);
        assert_eq!(pipe.len(), 2);
        assert_eq!(comma[0].raw, pipe[0].raw);
        assert_eq!(comma[1].raw, pipe[1].raw);
        let nested = parse_projection("observation{temperature|humidity}", &ctx).expect("nested");
        assert_eq!(nested.len(), 1);
        let kids = nested[0].children.as_ref().expect("children");
        assert_eq!(kids.len(), 2, "pipe splits inside the braces too");
    }

    /// 4.21 grammar: an empty member or unbalanced braces are violations.
    #[test]
    fn grammar_rejections_hold_for_both_spellings() {
        let ctx = Loader::new().core();
        for bad in ["a||b", "a|,b", "|a", "a|", "a{b|}"] {
            assert!(
                parse_projection(bad, &ctx).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    /// Each `{…}` level of a projection is one Linked Entity hop (5.7.1.4),
    /// so a selection deeper than the joinLevel ceiling can never be
    /// satisfied. The depth is bounded BEFORE the recursive descent parses
    /// it: pick= arrives as a plain string inside a query body, where the
    /// JSON nesting wall never sees its braces.
    #[test]
    fn projection_nesting_is_bounded_before_the_parser_recurses() {
        let ctx = Loader::new().core();
        let cap = crate::bounds::MAX_JOIN_LEVEL;
        let nested = |n: usize| "a{".repeat(n) + "b" + &"}".repeat(n);
        // the body path can carry a projection far past any stack budget
        assert!(
            parse_projection(&nested(200_000), &ctx).is_err(),
            "a body-sized projection must be rejected, not recursed into"
        );
        assert!(parse_projection(&nested(cap), &ctx).is_ok(), "at the cap");
        assert!(
            parse_projection(&nested(cap + 1), &ctx).is_err(),
            "one level over the cap must be rejected"
        );
    }

    /// 5.7.1.4: proj_depth reports the hops a projection implies, so the
    /// joinLevel comparison upstream is made against the deepest branch.
    #[test]
    fn projection_depth_counts_the_deepest_branch() {
        let ctx = Loader::new().core();
        let d = |s: &str| proj_depth(&parse_projection(s, &ctx).expect("parse"));
        assert_eq!(d("a,b"), 0, "a flat selection implies no hop");
        assert_eq!(d("a{b}"), 1);
        assert_eq!(d("a,b{c{d}}"), 2, "the deepest branch wins");
    }
}

#[cfg(test)]
mod clause_6_3_7 {
    use super::*;
    use antares_jsonld::Loader;
    use serde_json::json;

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// 6.3.7: an unknown options or format value is not silently ignored.
    #[test]
    fn unknown_options_and_format_values_are_rejected() {
        let ctx = Loader::new().core();
        for p in [
            params(&[("options", "sysattrs")]),
            params(&[("options", "keyValues,bogus")]),
            params(&[("options", "")]),
            params(&[("format", "verbose")]),
            params(&[("format", "KeyValues")]),
        ] {
            let e = parse_repr(&p, &ctx).expect_err("must be rejected");
            assert!(
                matches!(e, NgsiError::InvalidRequest(_)),
                "unsupported representation value is InvalidRequest, got {e:?}"
            );
        }
    }

    /// 6.3.7: format wins over options when the two disagree, and keyValues
    /// is the older spelling of simplified.
    #[test]
    fn format_wins_over_options_on_conflict() {
        let ctx = Loader::new().core();
        let r = parse_repr(
            &params(&[("options", "concise"), ("format", "simplified")]),
            &ctx,
        )
        .expect("parse");
        assert!(r.key_values, "format=simplified wins");
        assert!(!r.concise, "options=concise must NOT survive the conflict");
        let r = parse_repr(
            &params(&[("options", "keyValues"), ("format", "normalized")]),
            &ctx,
        )
        .expect("parse");
        assert!(!r.key_values && !r.concise, "normalized is neither");
        let r = parse_repr(&params(&[("options", "sysAttrs,keyValues")]), &ctx).expect("parse");
        assert!(r.sys_attrs && r.key_values);
    }

    /// Table 6.4.3.2-1 datasetId: "Shall be valid URIs, "@none" for including
    /// the default Attribute instances" (Coraine url_param_datasetid_validation).
    #[test]
    fn a_dataset_id_parameter_is_a_uri_or_none() {
        let ctx = Loader::new().core();
        for bad in ["14", "urn:ds:1,14", ""] {
            let e = parse_repr(&params(&[("datasetId", bad)]), &ctx).expect_err(bad);
            assert!(matches!(e, NgsiError::BadRequestData(_)), "{bad}: {e:?}");
        }
        assert!(parse_repr(&params(&[("datasetId", "urn:ds:1,@none")]), &ctx).is_ok());
    }

    /// pick, omit and attrs: any pair is a 400 (see check_projection_exclusive),
    /// each one alone is fine.
    #[test]
    fn pick_omit_and_attrs_cannot_be_combined() {
        let ctx = Loader::new().core();
        for p in [
            params(&[("pick", "a"), ("omit", "b")]),
            params(&[("pick", "a"), ("attrs", "b")]),
            params(&[("omit", "a"), ("attrs", "b")]),
            params(&[("pick", "a"), ("omit", "b"), ("attrs", "c")]),
        ] {
            let e = parse_repr(&p, &ctx).expect_err("must be rejected");
            assert!(matches!(e, NgsiError::BadRequestData(_)), "got {e:?}");
        }
        assert!(parse_repr(&params(&[("pick", "a")]), &ctx).is_ok());
        assert!(parse_repr(&params(&[("attrs", "a")]), &ctx).is_ok());
    }

    /// attrs= selects ATTRIBUTES: an entity meta member or @context is not an
    /// attribute name, and an empty member is a grammar violation.
    #[test]
    fn attrs_rejects_entity_members_and_empty_names() {
        let ctx = Loader::new().core();
        for bad in ["id", "type", "scope", "createdAt", "@context", "a,,b", ""] {
            assert!(
                parse_repr(&params(&[("attrs", bad)]), &ctx).is_err(),
                "attrs={bad:?} must be rejected"
            );
        }
        let r = parse_repr(&params(&[("attrs", "temperature, humidity")]), &ctx).expect("parse");
        let list = r.attrs.expect("attrs");
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|a| a.contains("://")), "names are expanded");
    }

    fn entity() -> Value {
        json!({
            "id": "urn:ngsi-ld:E:1",
            "type": "T",
            "createdAt": "2026-01-01T00:00:00Z",
            "modifiedAt": "2026-01-02T00:00:00Z",
            "https://example.org/temperature": [
                {"type": "Property", "value": 21,
                 "createdAt": "2026-01-01T00:00:00Z",
                 "https://example.org/accuracy": [{"type": "Property", "value": 0.5}]},
                {"type": "Property", "value": 9, "datasetId": "urn:ds:2"}
            ]
        })
    }

    /// 6.3.11 Table 6.3.11-1: createdAt/modifiedAt are system attributes —
    /// they must be absent from the default representation, on the entity AND
    /// on every attribute instance, and present with options=sysAttrs.
    #[test]
    fn system_attributes_stay_hidden_without_sysattrs() {
        let plain = apply(&entity(), &Repr::default());
        assert!(plain.get("createdAt").is_none(), "entity createdAt leaked");
        assert!(
            plain.get("modifiedAt").is_none(),
            "entity modifiedAt leaked"
        );
        assert!(
            plain["https://example.org/temperature"][0]
                .get("createdAt")
                .is_none(),
            "instance createdAt leaked"
        );
        assert_eq!(plain["id"], "urn:ngsi-ld:E:1");
        let sys = apply(
            &entity(),
            &Repr {
                sys_attrs: true,
                ..Repr::default()
            },
        );
        assert_eq!(sys["createdAt"], "2026-01-01T00:00:00Z");
        assert_eq!(
            sys["https://example.org/temperature"][0]["createdAt"],
            "2026-01-01T00:00:00Z"
        );
    }

    /// 4.5.4 concise: the type member is dropped and an instance left with a
    /// lone value collapses to that bare value — sub-attributes included.
    #[test]
    fn concise_drops_type_and_collapses_bare_values() {
        let out = apply(
            &entity(),
            &Repr {
                concise: true,
                ..Repr::default()
            },
        );
        let inst = &out["https://example.org/temperature"][0];
        assert!(inst.get("type").is_none(), "concise keeps no type member");
        assert_eq!(
            inst["https://example.org/accuracy"],
            json!([0.5]),
            "a value-only sub-attribute collapses to the bare value"
        );
        // an instance carrying more than value keeps the object form
        let d = apply(
            &json!({"https://example.org/a": [{"type": "Property", "value": 1, "unitCode": "CEL"}]}),
            &Repr {
                concise: true,
                ..Repr::default()
            },
        );
        assert_eq!(d["https://example.org/a"][0]["unitCode"], "CEL");
        assert_eq!(d["https://example.org/a"][0]["value"], 1);
    }

    /// 4.5.4: with several instances the simplified form is a dataset map
    /// keyed by datasetId, "@none" standing for the default instance — a bare
    /// array of values would lose which instance is which. The map carries one
    /// pair for each datasetId, so a dropped instance is a dropped datasetId.
    #[test]
    fn simplified_multi_instance_uses_the_dataset_map() {
        let out = apply(
            &entity(),
            &Repr {
                key_values: true,
                ..Repr::default()
            },
        );
        let t = &out["https://example.org/temperature"];
        assert!(t.get("dataset").is_some(), "multi-instance needs the map");
        assert_eq!(t["dataset"]["@none"], 21);
        assert_eq!(t["dataset"]["urn:ds:2"], 9);
        assert!(t.as_array().is_none(), "not a bare array");
        // one instance stays a bare value
        let one = apply(
            &json!({"https://example.org/a": [{"type": "Property", "value": 1}]}),
            &Repr {
                key_values: true,
                ..Repr::default()
            },
        );
        assert_eq!(one["https://example.org/a"], json!(1));
        // 4.5.4 EXAMPLE 2 is three instances; the map is built by pairing
        // instances with the rows they came from, so an instance lost on the
        // way leaves a map one pair short rather than an error
        let three = apply(
            &json!({"https://example.org/name": [
                {"type": "Property", "value": "David Robert Jones"},
                {"type": "Property", "value": "David Bowie",
                 "datasetId": "urn:ngsi-ld:datasetId:001"},
                {"type": "Property", "value": "Ziggy Stardust",
                 "datasetId": "urn:ngsi-ld:datasetId:002"}
            ]}),
            &Repr {
                key_values: true,
                ..Repr::default()
            },
        );
        let ds = three["https://example.org/name"]["dataset"]
            .as_object()
            .expect("dataset map");
        assert_eq!(ds.len(), 3, "one pair for each datasetId");
        assert_eq!(ds["@none"], "David Robert Jones");
        assert_eq!(ds["urn:ngsi-ld:datasetId:001"], "David Bowie");
        assert_eq!(ds["urn:ngsi-ld:datasetId:002"], "Ziggy Stardust");
    }

    /// datasetId= selects instances; "@none" selects the default one. An
    /// attribute left with no surviving instance is absent, not empty.
    #[test]
    fn dataset_id_filter_selects_instances_and_drops_empty_attributes() {
        let sel = |ids: &[&str]| {
            apply(
                &entity(),
                &Repr {
                    dataset_id: Some(ids.iter().map(|s| (*s).to_owned()).collect()),
                    ..Repr::default()
                },
            )
        };
        let out = sel(&["urn:ds:2"]);
        let insts = out["https://example.org/temperature"]
            .as_array()
            .expect("array");
        assert_eq!(insts.len(), 1);
        assert_eq!(insts[0]["value"], 9, "the default instance must be gone");
        let out = sel(&["@none"]);
        let insts = out["https://example.org/temperature"]
            .as_array()
            .expect("array");
        assert_eq!(insts.len(), 1);
        assert_eq!(insts[0]["value"], 21);
        let out = sel(&["urn:ds:absent"]);
        assert!(
            out.get("https://example.org/temperature").is_none(),
            "an attribute with no matching instance is omitted entirely"
        );
        assert_eq!(out["id"], "urn:ngsi-ld:E:1", "entity members survive");
    }

    /// The reserved members of an attribute instance carry values, not
    /// sub-attributes: an array-valued `value` (or objectList, previousValue…)
    /// must be passed through untouched, never walked as a list of instances.
    #[test]
    fn array_valued_reserved_members_are_not_sub_attributes() {
        let doc = json!({"https://example.org/a": [{
            "type": "ListProperty",
            "valueList": [1, 2, 3],
            "value": [{"type": "Property", "value": 7}],
            "previousValue": [9],
            "https://example.org/note": [{"type": "Property", "value": "sub"}]
        }]});
        let out = apply(&doc, &Repr::default());
        let inst = &out["https://example.org/a"][0];
        assert_eq!(inst["valueList"], json!([1, 2, 3]));
        assert_eq!(
            inst["value"],
            json!([{"type": "Property", "value": 7}]),
            "an array value is data, not an instance list to transform"
        );
        assert_eq!(inst["previousValue"], json!([9]));
        // a genuine sub-attribute IS walked
        assert_eq!(inst["https://example.org/note"][0]["value"], "sub");
        // …and the walk applies the representation to it
        let sys = apply(
            &json!({"https://example.org/a": [{"type": "Property", "value": 1,
                "https://example.org/note": [{"type": "Property", "value": "s",
                    "modifiedAt": "2026-01-01T00:00:00Z"}]}]}),
            &Repr::default(),
        );
        assert!(
            sys["https://example.org/a"][0]["https://example.org/note"][0]
                .get("modifiedAt")
                .is_none(),
            "the sysAttrs gate reaches sub-attributes"
        );
    }

    /// 4.21: pick constrains core members too — an entity member not picked
    /// does not survive; omit only drops the heads it names outright.
    #[test]
    fn pick_is_strict_over_core_members_and_omit_is_not() {
        let ctx = Loader::new().core();
        let picked = apply(
            &entity(),
            &Repr {
                pick: Some(parse_projection("id", &ctx).expect("pick")),
                ..Repr::default()
            },
        );
        assert_eq!(picked["id"], "urn:ngsi-ld:E:1");
        assert!(picked.get("type").is_none(), "type was not picked");
        assert!(
            picked.get("https://example.org/temperature").is_none(),
            "an unpicked attribute must not survive"
        );
        let omitted = apply(
            &entity(),
            &Repr {
                omit: Some(parse_projection("scope", &ctx).expect("omit")),
                ..Repr::default()
            },
        );
        assert_eq!(omitted["type"], "T", "omit leaves the rest of the entity");
    }

    /// 4.5.16.1/4.5.16.2/4.5.16.3: geometry selection (default instance,
    /// datasetId-narrowed single, invalid value -> null) and the
    /// RFC 7946 3.1.6 on the way out: a clockwise exterior turns counterclockwise, a
    /// counterclockwise hole clockwise; a geometry across the antimeridian stays as stored.
    #[test]
    fn geojson_rings_are_wound_and_the_antimeridian_is_not_cut() {
        let cw = json!([[0, 0], [0, 1], [1, 1], [1, 0], [0, 0]]);
        let ccw = json!([[0, 0], [1, 0], [1, 1], [0, 1], [0, 0]]);
        let hole_ccw = json!([[0.2, 0.2], [0.8, 0.2], [0.8, 0.8], [0.2, 0.8], [0.2, 0.2]]);
        let hole_cw = json!([[0.2, 0.2], [0.2, 0.8], [0.8, 0.8], [0.8, 0.2], [0.2, 0.2]]);
        let polygon = |rings: Value| json!({"type": "Polygon", "coordinates": rings});
        assert_eq!(
            rewound(polygon(json!([cw, hole_ccw]))),
            polygon(json!([ccw, hole_cw]))
        );
        assert_eq!(rewound(polygon(json!([ccw]))), polygon(json!([ccw])));
        let multi = json!({"type": "MultiPolygon", "coordinates": [[cw]]});
        assert_eq!(rewound(multi)["coordinates"], json!([[ccw]]));
        let across = polygon(json!([[
            [179, 0],
            [-179, 0],
            [-179, 1],
            [179, 1],
            [179, 0]
        ]]));
        let entity = json!({"location": {"type": "GeoProperty", "value": across}});
        assert_eq!(geometry_of(&entity, None), across);
        // the default instance, not the first, of a multi-instance GeoProperty
        let two = json!({"place": [
            {"type": "GeoProperty", "datasetId": "urn:a", "value": {"type": "Point", "coordinates": [1, 1]}},
            {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [2, 2]}}]});
        assert_eq!(
            geometry_of(&two, Some("place"))["coordinates"],
            json!([2, 2])
        );
    }

    /// Feature/FeatureCollection shapes.
    #[test]
    fn geojson_feature_selection_and_shape() {
        use super::{to_geojson_collection, to_geojson_feature};
        use serde_json::Value;
        let entity = json!({
            "id": "urn:ngsi-ld:V:1", "type": "Vehicle",
            "location": [
                {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [9.0, 9.0]},
                 "datasetId": "urn:ngsi-ld:Dataset:gps"},
                {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [1.0, 2.0]}}
            ],
            "speed": {"type": "Property", "value": 5}
        });
        let f = to_geojson_feature(entity.clone(), None);
        assert_eq!(f["type"], "Feature");
        assert_eq!(f["id"], "urn:ngsi-ld:V:1");
        // default instance (no datasetId) wins over the first array element
        assert_eq!(
            f["geometry"],
            json!({"type": "Point", "coordinates": [1.0, 2.0]})
        );
        assert_eq!(f["properties"]["type"], "Vehicle");
        assert!(
            f["properties"].get("id").is_none(),
            "id only at Feature level"
        );
        assert!(f["properties"].get("speed").is_some());

        // geometryProperty naming a non-geometry Property -> null geometry
        let f2 = to_geojson_feature(entity.clone(), Some(&"speed".to_string()));
        assert_eq!(f2["geometry"], Value::Null);
        // absent GeoProperty -> null geometry
        let f3 = to_geojson_feature(entity.clone(), Some(&"missing".to_string()));
        assert_eq!(f3["geometry"], Value::Null);

        // 4.5.17.1: simplified multi-instance GeoProperty = dataset map;
        // the "@none" (default) entry is the geometry
        let simplified = json!({
            "id": "urn:ngsi-ld:V:2", "type": "Vehicle",
            "location": {"dataset": {
                "urn:ngsi-ld:Dataset:gps": {"type": "Point", "coordinates": [9.0, 9.0]},
                "@none": {"type": "Point", "coordinates": [3.0, 4.0]}
            }},
            "speed": 5
        });
        let fs = to_geojson_feature(simplified, None);
        assert_eq!(
            fs["geometry"],
            json!({"type": "Point", "coordinates": [3.0, 4.0]})
        );
        assert_eq!(fs["properties"]["speed"], 5);

        let fc = to_geojson_collection(vec![entity], None);
        assert_eq!(fc["type"], "FeatureCollection");
        assert_eq!(fc["features"].as_array().map(Vec::len), Some(1));
        assert!(
            fc["features"][0].get("@context").is_none(),
            "no per-Feature @context"
        );
        // Table 5.2.30-1: "In the case that no matches are found, features
        // will be an empty array"
        let empty = to_geojson_collection(vec![], None);
        assert_eq!(empty["type"], "FeatureCollection");
        assert_eq!(empty["features"], json!([]));
    }
}

#[cfg(test)]
mod type_rules_memo_tests {
    use super::type_rules;
    use crate::policy::{Filter, Rule};
    use std::collections::BTreeMap;

    fn filter(t: &str, attr: &str) -> Filter {
        Filter {
            rules: vec![Rule {
                types: vec![t.to_owned()],
                attributes: vec![attr.to_owned()],
                ..Rule::default()
            }],
            ..Filter::default()
        }
    }

    const DEFAULT: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

    /// The memo answers only for the rules it was made from: alternating
    /// filters (two views, two subjects) each get their own expansion.
    #[test]
    fn the_memo_answers_only_for_its_own_rules() {
        let (car, user) = (filter("Car", "speed"), filter("User", "name"));
        for f in [&car, &user, &car, &car, &user] {
            let got = type_rules(f).unwrap();
            let r = &f.rules[0];
            assert_eq!(got.rules[0].types, vec![format!("{DEFAULT}{}", r.types[0])]);
            assert_eq!(
                got.rules[0].attributes,
                vec![format!("{DEFAULT}{}", r.attributes[0])]
            );
        }
        let omitting = Filter {
            omit_by_type: BTreeMap::from([("Car".to_owned(), vec!["speed".to_owned()])]),
            ..car.clone()
        };
        let got = type_rules(&omitting).unwrap();
        assert_eq!(
            got.omit_by_type.get(&format!("{DEFAULT}Car")),
            Some(&vec![format!("{DEFAULT}speed")])
        );
        assert!(type_rules(&car).unwrap().omit_by_type.is_empty());
        assert!(type_rules(&Filter::default()).is_none());
    }

    /// T-3697: the reach memo answers for the one `Arc` it holds. The same
    /// `Arc` gets the same reach back; another `Arc`, even one with equal
    /// content, or a narrowing that was dropped and rebuilt, gets its own,
    /// so a reach never outlives the Filter it was made from.
    #[test]
    fn the_reach_memo_answers_only_for_its_own_arc() {
        use std::sync::Arc;
        let reach = |f: &Arc<Filter>| {
            let mut r = super::Repr::default();
            super::narrow_repr(&mut r, f);
            r
        };
        let car = Arc::new(filter("Car", "speed"));
        let user = Arc::new(filter("User", "name"));
        let (a, b) = (reach(&car), reach(&car));
        let (ra, rb) = (a.policy_reach.unwrap(), b.policy_reach.unwrap());
        assert!(Arc::ptr_eq(&ra, &rb), "the same Arc reuses its reach");
        for (f, t) in [(&user, "User"), (&car, "Car"), (&user, "User")] {
            let got = reach(f).policy_reach.unwrap();
            let rules = got.rules.as_ref().unwrap();
            assert_eq!(rules.rules[0].types, vec![format!("{DEFAULT}{t}")]);
        }
        let twin = Arc::new(filter("Car", "speed"));
        let own = reach(&twin).policy_reach.unwrap();
        assert!(
            !Arc::ptr_eq(&own, &ra),
            "an equal Filter in another Arc builds its own"
        );
        assert_eq!(own.rules, ra.rules);
        let narrowed = Arc::new(Filter {
            q: Some(Box::new(antares_ql::parse_q("speed>1").expect("q"))),
            ..Filter::default()
        });
        assert!(reach(&narrowed).policy_reach.unwrap().q.is_some());
        let open = crate::policy::unnarrowed();
        let none = reach(&open);
        assert!(none.policy_reach.is_none() && none.policy_types.is_none());
        assert!(super::narrowing(&car).policy_reach.unwrap().rules.is_some());
    }
}

#[cfg(test)]
mod t3932_projection_edges {
    use super::*;
    use antares_jsonld::Loader;

    /// T-3932: the projection parser splits at the brace it found and counts
    /// depth without overflow; a malformed or multi-byte term is a 400, never
    /// a panic.
    #[test]
    fn malformed_and_multibyte_projections_are_refused() {
        let ctx = Loader::new().core();
        let deep = parse_projection("a{b{c}}", &ctx).expect("nested");
        assert_eq!(proj_depth(&deep), 2);
        assert_eq!(deep[0].raw, "a");
        for bad in ["a{b", "a}", "a{}", "{", "}", ",", "a,,b", "é{ü|ö}", "a{é}"] {
            assert!(parse_projection(bad, &ctx).is_err(), "{bad} is refused");
        }
    }
}
