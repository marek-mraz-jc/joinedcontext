// SPDX-License-Identifier: EUPL-1.2
//! A view of another model (gap G18, `docs/analysis/view-mapping.md`).
//!
//! A view with a `mapping` serves the *target* class of a Mapping while the
//! store holds the *source* class. The view root inverts each request into
//! the source model ([`Mapping::invert`]), runs it through the view as any
//! other request, and rebuilds the answer in the target model
//! ([`Mapping::rebuild`]). The view and the policy engine judge the request
//! in the source model, so a mapped view serves at most what the same view
//! without its mapping serves (section 6): the rebuild reads only what the
//! narrowed answer holds and adds only the Mapping's own constants.
//!
//! The IR is the document the gateway reads today (version 2, section 2.1).
//! Its names are expanded through the view's `@context` at load, so a
//! caller's `@context` cannot rebind a slot. The request reaches the broker
//! under an inline `@context` of generated terms, one per source IRI,
//! because a `q` name is a 7.2.3.2 name and cannot carry an IRI.
//!
//! ```
//! use antares_ops::mapping::Mapping;
//! let ir = serde_json::json!({
//!     "version": 2, "sourceClass": "Sensor", "targetClass": "AirQuality",
//!     "slots": [{"target": "pm25", "source": "pm2p5", "kind": "rename", "filterable": true}]
//! });
//! let mapping = Mapping::compile(&ir, &antares_jsonld::core_context(), None).expect("an IR");
//! let pairs = vec![("q".to_owned(), "pm25>10".to_owned())];
//! let inverted = mapping.invert(&pairs).expect("it inverts");
//! assert_eq!(inverted.pairs, [("q".to_owned(), "jcSrc0>10".to_owned())]);
//! ```

use antares_jsonld::Context;
use antares_ql::{CmpOp, QNode, QPath, QValue};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The operations a mapped view may allow (section 7): the reads whose
/// request the view root inverts and whose answer it rebuilds.
pub const READS: [crate::OperationId; 11] = [
    crate::OperationId::RetrieveEntity,
    crate::OperationId::QueryEntities,
    crate::OperationId::RetrieveAttributeValue,
    crate::OperationId::TemporalRetrieve,
    crate::OperationId::TemporalQuery,
    crate::OperationId::RetrieveEntityTypes,
    crate::OperationId::RetrieveEntityTypeDetails,
    crate::OperationId::RetrieveEntityTypeInfo,
    crate::OperationId::RetrieveAttributes,
    crate::OperationId::RetrieveAttributeDetails,
    crate::OperationId::RetrieveAttributeInfo,
];

/// The discovery reads (10.6.3 to 10.6.8): answered from the view's
/// narrowed answer and named by the mapping ([`Mapping::rebuild_discovery`]).
pub const DISCOVERY: [crate::OperationId; 6] = [
    crate::OperationId::RetrieveEntityTypes,
    crate::OperationId::RetrieveEntityTypeDetails,
    crate::OperationId::RetrieveEntityTypeInfo,
    crate::OperationId::RetrieveAttributes,
    crate::OperationId::RetrieveAttributeDetails,
    crate::OperationId::RetrieveAttributeInfo,
];

/// The temporal reads (11.3.2, 11.3.3): each instance rebuilt
/// ([`Mapping::rebuild_history`]).
pub const HISTORY: [crate::OperationId; 2] = [
    crate::OperationId::TemporalRetrieve,
    crate::OperationId::TemporalQuery,
];

/// The IR version this interpreter runs (DM-52).
pub const IR_VERSION: u64 = 2;

/// The most slots one mapping holds.
pub const MAX_SLOTS: usize = 256;

/// The prefix of the generated terms the inner request names source
/// Attributes by.
const SOURCE_TERM: &str = "jcSrc";

/// The generated term of the source class.
const CLASS_TERM: &str = "jcSrcClass";

/// The `GeoProperty` a geo-query filters when it names none (4.10).
const LOCATION: &str = "location";

/// The members of an Entity that belong to NGSI-LD rather than to a model.
const STRUCTURAL: [&str; 3] = ["id", "@id", "scope"];

/// The members the broker generates, kept as a projection keeps them.
const SYSTEM: [&str; 4] = ["createdAt", "modifiedAt", "deletedAt", "expiresAt"];

/// The parameters that pass to the broker as the caller wrote them: none
/// names an Attribute or a type (`timeproperty` names a member NGSI-LD puts
/// on every instance).
const PASSED: [&str; 20] = [
    "id",
    "idPattern",
    "georel",
    "geometry",
    "coordinates",
    "scopeQ",
    "csf",
    "limit",
    "offset",
    "count",
    "options",
    "local",
    "orderFrom",
    "orderGeometry",
    "collation",
    "timerel",
    "timeAt",
    "endTimeAt",
    "timeproperty",
    "lastN",
];

/// The parameters of a discovery read; none names an Attribute or a type.
const DISCOVERY_PASSED: [&str; 3] = ["details", "local", "count"];

/// The attribute types of the normalized form (4.5).
const ATTRIBUTE_TYPES: [&str; 8] = [
    "Property",
    "GeoProperty",
    "Relationship",
    "LanguageProperty",
    "VocabProperty",
    "JsonProperty",
    "ListProperty",
    "ListRelationship",
];

/// A name the request uses that the target model does not define.
pub const UNKNOWN: &str =
    "the request names an attribute the model this view serves does not define";
/// A slot without an inverse named in a filter.
pub const NOT_FILTERABLE: &str =
    "the request filters on an attribute this view computes or translates without an inverse";
/// A comparison the slot's translation cannot invert.
pub const NOT_COMPARABLE: &str =
    "the request compares an attribute in a way its translation cannot invert";
/// A path below an attribute, or through a linked Entity.
pub const DEEP_PATH: &str =
    "a mapped view filters and orders on whole attributes of its own class only";
/// A parameter a mapped view does not serve.
pub const NOT_SERVED: &str = "the request uses a parameter a mapped view does not serve";
/// A parameter named twice.
pub const TWICE: &str = "the request names a parameter twice";
/// A `q` that does not parse.
pub const BAD_Q: &str = "the q parameter is not a query";
/// An attribute read through its path that has no source attribute.
pub const COMPUTED: &str =
    "this attribute is produced by the view; read it through Retrieve Entity";

/// What one target slot is made of (section 2.1).
#[derive(Debug, Clone, PartialEq)]
enum Derivation {
    Rename,
    /// `value * factor + offset`; the factor is finite and not zero.
    UnitConversion {
        factor: f64,
        offset: f64,
    },
    /// Source value text to target value; values outside pass unchanged.
    ValueMappings(BTreeMap<String, Value>),
    Cast(Cast),
    Constant(Value),
    Expr(Expression),
}

/// The casts the IR uses, as the reference engine performs them (DM-39).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cast {
    Integer,
    Float,
    String,
}

/// One node of a computed slot's expression (DM-52). A `Slot` names the
/// generated term of the source Attribute it reads.
#[derive(Debug, Clone, PartialEq)]
enum Expression {
    Slot(String),
    Constant(Value),
    Arithmetic(Arithmetic, Box<Expression>, Box<Expression>),
    Comparison(Comparison, Box<Expression>, Box<Expression>),
    Junction {
        all: bool,
        operands: Vec<Expression>,
    },
    Negate(Box<Expression>),
    Not(Box<Expression>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arithmetic {
    Add,
    Subtract,
    Multiply,
    Divide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Comparison {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

/// One target slot.
#[derive(Debug, Clone, PartialEq)]
struct Slot {
    /// The target Attribute's IRI.
    iri: String,
    /// The term the view serves it under (its IRI compacted through the
    /// view's `@context`).
    term: String,
    /// The source Attribute, absent for a constant and a computed slot.
    source: Option<Source>,
    derivation: Derivation,
    /// The compiler's verdict; it can only take a cell of 4.2 away.
    filterable: bool,
}

/// One source Attribute as the inner request names it and as the broker's
/// answer, compacted under the inner `@context`, keys it. The two differ
/// when a shorter term of the core `@context` names the same IRI.
#[derive(Debug, Clone, PartialEq)]
struct Source {
    iri: String,
    term: String,
    key: String,
}

/// A compiled Mapping, ready to run in both directions.
#[derive(Debug, Clone)]
pub struct Mapping {
    /// The view's `@context` over the core: what a caller's names mean.
    ctx: Context,
    /// The source class IRI, the view's one selector type.
    source_class: String,
    /// The term the answer names the target class by.
    target_class: String,
    target_class_iri: String,
    slots: Vec<Slot>,
    /// The inline `@context` the inner request runs under.
    inner: Value,
    /// Every source Attribute, in IRI order.
    sources: Vec<Source>,
    /// The `@context` the rebuilt answer carries.
    served: Value,
    /// The core with the inner terms over it: what the broker's answer
    /// names mean.
    answer: Context,
    /// The slots the view's rules never show ([`Self::hide`]): no filter,
    /// geo-query or sort reads them, or it would answer by what is hidden.
    hidden: BTreeSet<usize>,
}

/// A request inverted into the source model.
#[derive(Debug, Clone, PartialEq)]
pub struct Inverted {
    /// The query parameters for the inner request, decoded.
    pub pairs: Vec<(String, String)>,
    /// The `type` selects nothing the view serves: answer an empty result
    /// without asking the store.
    pub selects_nothing: bool,
    /// What the rebuilt answer keeps.
    pub keep: Keep,
}

/// The `attrs`, `pick` and `omit` of a request, applied to the rebuilt
/// answer in the target model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Keep {
    /// The target slots `attrs` asked for (indices), `None` for all.
    attrs: Option<Vec<usize>>,
    /// The members `pick` keeps, as the answer names them.
    pick: Option<Vec<String>>,
    /// The members `omit` drops, as the answer names them.
    omit: Option<Vec<String>>,
}

impl Mapping {
    /// The source class IRI: the view's selector must be exactly this.
    #[must_use]
    pub fn source_class(&self) -> &str {
        &self.source_class
    }

    /// The inline `@context` the inner request runs under.
    #[must_use]
    pub fn inner_context(&self) -> &Value {
        &self.inner
    }

    /// What a caller of the view names: the target class and each slot the
    /// view can produce when it shows the source Attributes `shown`
    /// (expanded IRIs), as discovery names them
    /// ([`Self::rebuild_discovery`]); nothing of the source model.
    #[must_use]
    pub fn target_names(&self, shown: &BTreeSet<String>) -> (Vec<String>, Vec<String>) {
        let present: BTreeSet<String> = self
            .sources
            .iter()
            .map(|s| s.iri.clone())
            .filter(|iri| shown.contains(iri))
            .collect();
        let mut slots: Vec<String> = self.shown(&present, true).map(|s| s.term.clone()).collect();
        slots.sort();
        (vec![self.target_class.clone()], slots)
    }

    /// The slots a view showing the source Attributes `shown` (expanded
    /// IRIs) never serves, because no rule shows what they read: after a
    /// model change that renamed or removed a source Attribute, or a
    /// Policy that hides it. Served, they would answer empty.
    #[must_use]
    pub fn hidden_slots(&self, shown: &BTreeSet<String>) -> Vec<String> {
        let (_, served) = self.target_names(shown);
        let mut hidden: Vec<String> = self
            .slots
            .iter()
            .map(|s| s.term.clone())
            .filter(|t| !served.contains(t))
            .collect();
        hidden.sort();
        hidden
    }

    /// Records [`Self::hidden_slots`] for the view that loads this mapping,
    /// so no filter reads them, and returns their names for the load note.
    pub fn hide(&mut self, shown: &BTreeSet<String>) -> Vec<String> {
        let hidden = self.hidden_slots(shown);
        self.hidden = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| hidden.contains(&s.term))
            .map(|(i, _)| i)
            .collect();
        hidden
    }

    /// A slot a filter, geo-query or sort may read: named, filterable and
    /// shown by the view's rules.
    fn filter_slot(&self, name: &str) -> Result<&Slot, &'static str> {
        let (i, slot) = self.slot_named(name).ok_or(UNKNOWN)?;
        if slot.filterable && !self.hidden.contains(&i) {
            Ok(slot)
        } else {
            Err(NOT_FILTERABLE)
        }
    }

    /// Read and compile one IR document. Names are expanded through `ctx`
    /// (the view's `@context` over the core); `served` is the view's own
    /// `@context`, which the rebuilt answer carries (the core when `None`).
    ///
    /// # Errors
    /// Why the document is not an IR this broker runs.
    pub fn compile(
        ir: &Value,
        ctx: &Context,
        served: Option<&Map<String, Value>>,
    ) -> Result<Self, String> {
        let members = ir.as_object().ok_or("mapping: the IR is an object")?;
        if let Some(unknown) = members.keys().find(|k| {
            !matches!(
                k.as_str(),
                "version" | "sourceClass" | "targetClass" | "slots"
            )
        }) {
            return Err(format!(
                "mapping: the IR member {unknown:?} is not one of version 2"
            ));
        }
        match members.get("version").and_then(Value::as_u64) {
            Some(IR_VERSION) => {}
            Some(v) => {
                return Err(format!(
                    "mapping: the IR is version {v}, and this broker runs version {IR_VERSION}"
                ))
            }
            None => return Err("mapping: the IR names no version".into()),
        }
        let iri = |what: &str, name: Option<&Value>| -> Result<String, String> {
            let name = name
                .and_then(Value::as_str)
                .ok_or_else(|| format!("mapping: {what} is a name"))?;
            let iri = ctx.expand_key(name);
            if antares_jsonld::context::is_absolute_iri(&iri) {
                Ok(iri)
            } else {
                Err(format!(
                    "mapping: {what} {name:?} does not expand to an IRI"
                ))
            }
        };
        let source_class = iri("sourceClass", members.get("sourceClass"))?;
        let target_class_iri = iri("targetClass", members.get("targetClass"))?;
        let listed = members
            .get("slots")
            .and_then(Value::as_array)
            .ok_or("mapping: the IR lists its slots")?;
        if listed.is_empty() || listed.len() > MAX_SLOTS {
            return Err(format!("mapping: an IR holds 1 to {MAX_SLOTS} slots"));
        }
        let mut raw = Vec::with_capacity(listed.len());
        for (index, entry) in listed.iter().enumerate() {
            raw.push(slot(index, entry, &iri)?);
        }
        let (inner, source_of, answer) = sources(&raw, &source_class)?;
        let key_of: BTreeMap<String, String> = source_of
            .iter()
            .map(|(iri, s)| (iri.clone(), s.key.clone()))
            .collect();
        let mut ctx = ctx.clone();
        ctx.freeze();
        let mut slots: Vec<Slot> = Vec::with_capacity(raw.len());
        for (target, source, derivation, filterable) in raw {
            if slots.iter().any(|s| s.iri == target) {
                return Err(format!("mapping: two slots serve {target:?}"));
            }
            slots.push(Slot {
                term: ctx.compact_iri(&target),
                iri: target,
                source: source.and_then(|s| source_of.get(&s).cloned()),
                derivation: derivation.with_terms(&key_of),
                filterable,
            });
        }
        Ok(Self {
            sources: source_of.into_values().collect(),
            target_class: ctx.compact_iri(&target_class_iri),
            target_class_iri,
            source_class,
            slots,
            inner: Value::Object(inner),
            answer,
            hidden: BTreeSet::new(),
            ctx,
            served: served.map_or_else(
                || Value::String(antares_jsonld::CORE_CONTEXT.to_owned()),
                |m| Value::Object(m.clone()),
            ),
        })
    }

    fn slot_named(&self, name: &str) -> Option<(usize, &Slot)> {
        let iri = self.ctx.expand_key(name.trim());
        self.slots.iter().enumerate().find(|(_, s)| s.iri == iri)
    }

    /// The request's query parameters in the source model (section 4.1).
    ///
    /// # Errors
    /// The fixed text of the 400 the request answers with.
    pub fn invert(&self, pairs: &[(String, String)]) -> Result<Inverted, &'static str> {
        let mut seen = BTreeSet::new();
        let mut out = Inverted {
            pairs: Vec::with_capacity(pairs.len()),
            selects_nothing: false,
            keep: Keep::default(),
        };
        for (name, value) in pairs {
            if !seen.insert(name.as_str()) {
                return Err(TWICE);
            }
            let rewritten = match name.as_str() {
                "type" => {
                    let Some(t) = self.invert_type(value) else {
                        out.selects_nothing = true;
                        continue;
                    };
                    t
                }
                "attrs" => {
                    let (sources, asked) = self.invert_attrs(value)?;
                    out.keep.attrs = Some(asked);
                    match sources {
                        Some(s) => s,
                        // only produced slots asked: the rebuild keeps them
                        None => continue,
                    }
                }
                "pick" => {
                    out.keep.pick = Some(self.members(value)?);
                    continue;
                }
                "omit" => {
                    out.keep.omit = Some(self.members(value)?);
                    continue;
                }
                "q" => self.invert_q(value)?,
                "geoproperty" | "geometryProperty" => self.geo_property(value)?,
                // an aggregate of a translated value is not the translation
                // of the aggregate (an offset summed n times)
                "options" if value.split(',').any(|o| o.trim() == "aggregatedValues") => {
                    return Err(NOT_SERVED)
                }
                "orderBy" => self.invert_order_by(value)?,
                "format"
                    if matches!(
                        value.as_str(),
                        "normalized" | "concise" | "keyValues" | "simplified"
                    ) =>
                {
                    value.clone()
                }
                passed if PASSED.contains(&passed) => value.clone(),
                _ => return Err(NOT_SERVED),
            };
            out.pairs.push((name.clone(), rewritten));
        }
        // 4.10: a geo-query without `geoproperty` filters `location`, the
        // target model's, so it is inverted like a named one
        if seen.contains("georel")
            && !seen.contains("geoproperty")
            && !seen.contains("geometryProperty")
        {
            let source = self.geo_property(LOCATION).map_err(|_| NOT_FILTERABLE)?;
            out.pairs.push(("geoproperty".into(), source));
        }
        Ok(out)
    }

    /// 4.17 type selection: an alternative that names only the target
    /// class (or `*`) selects the source class; `None` when no alternative
    /// does, so the request selects nothing the view serves.
    fn invert_type(&self, selection: &str) -> Option<String> {
        let served = selection.split([',', '|']).any(|alt| {
            alt.trim()
                .trim_start_matches('(')
                .trim_end_matches(')')
                .split(';')
                .all(|t| {
                    let t = t.trim();
                    t == "*" || self.ctx.expand_key(t) == self.target_class_iri
                })
        });
        served.then(|| CLASS_TERM.to_owned())
    }

    /// `attrs`: the source terms to ask for (`None` when only produced
    /// slots were asked) and the slots asked.
    fn invert_attrs(&self, list: &str) -> Result<(Option<String>, Vec<usize>), &'static str> {
        let mut sources: Vec<String> = Vec::new();
        let mut asked = Vec::new();
        for name in list.split(',').filter(|n| !n.trim().is_empty()) {
            let (i, slot) = self.slot_named(name).ok_or(UNKNOWN)?;
            asked.push(i);
            let mut reads = Vec::new();
            match &slot.derivation {
                Derivation::Expr(e) => {
                    let mut keys = Vec::new();
                    e.reads(&mut keys);
                    reads.extend(keys.iter().filter_map(|k| {
                        self.sources
                            .iter()
                            .find(|s| &s.key == k)
                            .map(|s| s.term.clone())
                    }));
                }
                _ => reads.extend(slot.source.iter().map(|s| s.term.clone())),
            }
            for r in reads {
                if !sources.contains(&r) {
                    sources.push(r);
                }
            }
        }
        if asked.is_empty() {
            return Err(UNKNOWN);
        }
        Ok(((!sources.is_empty()).then(|| sources.join(",")), asked))
    }

    /// `pick` / `omit`: each name a slot (as the answer names it) or a
    /// member NGSI-LD itself puts on an Entity.
    fn members(&self, list: &str) -> Result<Vec<String>, &'static str> {
        list.split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| {
                if n == "type" || STRUCTURAL.contains(&n) || SYSTEM.contains(&n) {
                    Ok(n.to_owned())
                } else {
                    self.slot_named(n)
                        .map(|(_, s)| s.term.clone())
                        .ok_or(UNKNOWN)
                }
            })
            .collect()
    }

    /// `geoproperty`: a filterable renamed slot. A geometry has no unit and
    /// no value table, so no other kind compares the same in both models.
    fn geo_property(&self, name: &str) -> Result<String, &'static str> {
        let slot = self.filter_slot(name)?;
        match (&slot.derivation, &slot.source) {
            (Derivation::Rename, Some(source)) => Ok(source.term.clone()),
            _ => Err(NOT_FILTERABLE),
        }
    }

    /// 4.23 `orderBy`: a renamed slot, or a unit conversion with its
    /// direction reversed when the factor is negative.
    fn invert_order_by(&self, spec: &str) -> Result<String, &'static str> {
        spec.split(',')
            .map(|part| {
                let (member, dir) = match part.split_once(';') {
                    Some((m, d)) => (m.trim(), Some(d.trim())),
                    None => (part.trim(), None),
                };
                if member.contains(['.', '[']) {
                    return Err(DEEP_PATH);
                }
                let slot = self.filter_slot(member)?;
                let source = slot
                    .source
                    .as_ref()
                    .map(|s| s.term.as_str())
                    .ok_or(NOT_FILTERABLE)?;
                let dir = match (&slot.derivation, dir) {
                    (Derivation::UnitConversion { factor, .. }, d) if *factor < 0.0 => {
                        match d.unwrap_or("asc") {
                            "asc" => Some("desc"),
                            "desc" => Some("asc"),
                            _ => return Err(NOT_COMPARABLE),
                        }
                    }
                    (Derivation::Rename, d)
                    | (Derivation::UnitConversion { .. }, d @ (None | Some("asc" | "desc"))) => d,
                    _ => return Err(NOT_COMPARABLE),
                };
                Ok(match dir {
                    Some(d) => format!("{source};{d}"),
                    None => source.to_owned(),
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|parts| parts.join(","))
    }

    /// `q` on the `antares-ql` AST (section 4.2), rendered back through its
    /// round-trip `Display`.
    fn invert_q(&self, q: &str) -> Result<String, &'static str> {
        let node = antares_ql::parse_q(q).map_err(|_| BAD_Q)?;
        Ok(self.invert_node(node)?.to_string())
    }

    fn invert_node(&self, node: QNode) -> Result<QNode, &'static str> {
        match node {
            QNode::And(ns) => ns
                .into_iter()
                .map(|n| self.invert_node(n))
                .collect::<Result<_, _>>()
                .map(QNode::And),
            QNode::Or(ns) => ns
                .into_iter()
                .map(|n| self.invert_node(n))
                .collect::<Result<_, _>>()
                .map(QNode::Or),
            // ADR-0023 rule 2, as the native view answers: a term on a slot
            // the rules hide holds for nobody (`!slot` for everybody), so
            // the hidden source value is never compared
            QNode::Exists { path, negated } if self.hidden_source(&path).is_some() => {
                Ok(hidden_term(self.hidden_source(&path), negated))
            }
            QNode::Cmp { path, .. } if self.hidden_source(&path).is_some() => {
                Ok(hidden_term(self.hidden_source(&path), false))
            }
            // a constant or a computed slot has no source: `source_path`
            // refuses it
            QNode::Exists { path, negated } => Ok(QNode::Exists {
                path: self.source_path(path)?.0,
                negated,
            }),
            QNode::Cmp { path, op, value } => {
                let deep =
                    !path.path.get(1..).unwrap_or_default().is_empty() || path.bracket.is_some();
                let (path, slot) = self.source_path(path)?;
                if deep && slot.derivation != Derivation::Rename {
                    return Err(DEEP_PATH);
                }
                invert_comparison(&slot.derivation, path, op, value)
            }
        }
    }

    /// The source term of the slot the term's top segment names, when the
    /// view's rules hide that slot.
    fn hidden_source(&self, path: &QPath) -> Option<&str> {
        let (i, slot) = self.slot_named(path.path.first()?)?;
        let source = slot.source.as_ref()?;
        self.hidden.contains(&i).then_some(source.term.as_str())
    }

    /// The source path of a term: its first segment renamed, the tail kept.
    fn source_path(&self, mut path: QPath) -> Result<(QPath, &Slot), &'static str> {
        if !path.links.is_empty() {
            return Err(DEEP_PATH);
        }
        let first = path.path.first_mut().ok_or(UNKNOWN)?;
        let slot = self.filter_slot(first)?;
        first.clone_from(&slot.source.as_ref().ok_or(NOT_FILTERABLE)?.term);
        Ok((path, slot))
    }

    /// The source term the attribute path segment of Retrieve Attribute
    /// Value names (percent-encoded as sent), and the slot it reads.
    ///
    /// # Errors
    /// The fixed text of the 400: no such slot, or one with no source.
    pub fn source_attribute(&self, segment: &str) -> Result<(usize, String), &'static str> {
        let name = antares_ql::percent_decode(segment.as_bytes());
        let (i, slot) = self.slot_named(&name).ok_or(UNKNOWN)?;
        match (&slot.derivation, &slot.source) {
            (Derivation::Constant(_) | Derivation::Expr(_), _) | (_, None) => Err(COMPUTED),
            (_, Some(source)) => Ok((i, source.term.clone())),
        }
    }

    /// Retrieve Attribute Value's answer in the target model: the
    /// Attribute, or its bare value, through the slot's derivation, with
    /// the view's `@context` in place of the inner one.
    ///
    /// `false` when the slot's derivation cannot translate the value: the
    /// view answers as for an absent Attribute.
    #[must_use]
    pub fn rebuild_attribute(&self, slot: usize, body: &mut Value) -> bool {
        let Some(mut rebuilt) = self
            .slots
            .get(slot)
            .and_then(|s| forward(&s.derivation, body))
        else {
            return false;
        };
        if let Some(m) = rebuilt.as_object_mut() {
            if m.contains_key("@context") {
                m.insert("@context".into(), self.served.clone());
            }
        }
        *body = rebuilt;
        true
    }

    /// A refusal's text with every generated term in it replaced by the
    /// target name that reads it (the target class for the class term), so
    /// an error the broker words in the inner request's terms names only
    /// the target model.
    #[must_use]
    pub fn reword(&self, text: &str) -> String {
        let mut out = text.replace(CLASS_TERM, &self.target_class);
        // the longest term first, so `jcSrc1` does not eat into `jcSrc10`
        let mut sources: Vec<&Source> = self.sources.iter().collect();
        sources.sort_by_key(|s| std::cmp::Reverse(s.term.len()));
        for source in sources {
            let target = self
                .slots
                .iter()
                .find(|s| s.source.as_ref() == Some(source))
                .map_or("an attribute", |s| s.term.as_str());
            out = out.replace(&source.term, target);
        }
        out
    }

    /// Rebuild an answer, one Entity or a page of them, in the target
    /// model (section 5), then apply `keep`.
    pub fn rebuild(&self, body: &mut Value, keep: &Keep) {
        match body {
            Value::Array(entities) => {
                for e in entities {
                    self.rebuild_entity(e, keep, false);
                }
            }
            entity => self.rebuild_entity(entity, keep, false),
        }
    }

    /// Rebuild a temporal answer (11.3.2, 11.3.3) in the target model:
    /// each instance of a renamed or translated slot, in the normalized
    /// and the `temporalValues` form. A constant and a computed slot have
    /// no history and are left out.
    pub fn rebuild_history(&self, body: &mut Value, keep: &Keep) {
        match body {
            Value::Array(entities) => {
                for e in entities {
                    self.rebuild_entity(e, keep, true);
                }
            }
            entity => self.rebuild_entity(entity, keep, true),
        }
    }

    fn rebuild_entity(&self, entity: &mut Value, keep: &Keep, history: bool) {
        let Some(members) = entity.as_object() else {
            return;
        };
        if !members.contains_key("id") && !members.contains_key("@id") {
            return;
        }
        let mut out = Map::new();
        if members.contains_key("@context") {
            out.insert("@context".into(), self.served.clone());
        }
        for name in STRUCTURAL.iter().chain(SYSTEM.iter()) {
            if let Some(v) = members.get(*name) {
                out.insert((*name).to_owned(), v.clone());
            }
        }
        out.insert("type".into(), Value::String(self.target_class.clone()));
        let normalized = is_normalized(members);
        for (i, slot) in self.slots.iter().enumerate() {
            if keep.attrs.as_ref().is_some_and(|a| !a.contains(&i)) {
                continue;
            }
            let value = match &slot.derivation {
                Derivation::Constant(_) | Derivation::Expr(_) if history => None,
                Derivation::Constant(v) => Some(produced(v.clone(), normalized)),
                Derivation::Expr(e) => e.evaluate(members).map(|v| produced(v, normalized)),
                d => slot
                    .source
                    .as_ref()
                    .and_then(|s| members.get(&s.key))
                    .and_then(|a| forward(d, a)),
            };
            if let Some(v) = value {
                out.insert(slot.term.clone(), v);
            }
        }
        if let Some(pick) = &keep.pick {
            out.retain(|k, _| k == "@context" || pick.contains(k));
        }
        if let Some(omit) = &keep.omit {
            out.retain(|k, _| !omit.contains(k));
        }
        *entity = Value::Object(out);
    }
}

impl Mapping {
    /// The term the inner request names the type of Retrieve Entity Type
    /// Information by (its path segment, percent-encoded as sent): the
    /// target class reads the source class; `None` for any other type.
    #[must_use]
    pub fn source_type(&self, segment: &str) -> Option<&'static str> {
        let name = antares_ql::percent_decode(segment.as_bytes());
        (self.ctx.expand_key(&name) == self.target_class_iri).then_some(CLASS_TERM)
    }

    /// A discovery answer (10.6.3 to 10.6.8) in the target model: the
    /// target class for the source class, and the target slots the view
    /// shows for the source Attributes the narrowed answer names (ADR-0023
    /// rule 5, then named by the mapping). A slot with a source shows when
    /// its source does, a computed one when every source it reads does, a
    /// constant when the answer shows the class or any Attribute of it; no
    /// source name survives. `slot` is the slot Retrieve Attribute
    /// Information asked for.
    pub fn rebuild_discovery(&self, body: &mut Value, slot: Option<usize>) {
        match body {
            Value::Array(items) => {
                let rebuilt = if items
                    .iter()
                    .any(|i| i.get("type").and_then(Value::as_str) == Some("EntityType"))
                {
                    items
                        .iter()
                        .filter(|i| self.is_source_class(i.get("id")))
                        .map(|i| self.entity_type(i))
                        .collect()
                } else {
                    self.attribute_entries(items)
                };
                *items = rebuilt;
            }
            Value::Object(m) => {
                let names = |key: &str| -> Vec<Value> {
                    m.get(key)
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                };
                match m.get("type").and_then(Value::as_str) {
                    Some("EntityTypeList") => {
                        let shows = names("typeList")
                            .iter()
                            .any(|t| self.is_source_class(Some(t)));
                        let list = if shows {
                            vec![Value::String(self.target_class.clone())]
                        } else {
                            Vec::new()
                        };
                        m.insert("typeList".into(), Value::Array(list));
                    }
                    Some("EntityTypeInfo") => {
                        let details = self.attribute_entries(&names("attributeDetails"));
                        self.name_class(m);
                        m.insert("attributeDetails".into(), Value::Array(details));
                    }
                    Some("AttributeList") => {
                        let present = self.present(&names("attributeList"));
                        let list = self
                            .shown(&present, !present.is_empty())
                            .map(|s| Value::String(s.term.clone()))
                            .collect();
                        m.insert("attributeList".into(), Value::Array(list));
                    }
                    Some("Attribute") => {
                        if let Some(s) = slot.and_then(|i| self.slots.get(i)) {
                            self.name_attribute(m, s);
                        }
                    }
                    _ => {}
                }
                self.serve_context(m);
            }
            _ => {}
        }
    }

    /// Whether `name` (an IRI or a term of the answer) is the source class.
    fn is_source_class(&self, name: Option<&Value>) -> bool {
        name.and_then(Value::as_str)
            .is_some_and(|n| self.answer.expand_key(n) == self.source_class)
    }

    /// The source IRIs `names` (IRIs or terms of the answer) name.
    fn present(&self, names: &[Value]) -> BTreeSet<String> {
        names
            .iter()
            .filter_map(Value::as_str)
            .map(|n| self.answer.expand_key(n))
            .filter(|iri| self.sources.iter().any(|s| &s.iri == iri))
            .collect()
    }

    /// The slots shown when the answer names the sources in `present`;
    /// `class`: it shows the class.
    fn shown<'a>(
        &'a self,
        present: &'a BTreeSet<String>,
        class: bool,
    ) -> impl Iterator<Item = &'a Slot> + 'a {
        self.slots
            .iter()
            .filter(move |slot| match &slot.derivation {
                Derivation::Constant(_) => class,
                Derivation::Expr(e) => {
                    let mut keys = Vec::new();
                    e.reads(&mut keys);
                    class
                        && keys.iter().all(|k| {
                            self.sources
                                .iter()
                                .any(|s| &s.key == k && present.contains(&s.iri))
                        })
                }
                _ => slot
                    .source
                    .as_ref()
                    .is_some_and(|s| present.contains(&s.iri)),
            })
    }

    /// One `EntityType` of the detailed type list, for the source class.
    fn entity_type(&self, item: &Value) -> Value {
        let mut out = item.as_object().cloned().unwrap_or_default();
        let names = out
            .get("attributeNames")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let present = self.present(&names);
        let shown = self
            .shown(&present, true)
            .map(|s| Value::String(s.term.clone()))
            .collect();
        self.name_class(&mut out);
        out.insert("attributeNames".into(), Value::Array(shown));
        self.serve_context(&mut out);
        Value::Object(out)
    }

    /// `Attribute` entries (a detailed list, or a type's
    /// `attributeDetails`) in the target model: one per shown slot, made
    /// from its source's entry; a slot without a source is a Property.
    fn attribute_entries(&self, entries: &[Value]) -> Vec<Value> {
        let ids: Vec<Value> = entries
            .iter()
            .filter_map(|e| e.get("id").cloned())
            .collect();
        let present = self.present(&ids);
        let entry_of = |iri: &str| {
            entries.iter().find(|e| {
                e.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| self.answer.expand_key(id) == iri)
            })
        };
        self.shown(&present, !present.is_empty())
            .filter_map(|slot| {
                let from = slot.source.as_ref().and_then(|s| entry_of(&s.iri));
                let mut out = from.or_else(|| entries.first())?.as_object()?.clone();
                if from.is_none() {
                    out.remove("attributeCount");
                    if out.contains_key("attributeTypes") {
                        out.insert("attributeTypes".into(), serde_json::json!(["Property"]));
                    }
                }
                self.name_attribute(&mut out, slot);
                self.serve_context(&mut out);
                Some(Value::Object(out))
            })
            .collect()
    }

    fn name_class(&self, m: &mut Map<String, Value>) {
        m.insert("id".into(), Value::String(self.target_class_iri.clone()));
        m.insert("typeName".into(), Value::String(self.target_class.clone()));
    }

    fn name_attribute(&self, m: &mut Map<String, Value>, slot: &Slot) {
        m.insert("id".into(), Value::String(slot.iri.clone()));
        m.insert("attributeName".into(), Value::String(slot.term.clone()));
        if m.contains_key("typeNames") {
            m.insert(
                "typeNames".into(),
                Value::Array(vec![Value::String(self.target_class.clone())]),
            );
        }
    }

    fn serve_context(&self, m: &mut Map<String, Value>) {
        if m.contains_key("@context") {
            m.insert("@context".into(), self.served.clone());
        }
    }
}

/// The parameters of a discovery read: `details`, `local` and `count`
/// pass, once each.
///
/// # Errors
/// The fixed text of the 400: a parameter named twice or one a mapped view
/// does not serve.
pub fn discovery_pairs(pairs: &[(String, String)]) -> Result<(), &'static str> {
    let mut seen = BTreeSet::new();
    for (name, _) in pairs {
        if !seen.insert(name.as_str()) {
            return Err(TWICE);
        }
        if !DISCOVERY_PASSED.contains(&name.as_str()) {
            return Err(NOT_SERVED);
        }
    }
    Ok(())
}

impl Derivation {
    /// The derivation with its source IRIs replaced by the answer's keys.
    fn with_terms(self, term_of: &BTreeMap<String, String>) -> Self {
        match self {
            Derivation::Expr(e) => Derivation::Expr(e.with_terms(term_of)),
            other => other,
        }
    }
}

impl Expression {
    /// The source names this expression reads, appended once each.
    fn reads(&self, into: &mut Vec<String>) {
        match self {
            Expression::Slot(name) => {
                if !into.contains(name) {
                    into.push(name.clone());
                }
            }
            Expression::Constant(_) => {}
            Expression::Arithmetic(_, l, r) | Expression::Comparison(_, l, r) => {
                l.reads(into);
                r.reads(into);
            }
            Expression::Junction { operands, .. } => {
                for o in operands {
                    o.reads(into);
                }
            }
            Expression::Negate(o) | Expression::Not(o) => o.reads(into),
        }
    }

    fn with_terms(self, term_of: &BTreeMap<String, String>) -> Self {
        let walk = |e: Box<Expression>| Box::new(e.with_terms(term_of));
        match self {
            Expression::Slot(iri) => Expression::Slot(term_of.get(&iri).cloned().unwrap_or(iri)),
            Expression::Constant(v) => Expression::Constant(v),
            Expression::Arithmetic(op, l, r) => Expression::Arithmetic(op, walk(l), walk(r)),
            Expression::Comparison(op, l, r) => Expression::Comparison(op, walk(l), walk(r)),
            Expression::Junction { all, operands } => Expression::Junction {
                all,
                operands: operands
                    .into_iter()
                    .map(|o| o.with_terms(term_of))
                    .collect(),
            },
            Expression::Negate(o) => Expression::Negate(walk(o)),
            Expression::Not(o) => Expression::Not(walk(o)),
        }
    }

    /// The value for one Entity, `None` where it has none: an input the
    /// answer does not hold (the view hides it, or the Entity lacks it), or
    /// operands that do not combine. A view never invents a value.
    fn evaluate(&self, members: &Map<String, Value>) -> Option<Value> {
        match self {
            Expression::Slot(name) => members.get(name).map(plain_value),
            Expression::Constant(v) => Some(v.clone()),
            Expression::Arithmetic(op, l, r) => {
                arithmetic(*op, &l.evaluate(members)?, &r.evaluate(members)?)
            }
            Expression::Comparison(op, l, r) => {
                compare(*op, &l.evaluate(members)?, &r.evaluate(members)?)
            }
            // every operand is evaluated: one that cannot be is a value the
            // view does not serve, whatever the order of the terms
            Expression::Junction { all, operands } => {
                let mut answer = *all;
                for o in operands {
                    let v = o.evaluate(members)?.as_bool()?;
                    answer = if *all { answer && v } else { answer || v };
                }
                Some(Value::Bool(answer))
            }
            Expression::Negate(o) => match o.evaluate(members)? {
                Value::Number(n) => match n.as_i64() {
                    Some(whole) => whole.checked_neg().map(Value::from),
                    None => finite(-n.as_f64()?),
                },
                _ => None,
            },
            Expression::Not(o) => Some(Value::Bool(!o.evaluate(members)?.as_bool()?)),
        }
    }
}

/// One attribute's value, whatever shape it arrived in.
fn plain_value(attribute: &Value) -> Value {
    attribute
        .as_object()
        .and_then(|m| ["value", "object"].into_iter().find_map(|k| m.get(k)))
        .unwrap_or(attribute)
        .clone()
}

fn arithmetic(op: Arithmetic, l: &Value, r: &Value) -> Option<Value> {
    if let (Some(a), Some(b)) = (l.as_str(), r.as_str()) {
        return (op == Arithmetic::Add).then(|| Value::String(format!("{a}{b}")));
    }
    // two whole numbers stay whole, as in Python and Bloblang
    if let (Some(a), Some(b)) = (l.as_i64(), r.as_i64()) {
        match op {
            Arithmetic::Add => return a.checked_add(b).map(Value::from),
            Arithmetic::Subtract => return a.checked_sub(b).map(Value::from),
            Arithmetic::Multiply => return a.checked_mul(b).map(Value::from),
            Arithmetic::Divide => {}
        }
    }
    let (a, b) = (l.as_f64()?, r.as_f64()?);
    match op {
        Arithmetic::Add => finite(a + b),
        Arithmetic::Subtract => finite(a - b),
        Arithmetic::Multiply => finite(a * b),
        Arithmetic::Divide if b == 0.0 => None,
        Arithmetic::Divide => finite(a / b),
    }
}

fn finite(v: f64) -> Option<Value> {
    serde_json::Number::from_f64(v).map(Value::Number)
}

fn compare(op: Comparison, l: &Value, r: &Value) -> Option<Value> {
    if let (Some(a), Some(b)) = (l.as_bool(), r.as_bool()) {
        return match op {
            Comparison::Equal => Some(Value::Bool(a == b)),
            Comparison::NotEqual => Some(Value::Bool(a != b)),
            _ => None,
        };
    }
    let ord = match (l.as_str(), r.as_str()) {
        (Some(a), Some(b)) => a.cmp(b),
        _ => l.as_f64()?.partial_cmp(&r.as_f64()?)?,
    };
    Some(Value::Bool(match op {
        Comparison::Equal => ord.is_eq(),
        Comparison::NotEqual => ord.is_ne(),
        Comparison::Less => ord.is_lt(),
        Comparison::LessOrEqual => ord.is_le(),
        Comparison::Greater => ord.is_gt(),
        Comparison::GreaterOrEqual => ord.is_ge(),
    }))
}

/// A term that holds for nobody (`negated`: for everybody) whatever the
/// Entity carries: `a;!a` or `a|!a` on the hidden source term, which reads
/// its presence as a constant and never its value.
fn hidden_term(source: Option<&str>, negated: bool) -> QNode {
    let top = source.unwrap_or_default().to_owned();
    let is = |negated| QNode::Exists {
        path: QPath::dotted(vec![top.clone()]),
        negated,
    };
    let pair = vec![is(false), is(true)];
    if negated {
        QNode::Or(pair)
    } else {
        QNode::And(pair)
    }
}

/// One comparison of 4.2 in the source model.
fn invert_comparison(
    d: &Derivation,
    path: QPath,
    op: CmpOp,
    value: QValue,
) -> Result<QNode, &'static str> {
    match d {
        Derivation::Rename => Ok(QNode::Cmp { path, op, value }),
        Derivation::UnitConversion { factor, offset } => {
            // Eq/Ne round away the float noise of the division, so a typed
            // 1.1 meets the stored decimal; an ordering keeps the exact
            // quotient, whose 9-digit rounding moved the boundary to either
            // side (factor 3, `t>1`: a source of 0.3333333331 matched)
            let exact = matches!(op, CmpOp::Gt | CmpOp::Ge | CmpOp::Lt | CmpOp::Le);
            let back = |v: QValue| match v {
                QValue::Num(n) if exact => Ok(QValue::Num((n - offset) / factor)),
                QValue::Num(n) => Ok(QValue::Num(rounded((n - offset) / factor))),
                _ => Err(NOT_COMPARABLE),
            };
            let reversed = *factor < 0.0;
            let (op, value) = match (op, value) {
                (CmpOp::Eq | CmpOp::Ne, QValue::List(items)) => (
                    op,
                    QValue::List(items.into_iter().map(back).collect::<Result<_, _>>()?),
                ),
                // a negative factor reverses the order: the ends trade places
                (CmpOp::Eq | CmpOp::Ne, QValue::Range(lo, hi)) => {
                    let (lo, hi) = (back(*lo)?, back(*hi)?);
                    let (lo, hi) = if reversed { (hi, lo) } else { (lo, hi) };
                    (op, QValue::Range(Box::new(lo), Box::new(hi)))
                }
                (CmpOp::Eq | CmpOp::Ne, v) => (op, back(v)?),
                (CmpOp::Gt | CmpOp::Ge | CmpOp::Lt | CmpOp::Le, v) => {
                    let op = match (op, reversed) {
                        (CmpOp::Gt, true) => CmpOp::Lt,
                        (CmpOp::Ge, true) => CmpOp::Le,
                        (CmpOp::Lt, true) => CmpOp::Gt,
                        (CmpOp::Le, true) => CmpOp::Ge,
                        (op, _) => op,
                    };
                    (op, back(v)?)
                }
                _ => return Err(NOT_COMPARABLE),
            };
            Ok(QNode::Cmp { path, op, value })
        }
        Derivation::ValueMappings(forward) => {
            if !matches!(op, CmpOp::Eq | CmpOp::Ne) {
                return Err(NOT_COMPARABLE);
            }
            let asked = match value {
                QValue::List(items) => items,
                QValue::Range(..) => return Err(NOT_COMPARABLE),
                one => vec![one],
            };
            let mut sources: Vec<QValue> = Vec::new();
            for target in asked {
                let text = scalar_text(&target);
                let mut add = |v: QValue| {
                    if !sources.contains(&v) {
                        sources.push(v);
                    }
                };
                for (key, produced) in forward {
                    if plain(produced) == text {
                        add(source_value(key));
                    }
                }
                // a value outside the table passes unchanged, so it is its
                // own preimage unless the table maps it elsewhere
                if !forward.contains_key(&text) {
                    add(target);
                }
            }
            Ok(match (sources.len(), op) {
                (0, CmpOp::Eq) => QNode::And(vec![
                    QNode::Exists {
                        path: path.clone(),
                        negated: false,
                    },
                    QNode::Exists {
                        path,
                        negated: true,
                    },
                ]),
                (0, _) => QNode::Exists {
                    path,
                    negated: false,
                },
                (1, _) => QNode::Cmp {
                    path,
                    op,
                    value: sources.remove(0),
                },
                _ => QNode::Cmp {
                    path,
                    op,
                    value: QValue::List(sources),
                },
            })
        }
        // `int()` truncates, so a source range would be needed per target
        // value (Decided in view-mapping.md 4.2); constants and expressions
        // have no inverse
        Derivation::Cast(_) | Derivation::Constant(_) | Derivation::Expr(_) => Err(NOT_COMPARABLE),
    }
}

/// A table key as the scalar the store holds: the IR keys a number or a
/// boolean by its text.
fn source_value(key: &str) -> QValue {
    match serde_json::from_str::<Value>(key) {
        Ok(Value::Number(n)) => n
            .as_f64()
            .map_or_else(|| QValue::Str(key.to_owned()), QValue::Num),
        Ok(Value::Bool(b)) => QValue::Bool(b),
        _ => QValue::Str(key.to_owned()),
    }
}

/// A scalar of a `q` (4.9 lists hold scalars only) as the text a table is
/// keyed by.
fn scalar_text(v: &QValue) -> String {
    match v {
        QValue::Str(s) => s.clone(),
        other => other.to_string(),
    }
}

/// An inverted number at nine decimals, so `19` through kelvin reads
/// `292.15` and not `292.15000000000003`.
fn rounded(v: f64) -> f64 {
    let r = (v * 1e9).round() / 1e9;
    if r.is_finite() {
        r
    } else {
        v
    }
}

/// The target value one source attribute becomes; `None` when the
/// derivation cannot translate it (a language map, a JSON value, a
/// unit conversion of a value that is not a number): a view serves no
/// source value under a translated name.
fn forward(d: &Derivation, attribute: &Value) -> Option<Value> {
    match d {
        Derivation::Rename | Derivation::Constant(_) | Derivation::Expr(_) => {
            Some(attribute.clone())
        }
        Derivation::UnitConversion { factor, offset } => {
            map_value(attribute, &|v| match v {
                // a number past the JSON range keeps its value
                Value::Number(_) => Some(
                    v.as_f64()
                        .and_then(|n| finite(n * factor + offset))
                        .unwrap_or_else(|| v.clone()),
                ),
                // a geometry or a structured value: no scalar to convert
                Value::Object(_) | Value::Null => Some(v.clone()),
                // a text or a boolean is no quantity in either unit
                _ => None,
            })
        }
        Derivation::ValueMappings(table) => map_value(attribute, &|v| {
            Some(table_value(table, v).unwrap_or(v).clone())
        }),
        Derivation::Cast(c) => map_value(attribute, &|v| Some(cast_value(*c, v))),
    }
}

/// The forward value of `v`: keyed by its text, or, for a number, by any
/// key holding the same number (`1.0` and `1`).
fn table_value<'a>(table: &'a BTreeMap<String, Value>, v: &Value) -> Option<&'a Value> {
    table.get(&plain(v)).or_else(|| {
        let n = v.as_f64()?;
        table
            .iter()
            .find(|(k, _)| matches!(source_value(k), QValue::Num(m) if m.to_bits() == n.to_bits()))
            .map(|(_, out)| out)
    })
}

/// The members holding an attribute's value (4.5, 4.5.9 for
/// `temporalValues`), per kind: those the derivation translates, and
/// those it cannot.
const VALUE_MEMBERS: [&str; 5] = ["value", "object", "vocab", "valueList", "objectList"];
const TEMPORAL_MEMBERS: [&str; 5] = ["values", "objects", "vocabs", "valueLists", "objectLists"];
const OPAQUE_MEMBERS: [&str; 4] = ["languageMap", "json", "languageMaps", "jsons"];

/// `transform` on a value, or on each element of a list.
fn each(v: &Value, transform: &dyn Fn(&Value) -> Option<Value>) -> Option<Value> {
    match v {
        // an `objectList` holds `{"object": …}` members
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if item.is_object() {
                    map_value(item, transform)
                } else {
                    transform(item)
                }
            })
            .collect::<Option<Vec<_>>>()
            .map(Value::Array),
        v => transform(v),
    }
}

/// `transform` on an attribute's value in any representation and kind:
/// normalized, concise, simplified, each instance of a multi-attribute,
/// and the elements of a list. `None` drops what cannot be translated: a
/// language map or a JSON value, or an instance `transform` refuses.
fn map_value(attribute: &Value, transform: &dyn Fn(&Value) -> Option<Value>) -> Option<Value> {
    match attribute {
        Value::Array(instances) if instances.iter().all(Value::is_object) => {
            let kept: Vec<Value> = instances
                .iter()
                .filter_map(|i| map_value(i, transform))
                .collect();
            (!kept.is_empty() || instances.is_empty()).then_some(Value::Array(kept))
        }
        Value::Object(m) => {
            if OPAQUE_MEMBERS.iter().any(|k| m.contains_key(*k)) {
                return None;
            }
            let mut out = m.clone();
            if let Some(key) = VALUE_MEMBERS.into_iter().find(|k| m.contains_key(*k)) {
                let v = m.get(key)?;
                out.insert(key.to_owned(), each(v, transform)?);
                return Some(Value::Object(out));
            }
            // `temporalValues` (4.5.9): `[value, time]` pairs; a pair whose
            // value is not translated is left out
            for key in TEMPORAL_MEMBERS {
                if let Some(Value::Array(pairs)) = out.get_mut(key) {
                    pairs.retain_mut(|pair| {
                        let Some(v) = pair.get_mut(0) else {
                            return true;
                        };
                        each(v, transform).map(|t| *v = t).is_some()
                    });
                }
            }
            // a geometry in the simplified form: no scalar to translate
            Some(Value::Object(out))
        }
        // a simplified ListProperty is the array of its values
        list => each(list, transform),
    }
}

/// One cast as the reference engine performs it (DM-39); a value the cast
/// cannot make stays as it is, since a view does not invent data.
fn cast_value(c: Cast, v: &Value) -> Value {
    match c {
        Cast::String => Value::String(plain(v)),
        Cast::Float => v
            .as_f64()
            .or_else(|| plain(v).parse().ok())
            .and_then(finite)
            .unwrap_or_else(|| v.clone()),
        // Python `int()` truncates towards zero
        Cast::Integer => v
            .as_i64()
            .or_else(|| {
                v.as_f64()
                    .and_then(|f| format!("{}", f.trunc()).parse().ok())
            })
            .or_else(|| plain(v).parse().ok())
            .map_or_else(|| v.clone(), Value::from),
    }
}

fn plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Whether the answer is normalized, judged from its attributes; one with
/// none to judge by takes the default representation.
fn is_normalized(members: &Map<String, Value>) -> bool {
    let mut attributes = members
        .iter()
        .filter(|(k, _)| {
            let k = k.as_str();
            !matches!(k, "type" | "@type" | "@context")
                && !STRUCTURAL.contains(&k)
                && !SYSTEM.contains(&k)
        })
        .peekable();
    attributes.peek().is_none()
        || attributes.any(|(_, v)| {
            let first = match v {
                Value::Array(a) => a.first(),
                v => Some(v),
            };
            first
                .and_then(|v| v.get("type"))
                .and_then(Value::as_str)
                .is_some_and(|t| ATTRIBUTE_TYPES.contains(&t))
        })
}

/// A value the view produces, in the Entity's own representation.
fn produced(v: Value, normalized: bool) -> Value {
    if normalized {
        serde_json::json!({"type": "Property", "value": v})
    } else {
        v
    }
}

/// The inline `@context` of the inner request, and each source IRI's
/// generated term and answer key: one term per IRI, in IRI order, so equal
/// IRs give equal inner requests.
fn sources(
    raw: &[RawSlot],
    source_class: &str,
) -> Result<(Map<String, Value>, SourceMap, Context), String> {
    let iris: BTreeSet<String> = raw
        .iter()
        .flat_map(|(_, source, derivation, _)| {
            let mut reads: Vec<String> = source.iter().cloned().collect();
            if let Derivation::Expr(e) = derivation {
                e.reads(&mut reads);
            }
            reads
        })
        .collect();
    let term_of: BTreeMap<String, String> = iris
        .iter()
        .enumerate()
        .map(|(i, iri)| (iri.clone(), format!("{SOURCE_TERM}{i}")))
        .collect();
    let mut inner: Map<String, Value> = term_of
        .iter()
        .map(|(iri, term)| (term.clone(), Value::String(iri.clone())))
        .collect();
    inner.insert(CLASS_TERM.into(), Value::String(source_class.to_owned()));
    // the answer is compacted as the broker compacts it: the core with
    // the inner terms over it
    let mut answer = antares_jsonld::core_context();
    answer
        .merge_object(&inner)
        .map_err(|e| format!("mapping: the inner @context: {e}"))?;
    answer.freeze();
    let source_of: BTreeMap<String, Source> = term_of
        .iter()
        .map(|(iri, term)| {
            let key = answer.compact_iri(iri);
            (
                iri.clone(),
                Source {
                    iri: iri.clone(),
                    term: term.clone(),
                    key,
                },
            )
        })
        .collect();
    Ok((inner, source_of, answer))
}

/// Each source IRI with its term and its answer key.
type SourceMap = BTreeMap<String, Source>;

/// Expands one IR name to an IRI, or says why it does not.
type Expand<'a> = &'a dyn Fn(&str, Option<&Value>) -> Result<String, String>;

type RawSlot = (String, Option<String>, Derivation, bool);

/// One slot entry of the IR, its names expanded by `iri`.
fn slot(index: usize, entry: &Value, iri: Expand<'_>) -> Result<RawSlot, String> {
    let at = |reason: &str| format!("mapping: slot {index}: {reason}");
    let m = entry.as_object().ok_or_else(|| at("not an object"))?;
    let target = iri(&format!("slot {index} target"), m.get("target"))?;
    let source = match m.get("source") {
        Some(s) => Some(iri(&format!("slot {index} source"), Some(s))?),
        None => None,
    };
    let kind = m
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| at("names no kind"))?;
    // Every member changes what the view answers or which `q` it inverts, so an unknown one
    // (a typo such as `ofset`, or a member a newer compiler adds) refuses the IR instead of
    // being dropped (T-5207, mapping-contract rule 3).
    let own: &[&str] = match kind {
        "unitConversion" => &["factor", "offset"],
        "valueMappings" => &["forward", "inverse"],
        "cast" => &["range"],
        "constant" => &["value"],
        "expr" => &["expression"],
        _ => &[],
    };
    if let Some(unknown) = m.keys().find(|k| {
        !matches!(k.as_str(), "target" | "source" | "kind" | "filterable")
            && !own.contains(&k.as_str())
    }) {
        return Err(at(&format!(
            "member {unknown:?} is not one a {kind} slot takes"
        )));
    }
    let derivation = match kind {
        "rename" => Derivation::Rename,
        "unitConversion" => {
            let factor = m
                .get("factor")
                .and_then(Value::as_f64)
                .ok_or_else(|| at("a unit conversion carries a factor"))?;
            let offset = match m.get("offset") {
                None => 0.0,
                Some(o) => o.as_f64().ok_or_else(|| at("an offset is a number"))?,
            };
            // a JSON number is finite: zero is the one factor with no inverse
            if factor == 0.0 {
                return Err(at("a unit conversion has a non-zero factor"));
            }
            Derivation::UnitConversion { factor, offset }
        }
        "valueMappings" => Derivation::ValueMappings(
            m.get("forward")
                .and_then(Value::as_object)
                .ok_or_else(|| at("a value mapping carries its forward table"))?
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        "cast" => Derivation::Cast(match m.get("range").and_then(Value::as_str) {
            Some("integer") => Cast::Integer,
            Some("float") => Cast::Float,
            Some("string") => Cast::String,
            _ => return Err(at("a cast is to integer, float or string")),
        }),
        "constant" => Derivation::Constant(
            m.get("value")
                .cloned()
                .ok_or_else(|| at("a constant carries a value"))?,
        ),
        "expr" => Derivation::Expr(expression(
            index,
            m.get("expression")
                .ok_or_else(|| at("a computed slot carries no expression"))?,
            iri,
        )?),
        other => return Err(at(&format!("kind {other:?} is not one this broker runs"))),
    };
    if source.is_none() && !matches!(derivation, Derivation::Constant(_) | Derivation::Expr(_)) {
        return Err(at("names no source"));
    }
    let filterable = match m.get("filterable") {
        None => false,
        Some(f) => f.as_bool().ok_or_else(|| at("filterable is a boolean"))?,
    };
    Ok((target, source, derivation, filterable))
}

/// One expression node of the IR (DM-52).
fn expression(index: usize, node: &Value, iri: Expand<'_>) -> Result<Expression, String> {
    let at = |reason: &str| format!("mapping: slot {index}: {reason}");
    let m = node
        .as_object()
        .ok_or_else(|| at("an expression node is an object"))?;
    let side = |key: &str| -> Result<Box<Expression>, String> {
        let n = m
            .get(key)
            .ok_or_else(|| at(&format!("an operator node carries {key:?}")))?;
        expression(index, n, iri).map(Box::new)
    };
    let text = |key: &str| m.get(key).and_then(Value::as_str);
    if let Some(name) = m.get("slot") {
        return iri(&format!("slot {index} expression"), Some(name)).map(Expression::Slot);
    }
    if let Some(v) = m.get("const") {
        return Ok(Expression::Constant(v.clone()));
    }
    if let Some(op) = text("binary") {
        let op = match op {
            "+" => Arithmetic::Add,
            "-" => Arithmetic::Subtract,
            "*" => Arithmetic::Multiply,
            "/" => Arithmetic::Divide,
            other => return Err(at(&format!("{other:?} is not an arithmetic operator"))),
        };
        return Ok(Expression::Arithmetic(op, side("left")?, side("right")?));
    }
    if let Some(op) = text("compare") {
        let op = match op {
            "==" => Comparison::Equal,
            "!=" => Comparison::NotEqual,
            "<" => Comparison::Less,
            "<=" => Comparison::LessOrEqual,
            ">" => Comparison::Greater,
            ">=" => Comparison::GreaterOrEqual,
            other => return Err(at(&format!("{other:?} is not a comparison operator"))),
        };
        return Ok(Expression::Comparison(op, side("left")?, side("right")?));
    }
    if let Some(op) = text("boolean") {
        let all = match op {
            "and" => true,
            "or" => false,
            other => return Err(at(&format!("{other:?} is not a boolean operator"))),
        };
        let operands = m
            .get("operands")
            .and_then(Value::as_array)
            .filter(|o| !o.is_empty())
            .ok_or_else(|| at("a boolean node lists its operands"))?
            .iter()
            .map(|o| expression(index, o, iri))
            .collect::<Result<_, _>>()?;
        return Ok(Expression::Junction { all, operands });
    }
    if let Some(op) = text("unary") {
        let operand = side("operand")?;
        return match op {
            "-" => Ok(Expression::Negate(operand)),
            "not" => Ok(Expression::Not(operand)),
            other => Err(at(&format!("{other:?} is not a unary operator"))),
        };
    }
    Err(at("an expression node names no form this broker evaluates"))
}

#[cfg(test)]
mod tests;
