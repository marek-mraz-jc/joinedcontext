// SPDX-License-Identifier: EUPL-1.2
//! The rule document of one Tenant (README section 2), parsed and compiled
//! once at load: every name expanded, every query parsed, every pattern
//! compiled, so a decision only compares.

use antares_api::policy::metadata::Metadata;
use antares_api::policy::{Rule, TimeBound, Window};
use antares_ql::geo::GeoQuery;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    policies: Vec<PolicyDoc>,
    /// The policies each listed view is decided by (README section 2).
    #[serde(default)]
    views: HashMap<String, Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PolicyDoc {
    id: String,
    #[serde(default)]
    effect: Effect,
    #[serde(default)]
    assigner: Option<String>,
    assignee: Assignee,
    operations: Vec<String>,
    #[serde(default)]
    validity: Option<Validity>,
    #[serde(default)]
    information: Vec<Information>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    scope_q: Option<String>,
    #[serde(default)]
    geo_q: Option<String>,
    #[serde(default)]
    temporal_q: Option<String>,
}

#[derive(Deserialize, Default, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Effect {
    #[default]
    Permit,
    Prohibit,
}

/// Who a policy is about.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Assignee {
    /// What `id` names.
    pub kind: Kind,
    /// The user, group, role, service account or DID; empty for an `organization`, which
    /// names none (ADR-N-067 2.3).
    #[serde(default)]
    pub id: String,
}

/// The kinds of assignee (platform `PrincipalKind`).
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// `x-jc-subject`.
    User,
    /// One of `x-jc-groups`.
    Group,
    /// One of `x-jc-roles`.
    Role,
    /// `x-jc-client`.
    ServiceAccount,
    /// No header carries one yet: matches nobody.
    Did,
    /// `x-jc-member: true`: a member of the Organization of the Endpoint's project, as the
    /// gateway verified it from the token's `jc_org` (ADR-N-067 2.3). Carries no id.
    Organization,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Validity {
    #[serde(default)]
    from: Option<DateTime<Utc>>,
    #[serde(default)]
    to: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Information {
    #[serde(default)]
    entities: Vec<SelectorDoc>,
    #[serde(default)]
    property_names: Vec<String>,
    #[serde(default)]
    relationship_names: Vec<String>,
    /// Per Attribute or `*`, what the grant lets a caller do with it
    /// (platform ADR-N-066, DM-147): it only narrows the operations.
    #[serde(default)]
    attributes: BTreeMap<String, Capabilities>,
}

/// One Attribute's capabilities (platform `AttributeCapabilities`); a field
/// left unset takes the `*` entry's, then the grant's own behaviour.
#[derive(Deserialize, Clone, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Capabilities {
    #[serde(default)]
    read: Option<bool>,
    #[serde(default)]
    history: Option<bool>,
    /// `read`, `keep`: the gateway decides both (DM-149, DM-150).
    #[serde(default)]
    versions: Option<Vec<Versions>>,
    /// The members the gateway passes (DM-139): checked here, enforced there.
    #[serde(default)]
    metadata: Option<Vec<String>>,
    #[serde(default)]
    write: Option<Vec<Write>>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
enum Versions {
    Read,
    Keep,
}

/// A write kind of ADR-N-066 2.1.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
enum Write {
    Update,
    Append,
    Delete,
}

/// The metadata members a grant may pass (platform `MetadataMember`).
const METADATA: [&str; 9] = [
    "observedAt",
    "unitCode",
    "createdAt",
    "modifiedAt",
    "modifiedBy.organization",
    "modifiedBy.actorKind",
    "modifiedBy.actor",
    "provenance",
    "reason",
];

impl Capabilities {
    /// Why these capabilities are refused: a list naming a value twice, or
    /// a metadata member the platform does not know.
    fn refusal(&self) -> Option<String> {
        fn repeats<T: Ord>(list: Option<&Vec<T>>) -> bool {
            list.is_some_and(|l| l.iter().collect::<BTreeSet<_>>().len() != l.len())
        }
        if repeats(self.versions.as_ref())
            || repeats(self.write.as_ref())
            || repeats(self.metadata.as_ref())
        {
            return Some("a capability list names each value once".into());
        }
        self.metadata
            .iter()
            .flatten()
            .find(|m| !METADATA.contains(&m.as_str()))
            .map(|m| format!("metadata {m:?} is none of {}", METADATA.join(", ")))
    }
}

/// What a clause needs of an Attribute's capabilities (ADR-N-066 2.1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Need {
    /// A type or Attribute list (5.7.5 to 5.7.10): the shape, never data,
    /// narrowed by the entry's lists alone.
    Schema,
    /// The current value: retrievals, queries, subscriptions.
    Read,
    /// The history: 5.7.3, 5.7.4 and an Entity Map over it.
    History,
    /// A write needing every kind it names.
    Write(bool, bool, bool),
}

impl Need {
    fn of(clause: &str) -> Self {
        const U: Need = Need::Write(true, false, false);
        const A: Need = Need::Write(false, true, false);
        const D: Need = Need::Write(false, false, true);
        match clause {
            "5.7.5" | "5.7.6" | "5.7.7" | "5.7.8" | "5.7.9" | "5.7.10" => Self::Schema,
            "5.7.3" | "5.7.4" | "5.14.5" => Self::History,
            "5.6.2" | "5.6.4" | "5.6.9" | "5.6.14" | "5.6.17" | "5.6.19" | "5.6.20" => U,
            "5.6.1" | "5.6.3" | "5.6.7" | "5.6.12" => A,
            "5.6.5" | "5.6.6" | "5.6.10" | "5.6.13" | "5.6.15" | "5.6.16" | "5.6.21" => D,
            // an upsert may add or change; any other write, a replace among them, may do all
            "5.6.8" | "5.6.11" => Self::Write(true, true, false),
            c if c.starts_with("5.6.") => Self::Write(true, true, true),
            _ => Self::Read,
        }
    }

    /// Whether `caps` (the Attribute's own, then `*`'s) give this need.
    fn given(self, own: Option<&Capabilities>, every: Option<&Capabilities>) -> bool {
        let pick = |f: fn(&Capabilities) -> Option<bool>| {
            own.and_then(f)
                .or_else(|| every.and_then(f))
                .unwrap_or(true)
        };
        match self {
            Self::Schema => true,
            Self::Read => pick(|c| c.read),
            Self::History => pick(|c| c.history),
            Self::Write(u, a, d) => {
                let kinds = own
                    .and_then(|c| c.write.as_ref())
                    .or_else(|| every.and_then(|c| c.write.as_ref()));
                kinds.is_none_or(|k| {
                    (!u || k.contains(&Write::Update))
                        && (!a || k.contains(&Write::Append))
                        && (!d || k.contains(&Write::Delete))
                })
            }
        }
    }
}

impl Information {
    /// This entry as `need` sees it: its Attribute lists narrowed to the
    /// Attributes whose capabilities give it; `None` when none is left, so
    /// the entry grants nothing for `need` (deny by default). An entry
    /// without `attributes` is today's, whole.
    fn narrowed(&self, need: Need) -> Result<Option<Self>, String> {
        if self.attributes.is_empty() || need == Need::Schema {
            return Ok(Some(Self {
                attributes: BTreeMap::new(),
                ..self.clone()
            }));
        }
        let every = self.attributes.get("*");
        let keep = |names: &[String]| -> Vec<String> {
            names
                .iter()
                .filter(|n| need.given(self.attributes.get(n.as_str()), every))
                .cloned()
                .collect()
        };
        let (property_names, relationship_names) =
            if self.property_names.is_empty() && self.relationship_names.is_empty() {
                // every Attribute: `*` decides, a named key adds or takes away
                let named = self.attributes.keys().filter(|k| *k != "*");
                if need.given(None, every) {
                    if let Some(k) = named
                        .clone()
                        .find(|k| !need.given(self.attributes.get(k.as_str()), every))
                    {
                        return Err(format!(
                            "attribute {k:?} is narrowed under an entry that lists no Attribute: \
                         list propertyNames or relationshipNames"
                        ));
                    }
                    (Vec::new(), Vec::new())
                } else {
                    let keys: Vec<String> = named.cloned().collect();
                    let given = keep(&keys);
                    if given.is_empty() {
                        return Ok(None);
                    }
                    (given, Vec::new())
                }
            } else {
                let (p, r) = (keep(&self.property_names), keep(&self.relationship_names));
                if p.is_empty() && r.is_empty() {
                    return Ok(None);
                }
                (p, r)
            };
        Ok(Some(Self {
            entities: self.entities.clone(),
            property_names,
            relationship_names,
            attributes: BTreeMap::new(),
        }))
    }
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SelectorDoc {
    #[serde(rename = "type")]
    entity_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    id_pattern: Option<String>,
}

/// One compiled policy.
#[derive(Debug, Clone)]
pub struct Policy {
    /// Its `id`, for a refusal's reason and the access surface.
    pub id: String,
    /// A prohibition rather than a permission.
    pub prohibit: bool,
    /// Who it is about.
    pub assignee: Assignee,
    /// The CIM 009 clauses it speaks about.
    pub clauses: BTreeSet<String>,
    /// `[from, to)`; an open end is unbounded.
    pub validity: (Option<DateTime<Utc>>, Option<DateTime<Utc>>),
    /// One rule per selector: its type, id or pattern and the Attributes its
    /// information shows, with the policy's conditions. Empty = every type.
    pub selectors: Vec<Rule>,
    /// The policy's conditions as one rule without type over every
    /// Attribute, for a policy that names no selector: [`Rule::types`] is
    /// filled per operation.
    pub conditions: Rule,
    /// The policy as written, for the access surface.
    pub source: Source,
    /// The metadata members it passes (a permission) or takes back (a
    /// prohibition) per Attribute (platform ADR-N-066 2.4).
    pub metadata: Metadata,
}

/// A policy as its document wrote it (README section 4): names before
/// expansion and conditions as strings, so the access surface shows the
/// caller the words their grant was given in. Never read by a decision.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Source {
    /// The party that granted it.
    pub assigner: Option<String>,
    /// One per `information.entities` member, in document order.
    pub selectors: Vec<Selector>,
    /// `q`.
    pub q: Option<String>,
    /// `scopeQ`.
    pub scope_q: Option<String>,
    /// `geoQ`.
    pub geo_q: Option<String>,
    /// `temporalQ`.
    pub temporal_q: Option<String>,
}

/// One Entity selector as written, with its information's Attributes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selector {
    /// `type`.
    pub entity_type: String,
    /// `id`.
    pub id: Option<String>,
    /// `idPattern`.
    pub id_pattern: Option<String>,
    /// `propertyNames` and `relationshipNames`, sorted; empty = the whole
    /// Entity.
    pub attributes: Vec<String>,
}

impl Policy {
    /// Whether the policy is in force at `now`.
    #[must_use]
    pub fn in_force(&self, now: DateTime<Utc>) -> bool {
        self.validity.0.is_none_or(|from| now >= from) && self.validity.1.is_none_or(|to| now < to)
    }

    /// Whether the policy sets a condition on the Entity or its history.
    #[must_use]
    pub fn is_conditional(&self) -> bool {
        self.conditions.is_conditional() || !self.conditions.temporal.is_empty()
    }
}

/// One Tenant's compiled document.
#[derive(Debug, Clone, Default)]
pub struct Table {
    /// In document order.
    pub policies: Vec<Policy>,
    /// Per listed view, the table of its policies alone, in document order.
    pub views: HashMap<String, Arc<Table>>,
}

impl Table {
    /// The table a request through `view` is decided by: the view's own
    /// when the document lists it, this one otherwise.
    #[must_use]
    pub fn through(&self, view: Option<&str>) -> &Table {
        view.and_then(|v| self.views.get(v)).map_or(self, |t| t)
    }
}

/// The core `@context` every name is expanded by: the broker hands the
/// engine expanded names, and an IRI expands to itself.
pub fn context() -> &'static antares_jsonld::Context {
    static CORE: std::sync::OnceLock<antares_jsonld::Context> = std::sync::OnceLock::new();
    CORE.get_or_init(antares_jsonld::core_context)
}

/// Compile `doc`, or say why it is refused. `now` resolves nothing: a
/// relative `temporalQ` stays relative ([`TimeBound::Ago`]).
///
/// # Errors
/// The first member that does not parse, named by its policy.
pub fn compile(doc: &serde_json::Value) -> Result<Table, String> {
    let doc: Document =
        Document::deserialize(doc).map_err(|e| format!("the rule document: {e}"))?;
    let mut seen = BTreeSet::new();
    let mut policies = Vec::with_capacity(doc.policies.len());
    for p in doc.policies {
        if !seen.insert(p.id.clone()) {
            return Err(format!("policy {:?} is named twice", p.id));
        }
        policies.extend(policy(p)?);
    }
    let mut views = HashMap::with_capacity(doc.views.len());
    for (view, ids) in doc.views {
        let mut listed = BTreeSet::new();
        for id in &ids {
            if !seen.contains(id) {
                return Err(format!(
                    "view {view:?} names policy {id:?}, which is not in the document"
                ));
            }
            if !listed.insert(id.as_str()) {
                return Err(format!("view {view:?} names policy {id:?} twice"));
            }
        }
        let own = policies
            .iter()
            .filter(|p| listed.contains(p.id.as_str()))
            .cloned()
            .collect();
        views.insert(
            view,
            Arc::new(Table {
                policies: own,
                views: HashMap::new(),
            }),
        );
    }
    Ok(Table { policies, views })
}

fn policy(p: PolicyDoc) -> Result<Vec<Policy>, String> {
    let id = p.id;
    let why = |what: String| format!("policy {id:?}: {what}");
    if id.is_empty() {
        return Err("a policy has an empty id".into());
    }
    if p.assignee.kind == Kind::Organization && !p.assignee.id.is_empty() {
        // An id the engine would not compare would widen the grant to every member.
        return Err(why("an organization assignee names no id".into()));
    }
    if p.operations.is_empty() {
        return Err(why("names no operation".into()));
    }
    let q =
        p.q.as_deref()
            .map(|q| {
                let node = antares_ql::parse_q(q).map_err(|e| why(format!("q {q:?}: {e}")))?;
                if node.has_linked_paths() {
                    return Err(why(format!(
                        "q {q:?} reads a linked Entity, which a rule cannot follow"
                    )));
                }
                Ok(node)
            })
            .transpose()?;
    let geo = p
        .geo_q
        .as_deref()
        .map(|g| geo_query(g).map_err(|e| why(format!("geoQ {g:?}: {e}"))))
        .transpose()?;
    let temporal = p
        .temporal_q
        .as_deref()
        .map(|t| window(t).map_err(|e| why(format!("temporalQ {t:?}: {e}"))))
        .transpose()?;
    // a granted path covers its subtree (platform R13, R30): `/a/b` is the
    // scope query `/a/b/#`, which matches `/a/b` and below, never `/a/bc`
    let scope_q = match p.scope_q.as_deref() {
        None => None,
        Some(s) => {
            let tree: Vec<String> = scope_paths(s).map(|p| format!("{p}/#")).collect();
            if tree.is_empty() {
                return Err(why(format!("scopeQ {s:?} names no scope path")));
            }
            Some(tree.join(","))
        }
    };
    let (from, to) = p.validity.map_or((None, None), |v| (v.from, v.to));
    let mut source = Source {
        assigner: p.assigner,
        selectors: Vec::new(),
        q: p.q,
        scope_q: p.scope_q,
        geo_q: p.geo_q,
        temporal_q: p.temporal_q,
    };
    let conditions = Rule {
        every_attribute: true,
        q: q.map(Box::new),
        scope_q,
        geo_q: geo.map(Box::new),
        temporal: temporal.into_iter().collect(),
        ..Rule::default()
    };
    let prohibit = p.effect == Effect::Prohibit;
    capabilities_checked(&p.information, &why)?;
    let metadata = members(&p.information, &p.assignee, prohibit);
    if p.information.iter().all(|i| i.attributes.is_empty()) {
        let (selectors, written) = rules(p.information, &conditions, &why)?;
        source.selectors = written;
        return Ok(vec![Policy {
            id,
            prohibit,
            assignee: p.assignee,
            clauses: p.operations.into_iter().collect(),
            validity: (from, to),
            selectors,
            conditions,
            source,
            metadata,
        }]);
    }
    let parts = Parts {
        id: id.clone(),
        prohibit,
        assignee: p.assignee,
        validity: (from, to),
        conditions,
        source,
        metadata,
    };
    split(&parts, p.operations, &p.information, &why)
}

/// What every policy split from one written policy shares.
struct Parts {
    id: String,
    prohibit: bool,
    assignee: Assignee,
    validity: (Option<DateTime<Utc>>, Option<DateTime<Utc>>),
    conditions: Rule,
    source: Source,
    metadata: Metadata,
}

/// ADR-N-066 2.1: one policy per need of its clauses, each with the
/// Attributes whose capabilities give that need; a need no Attribute is
/// given grants (or prohibits) nothing. All keep the policy's id, so a view
/// listing it lists them all.
fn split(
    parts: &Parts,
    operations: Vec<String>,
    written: &[Information],
    why: &dyn Fn(String) -> String,
) -> Result<Vec<Policy>, String> {
    let mut by_need: BTreeMap<Need, BTreeSet<String>> = BTreeMap::new();
    for clause in operations {
        by_need.entry(Need::of(&clause)).or_default().insert(clause);
    }
    let mut out = Vec::with_capacity(by_need.len());
    for (need, clauses) in by_need {
        let mut information = Vec::with_capacity(written.len());
        for info in written {
            if let Some(narrowed) = info.narrowed(need).map_err(why)? {
                information.push(narrowed);
            }
        }
        if information.is_empty() {
            continue;
        }
        let (selectors, written_selectors) = rules(information, &parts.conditions, why)?;
        out.push(Policy {
            id: parts.id.clone(),
            prohibit: parts.prohibit,
            assignee: parts.assignee.clone(),
            clauses,
            validity: parts.validity,
            selectors,
            conditions: parts.conditions.clone(),
            source: Source {
                selectors: written_selectors,
                ..parts.source.clone()
            },
            metadata: parts.metadata.clone(),
        });
    }
    Ok(out)
}

/// Each `attributes` key is `*` or an Attribute its entry lists (any name
/// when it lists none), and each capability list is well formed.
/// The members the audience gets for an Attribute its entry and `*` leave
/// unset (ADR-N-066 2.4).
fn defaults(public: bool) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = ["observedAt", "unitCode", "modifiedAt"]
        .map(String::from)
        .into();
    if !public {
        out.extend(["modifiedBy.organization", "modifiedBy.actorKind"].map(String::from));
    }
    out
}

/// One policy's metadata members (ADR-N-066 2.4), names expanded against
/// [`context`]. A permission passes per Attribute what its entry lists,
/// else the `*` entry's list, else the audience defaults; one without any
/// `attributes` block passes every member, as before the block. A grant to
/// `role: public` never passes `modifiedBy.actor` or `provenance` (DM-140).
/// A prohibition takes back only what its entries list.
fn members(information: &[Information], assignee: &Assignee, prohibit: bool) -> Metadata {
    let mut named: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut star: Option<BTreeSet<String>> = None;
    for (name, listed) in information
        .iter()
        .flat_map(|i| &i.attributes)
        .filter_map(|(name, caps)| Some((name, caps.metadata.as_ref()?)))
    {
        let listed = listed.iter().cloned();
        if name == "*" {
            star.get_or_insert_with(BTreeSet::new).extend(listed);
        } else {
            named
                .entry(context().expand_key(name))
                .or_default()
                .extend(listed);
        }
    }
    if prohibit {
        return Metadata {
            named,
            rest: star.unwrap_or_default(),
        };
    }
    let public = assignee.kind == Kind::Role && assignee.id == "public";
    let blocked = information.iter().any(|i| !i.attributes.is_empty());
    let mut out = Metadata {
        named,
        rest: match (blocked, star) {
            (_, Some(star)) => star,
            (true, None) => defaults(public),
            (false, None) => METADATA.map(String::from).into(),
        },
    };
    if public {
        for m in out.named.values_mut().chain([&mut out.rest]) {
            m.remove("modifiedBy.actor");
            m.remove("provenance");
        }
    }
    out
}

fn capabilities_checked(
    information: &[Information],
    why: &dyn Fn(String) -> String,
) -> Result<(), String> {
    for info in information {
        let any = info.property_names.is_empty() && info.relationship_names.is_empty();
        for (name, caps) in &info.attributes {
            if name != "*"
                && (name.trim().is_empty()
                    || !(any
                        || info.property_names.contains(name)
                        || info.relationship_names.contains(name)))
            {
                return Err(why(format!(
                    "attribute {name:?} is `*` or one propertyNames or relationshipNames lists"
                )));
            }
            if let Some(reason) = caps.refusal() {
                return Err(why(format!("attribute {name:?}: {reason}")));
            }
        }
    }
    Ok(())
}

/// One rule per Entity selector of `information`, with its information's
/// Attributes and the policy's `conditions`, and the selectors as written.
fn rules(
    information: Vec<Information>,
    conditions: &Rule,
    why: &dyn Fn(String) -> String,
) -> Result<(Vec<Rule>, Vec<Selector>), String> {
    let ctx = context();
    let mut selectors = Vec::new();
    let mut written_selectors = Vec::new();
    for info in information {
        let attributes: Vec<String> = info
            .property_names
            .iter()
            .chain(&info.relationship_names)
            .map(|n| ctx.expand_key(n))
            .collect();
        if info.entities.is_empty() {
            return Err(why("an information entry selects no Entity".into()));
        }
        let mut written: Vec<String> = info
            .property_names
            .iter()
            .chain(&info.relationship_names)
            .cloned()
            .collect();
        written.sort();
        written.dedup();
        for s in info.entities {
            if s.id.is_some() && s.id_pattern.is_some() {
                return Err(why(format!(
                    "the selector of {:?} names both id and idPattern",
                    s.entity_type
                )));
            }
            if let Some(p) = s.id_pattern.as_deref() {
                antares_ql::regex::compile(&format!("^(?:{p})$"))
                    .map_err(|e| why(format!("idPattern {p:?}: {e}")))?;
            }
            written_selectors.push(Selector {
                entity_type: s.entity_type.clone(),
                id: s.id.clone(),
                id_pattern: s.id_pattern.clone(),
                attributes: written.clone(),
            });
            selectors.push(Rule {
                types: vec![ctx.expand_key(&s.entity_type)],
                ids: s.id.into_iter().collect(),
                id_patterns: s.id_pattern.into_iter().collect(),
                // no list is the whole Entity (EP-16, map K9)
                every_attribute: attributes.is_empty(),
                attributes: attributes.clone(),
                ..conditions.clone()
            });
        }
    }
    Ok((selectors, written_selectors))
}

/// `georel=…;geometry=…;coordinates=…[;geoproperty=…]` as the broker parses
/// a request's geoquery.
fn geo_query(raw: &str) -> Result<GeoQuery, String> {
    let params: HashMap<String, String> = raw
        .split(';')
        .filter_map(|part| part.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    GeoQuery::from_params(&params)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no georel".to_owned())
}

/// `timerel=before|after|between;timeAt=…[;endTimeAt=…]`, each instant RFC
/// 3339 or a duration before now (`P-1D`, `-P1D`).
fn window(raw: &str) -> Result<Window, String> {
    let (mut rel, mut at, mut end) = (None, None, None);
    for part in raw.split([';', '&']) {
        match part.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
            Some(("timerel", v)) => rel = Some(v),
            Some(("timeAt", v)) => at = Some(bound(v)?),
            Some(("endTimeAt", v)) => end = Some(bound(v)?),
            _ => return Err(format!("{part:?} is not timerel, timeAt or endTimeAt")),
        }
    }
    let need = |b: Option<TimeBound>, name: &str| b.ok_or_else(|| format!("no {name}"));
    match rel {
        Some("after") => Ok(Window {
            from: Some(need(at, "timeAt")?),
            to: None,
        }),
        Some("before") => Ok(Window {
            from: None,
            to: Some(need(at, "timeAt")?),
        }),
        Some("between") => Ok(Window {
            from: Some(need(at, "timeAt")?),
            to: Some(need(end, "endTimeAt")?),
        }),
        other => Err(format!("timerel {other:?} is not before, after or between")),
    }
}

fn bound(raw: &str) -> Result<TimeBound, String> {
    if let Ok(at) = DateTime::parse_from_rfc3339(raw) {
        return Ok(TimeBound::At(at.with_timezone(&Utc)));
    }
    match duration(raw) {
        Some(d) if d < Duration::zero() => Ok(TimeBound::Ago(d.abs())),
        // zero is now, which no window reaches back to
        Some(_) => Err(format!("{raw:?} does not lie in the past")),
        None => Err(format!(
            "{raw:?} is neither an instant nor a duration in weeks, days, hours, minutes and seconds"
        )),
    }
}

/// An ISO 8601 duration as a signed offset (`P-1D` and `-P1D` are a day
/// ago); months and years have no fixed length and are refused.
fn duration(raw: &str) -> Option<Duration> {
    let (mut sign, rest) = match raw.strip_prefix('-') {
        Some(rest) => (-1i32, rest),
        None => (1i32, raw),
    };
    let rest = rest.strip_prefix('P')?;
    let (date, time) = rest.split_once('T').unwrap_or((rest, ""));
    let mut total = Duration::zero();
    let mut digits = String::new();
    let mut units_seen = false;
    for (part, units) in [(date, "WD"), (time, "HMS")] {
        for c in part.chars() {
            match c {
                '-' if digits.is_empty() => sign = sign.checked_neg()?,
                '0'..='9' => digits.push(c),
                u if units.contains(u) => {
                    let n: i64 = digits.parse().ok()?;
                    digits.clear();
                    units_seen = true;
                    total = total.checked_add(&match u {
                        'W' => Duration::try_weeks(n)?,
                        'D' => Duration::try_days(n)?,
                        'H' => Duration::try_hours(n)?,
                        'M' => Duration::try_minutes(n)?,
                        _ => Duration::try_seconds(n)?,
                    })?;
                }
                _ => return None,
            }
        }
    }
    (digits.is_empty() && units_seen).then(|| total.checked_mul(sign))?
}

/// The scope paths of a `scopeQ`, whose operators separate them; a trailing
/// `/#` or `/` names the same subtree.
pub fn scope_paths(scope_q: &str) -> impl Iterator<Item = &str> {
    scope_q
        .split(['(', ')', ';', '|', ','])
        .map(str::trim)
        .filter(|p| p.starts_with('/'))
        .map(|p| p.trim_end_matches("/#").trim_end_matches('/'))
        .filter(|p| !p.is_empty())
}

/// Whether the granted path covers `scope` as a path: `/a/b` covers `/a/b`
/// and `/a/b/c`, never `/a/bc`.
#[must_use]
pub fn covers(granted: &str, scope: &str) -> bool {
    scope == granted
        || scope
            .strip_prefix(granted)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(p: &serde_json::Value) -> Result<Policy, String> {
        compile(&json!({ "policies": [p] }))?
            .policies
            .into_iter()
            .next()
            .ok_or_else(|| "no policy".to_owned())
    }

    fn base() -> serde_json::Value {
        json!({"id": "p", "assignee": {"kind": "role", "id": "public"}, "operations": ["5.7.2"]})
    }

    fn with(k: &str, v: serde_json::Value) -> serde_json::Value {
        let mut p = base();
        if let Some(o) = p.as_object_mut() {
            o.insert(k.to_owned(), v);
        }
        p
    }

    #[test]
    fn a_policy_compiles_one_rule_per_selector_with_its_conditions() -> Result<(), String> {
        let p = one(&with(
            "information",
            json!([{"entities": [{"type": "Car"}, {"type": "Bus", "id": "urn:ngsi-ld:Bus:1"}],
                    "propertyNames": ["speed"], "relationshipNames": ["ownedBy"]}]),
        ))?;
        let ctx = context();
        assert_eq!(p.selectors.len(), 2);
        let car = p.selectors.first().ok_or("no Car selector")?;
        let bus = p.selectors.get(1).ok_or("no Bus selector")?;
        assert_eq!(car.types, [ctx.expand_key("Car")]);
        assert_eq!(bus.ids, ["urn:ngsi-ld:Bus:1"]);
        assert_eq!(
            car.attributes,
            [ctx.expand_key("speed"), ctx.expand_key("ownedBy")]
        );
        assert!(!p.prohibit && !p.is_conditional());
        Ok(())
    }

    /// The compiled policies of one written policy, keyed by their clauses.
    type Compiled = Vec<(Vec<String>, Vec<Rule>)>;

    fn split(p: &serde_json::Value) -> Result<Compiled, String> {
        Ok(compile(&json!({ "policies": [p] }))?
            .policies
            .into_iter()
            .map(|p| (p.clauses.into_iter().collect(), p.selectors))
            .collect())
    }

    fn air(attributes: &serde_json::Value, operations: &serde_json::Value) -> serde_json::Value {
        json!({"id": "p", "assignee": {"kind": "role", "id": "public"},
               "operations": operations,
               "information": [{"entities": [{"type": "Air"}],
                                "propertyNames": ["name", "pm10"],
                                "attributes": attributes}]})
    }

    fn names(rules: &[Rule]) -> Vec<String> {
        let ctx = context();
        let (name, pm10) = (ctx.expand_key("name"), ctx.expand_key("pm10"));
        rules
            .iter()
            .flat_map(|r| &r.attributes)
            .map(|a| {
                if *a == name {
                    "name".to_owned()
                } else if *a == pm10 {
                    "pm10".to_owned()
                } else {
                    a.clone()
                }
            })
            .collect()
    }

    #[test]
    fn history_of_one_attribute_leaves_current_reads_whole() -> Result<(), String> {
        let got = split(&air(
            &json!({"*": {"history": false}, "name": {"history": true}}),
            &json!(["5.7.1", "5.7.2", "5.7.3", "5.7.4"]),
        ))?;
        let [(reads, read_rules), (history, history_rules)] = got.as_slice() else {
            return Err(format!("{} policies, not 2", got.len()));
        };
        assert_eq!(reads, &["5.7.1", "5.7.2"]);
        assert_eq!(names(read_rules), ["name", "pm10"]);
        assert_eq!(history, &["5.7.3", "5.7.4"]);
        assert_eq!(names(history_rules), ["name"]);
        Ok(())
    }

    #[test]
    fn a_need_no_attribute_is_given_grants_nothing() -> Result<(), String> {
        // never the whole Entity: an empty list would read as every Attribute
        let got = split(&air(
            &json!({"*": {"history": false}}),
            &json!(["5.7.2", "5.7.3"]),
        ))?;
        assert_eq!(got.len(), 1);
        assert_eq!(got.first().map(|g| g.0.clone()), Some(vec!["5.7.2".into()]));
        Ok(())
    }

    #[test]
    fn write_kinds_narrow_each_write_clause() -> Result<(), String> {
        let got = split(&air(
            &json!({"*": {"write": []}, "name": {"write": ["update"]}}),
            &json!(["5.6.2", "5.6.3", "5.6.5", "5.6.8"]),
        ))?;
        // 5.6.2 (update) keeps name; append, delete and the upsert keep none
        let [(clauses, rules)] = got.as_slice() else {
            return Err(format!("{} policies, not 1", got.len()));
        };
        assert_eq!(clauses, &["5.6.2"]);
        assert_eq!(names(rules), ["name"]);
        Ok(())
    }

    #[test]
    fn read_false_keeps_the_type_lists_and_the_history() -> Result<(), String> {
        let got = split(&air(
            &json!({"pm10": {"read": false}}),
            &json!(["5.7.2", "5.7.3", "5.7.8"]),
        ))?;
        let by = |c: &str| {
            got.iter()
                .find(|(cl, _)| cl.iter().any(|x| x == c))
                .map(|(_, r)| names(r))
        };
        assert_eq!(by("5.7.2"), Some(vec!["name".into()]));
        assert_eq!(by("5.7.3"), Some(vec!["name".into(), "pm10".into()]));
        assert_eq!(by("5.7.8"), Some(vec!["name".into(), "pm10".into()]));
        Ok(())
    }

    #[test]
    fn an_entry_listing_no_attribute_is_narrowed_only_by_naming() -> Result<(), String> {
        let every = json!({"id": "p", "assignee": {"kind": "role", "id": "public"},
            "operations": ["5.7.2", "5.7.3"],
            "information": [{"entities": [{"type": "Air"}],
                             "attributes": {"*": {"history": false}, "name": {"history": true}}}]});
        let got = split(&every)?;
        let history = got
            .iter()
            .find(|(c, _)| c.iter().any(|x| x == "5.7.3"))
            .ok_or("no history policy")?;
        assert_eq!(names(&history.1), ["name"]);
        let current = got
            .iter()
            .find(|(c, _)| c.iter().any(|x| x == "5.7.2"))
            .ok_or("no read policy")?;
        assert!(current.1.iter().all(|r| r.every_attribute));
        let except = json!({"id": "p", "assignee": {"kind": "role", "id": "public"},
            "operations": ["5.7.3"],
            "information": [{"entities": [{"type": "Air"}],
                             "attributes": {"pm10": {"history": false}}}]});
        assert!(split(&except).is_err_and(|e| e.contains("list propertyNames")));
        Ok(())
    }

    #[test]
    fn a_malformed_capability_is_refused_by_name() {
        for (attributes, said) in [
            (json!({"speed": {"read": true}}), "\"speed\" is `*` or one"),
            (
                json!({"name": {"write": ["update", "update"]}}),
                "each value once",
            ),
            (
                json!({"name": {"metadata": ["colour"]}}),
                "metadata \"colour\"",
            ),
            (json!({"name": {"edit": true}}), "unknown field"),
        ] {
            let got = split(&air(&attributes, &json!(["5.7.2"])));
            assert!(
                got.as_ref().is_err_and(|e| e.contains(said)),
                "{said}: {:?}",
                got.err()
            );
        }
    }

    #[test]
    fn every_malformed_member_is_refused_by_name() {
        let cases = [
            (json!({"policies": [base(), base()]}), "named twice"),
            (json!({"policies": [with("id", json!(""))]}), "empty id"),
            (
                json!({"policies": [with("operations", json!([]))]}),
                "names no operation",
            ),
            (
                json!({"policies": [with("q", json!("speed>"))]}),
                "q \"speed>\"",
            ),
            (
                json!({"policies": [with("q", json!("a{b}==1"))]}),
                "linked Entity",
            ),
            (
                json!({"policies": [with("geoQ", json!("georel=near"))]}),
                "geoQ",
            ),
            (
                json!({"policies": [with("geoQ", json!("geometry=Point"))]}),
                "geoQ",
            ),
            (
                json!({"policies": [with("temporalQ", json!("timerel=after"))]}),
                "no timeAt",
            ),
            (
                json!({"policies": [with("temporalQ", json!("timerel=between;timeAt=P-1D"))]}),
                "no endTimeAt",
            ),
            (
                json!({"policies": [with("temporalQ", json!("timerel=during;timeAt=P-1D"))]}),
                "is not before",
            ),
            (
                json!({"policies": [with("temporalQ", json!("timerel=after;timeAt=P1D"))]}),
                "past",
            ),
            (
                json!({"policies": [with("temporalQ", json!("timerel=after;timeAt=P1Y"))]}),
                "neither",
            ),
            (
                json!({"policies": [with("temporalQ", json!("x=1"))]}),
                "is not timerel",
            ),
            (
                json!({"policies": [with("temporalQ", json!("timerel=before"))]}),
                "no timeAt",
            ),
            (
                json!({"policies": [with("temporalQ", json!("timerel=between;timeAt=P-2D;endTimeAt=soon"))]}),
                "soon",
            ),
            (
                json!({"policies": [with("temporalQ", json!("timerel=after;timeAt=P-99999999999999999999D"))]}),
                "P-99999999999999999999D",
            ),
            (
                json!({"policies": [with("geoQ", json!("near"))]}),
                "no georel",
            ),
            (
                json!({"policies": [with("scopeQ", json!("nope"))]}),
                "names no scope path",
            ),
            (
                json!({"policies": [with("information", json!([{"entities": []}]))]}),
                "selects no Entity",
            ),
            (
                json!({"policies": [with("information", json!([{"entities": [{"type": "Car", "id": "urn:a", "idPattern": "x"}]}]))]}),
                "both id and idPattern",
            ),
            (
                json!({"policies": [with("information", json!([{"entities": [{"type": "Car", "idPattern": "("}]}]))]}),
                "idPattern",
            ),
            (
                json!({"policies": [with("extra", json!(1))]}),
                "unknown field",
            ),
            (json!({"rules": []}), "unknown field"),
            (
                json!({"policies": [with("assignee", json!({"kind": "team", "id": "x"}))]}),
                "unknown variant",
            ),
        ];
        for (doc, said) in cases {
            let why = compile(&doc).err().unwrap_or_default();
            assert!(why.contains(said), "{doc}: {why:?} does not say {said:?}");
        }
    }

    /// T-3729: names expand by the core `@context`, the form the broker
    /// hands the engine; a policy's own conditions show every Attribute;
    /// an `idPattern` selects by its pattern.
    #[test]
    fn a_selector_keeps_its_id_pattern_and_the_conditions_every_attribute() -> Result<(), String> {
        assert_eq!(
            context().expand_key("location"),
            "https://uri.etsi.org/ngsi-ld/location"
        );
        let p = one(&with(
            "information",
            json!([{"entities": [{"type": "Car", "idPattern": "urn:ngsi-ld:Car:a.*"}]}]),
        ))?;
        assert!(p.conditions.every_attribute);
        let car = p.selectors.first().ok_or("no Car selector")?;
        assert_eq!(car.id_patterns, ["urn:ngsi-ld:Car:a.*"]);
        assert!(car.selects_id("urn:ngsi-ld:Car:a1") && !car.selects_id("urn:ngsi-ld:Car:b1"));
        Ok(())
    }

    #[test]
    fn conditions_and_validity_compile() -> Result<(), String> {
        let mut p = with("q", json!("speed<100"));
        if let Some(o) = p.as_object_mut() {
            o.insert("scopeQ".into(), json!("/geo/SK/#"));
            o.insert("effect".into(), json!("prohibit"));
            o.insert(
                "geoQ".into(),
                json!(
                    "georel=within;geometry=Polygon;coordinates=[[[0,0],[1,0],[1,1],[0,1],[0,0]]]"
                ),
            );
            o.insert(
                "temporalQ".into(),
                json!("timerel=between;timeAt=2026-01-01T00:00:00Z;endTimeAt=-PT1H"),
            );
            o.insert(
                "validity".into(),
                json!({"from": "2026-01-01T00:00:00Z", "to": "2027-01-01T00:00:00Z"}),
            );
        }
        let p = one(&p)?;
        assert!(p.prohibit && p.is_conditional());
        assert_eq!(
            p.conditions.temporal.first().and_then(|w| w.to),
            Some(TimeBound::Ago(Duration::hours(1)))
        );
        let at = |s: &str| {
            DateTime::parse_from_rfc3339(s)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_default()
        };
        assert!(!p.in_force(at("2025-12-31T23:59:59Z")));
        assert!(p.in_force(at("2026-01-01T00:00:00Z")));
        assert!(!p.in_force(at("2027-01-01T00:00:00Z")));
        let only_window = one(&with("temporalQ", json!("timerel=before;timeAt=P-1W")))?;
        assert!(only_window.is_conditional());
        Ok(())
    }

    #[test]
    fn durations_read_as_the_platform_wrote_them() {
        assert_eq!(duration("P-1D"), Some(-Duration::days(1)));
        assert_eq!(duration("-P1D"), Some(-Duration::days(1)));
        assert_eq!(duration("-PT1H30M"), Some(-Duration::minutes(90)));
        assert_eq!(
            duration("P1W2DT3S"),
            Some(Duration::days(9) + Duration::seconds(3))
        );
        // the last sums past what a Duration holds
        for bad in [
            "P",
            "1D",
            "P1",
            "P1M",
            "PT1D",
            "P1-2D",
            "Px",
            "-P100000000000DT2000000000000H",
        ] {
            assert_eq!(duration(bad), None, "{bad}");
        }
        // T-3729: a bound is a duration strictly before now
        assert_eq!(duration("P0D"), Some(Duration::zero()));
        for (raw, ok) in [("P0D", false), ("PT1S", false), ("-PT1S", true)] {
            assert_eq!(bound(raw).is_ok(), ok, "{raw}");
        }
    }

    #[test]
    fn a_scope_path_covers_its_subtree_and_nothing_beside_it() {
        assert_eq!(
            scope_paths("(/a/b/#|/c/);x").collect::<Vec<_>>(),
            ["/a/b", "/c"]
        );
        assert!(covers("/a/b", "/a/b"));
        assert!(covers("/a/b", "/a/b/c"));
        assert!(!covers("/a/b", "/a/bc"));
        assert!(!covers("/a/b", "/a"));
    }
}
