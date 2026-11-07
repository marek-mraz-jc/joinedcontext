// SPDX-License-Identifier: EUPL-1.2
//! A [`Schema`] that passed the load checks, and what the writers ask of it:
//! IRIs, ranges, NGSI-LD kinds and the slots of a class with `is_a` and
//! `slot_usage` folded in.

use std::collections::BTreeMap;

use crate::schema::{Schema, Slot};
use crate::Error;

/// The caller's bounds on one load (ADR-0024 decision 4): no hidden default.
///
/// ```
/// let limits = linkml_model::Limits { max_bytes: 1 << 20, max_depth: 32, max_elements: 10_000 };
/// assert_eq!(limits.max_depth, 32);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Limits {
    /// Size of the JSON document, checked before it is parsed.
    pub max_bytes: usize,
    /// Brace and bracket nesting, scanned before it is parsed.
    pub max_depth: usize,
    /// Prefixes, classes, slots, enums, slot references, slot usages,
    /// annotations, unit mappings and permissible values, together.
    pub max_elements: usize,
}

/// The bounds a surface reads a stored schema under (threat LK7): the LinkML
/// surface's, and the first the `OData` surface tries, so the two share one
/// parse of a schema inside them ([`crate::cache`], T-4354).
pub const SURFACE_LIMITS: Limits = Limits {
    max_bytes: 1024 * 1024,
    max_depth: 32,
    max_elements: 20_000,
};

/// The LinkML built-in types the subset maps.
///
/// ```
/// use linkml_model::Builtin;
/// assert_eq!(Builtin::from_name("datetime"), Some(Builtin::Datetime));
/// assert_eq!(Builtin::Datetime.xsd(), "xsd:dateTime");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    /// `string`
    String,
    /// `integer`
    Integer,
    /// `float`
    Float,
    /// `double`
    Double,
    /// `decimal`
    Decimal,
    /// `boolean`
    Boolean,
    /// `date`
    Date,
    /// `datetime`
    Datetime,
    /// `time`
    Time,
    /// `uri`
    Uri,
    /// `uriorcurie`
    Uriorcurie,
}

const BUILTINS: [Builtin; 11] = [
    Builtin::String,
    Builtin::Integer,
    Builtin::Float,
    Builtin::Double,
    Builtin::Decimal,
    Builtin::Boolean,
    Builtin::Date,
    Builtin::Datetime,
    Builtin::Time,
    Builtin::Uri,
    Builtin::Uriorcurie,
];

impl Builtin {
    /// The type for a LinkML range name, if it is one of the subset's.
    ///
    /// ```
    /// assert!(linkml_model::Builtin::from_name("ncname").is_none());
    /// ```
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        BUILTINS.into_iter().find(|b| b.name() == name)
    }

    /// The LinkML name.
    ///
    /// ```
    /// assert_eq!(linkml_model::Builtin::Uriorcurie.name(), "uriorcurie");
    /// ```
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Float => "float",
            Self::Double => "double",
            Self::Decimal => "decimal",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::Datetime => "datetime",
            Self::Time => "time",
            Self::Uri => "uri",
            Self::Uriorcurie => "uriorcurie",
        }
    }

    /// The XML Schema datatype, as a CURIE on `xsd:`.
    ///
    /// ```
    /// assert_eq!(linkml_model::Builtin::Uri.xsd(), "xsd:anyURI");
    /// ```
    #[must_use]
    pub fn xsd(self) -> &'static str {
        match self {
            Self::String => "xsd:string",
            Self::Integer => "xsd:integer",
            Self::Float => "xsd:float",
            Self::Double => "xsd:double",
            Self::Decimal => "xsd:decimal",
            Self::Boolean => "xsd:boolean",
            Self::Date => "xsd:date",
            Self::Datetime => "xsd:dateTime",
            Self::Time => "xsd:time",
            Self::Uri | Self::Uriorcurie => "xsd:anyURI",
        }
    }
}

/// What a slot's `range` names.
///
/// ```
/// use linkml_model::{Builtin, Range, Slot};
/// let m = linkml_model::Model::read(
///     br#"{"id": "https://example.org/s", "name": "s", "slots": {"n": {}}}"#,
///     &linkml_model::Limits { max_bytes: 4096, max_depth: 8, max_elements: 8 },
/// ).unwrap();
/// assert_eq!(m.range(&Slot::default()), Range::Type(Builtin::String));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range<'a> {
    /// A built-in type.
    Type(Builtin),
    /// A class of the schema: the slot is a relationship.
    Class(&'a str),
    /// An enum of the schema.
    Enum(&'a str),
}

/// The NGSI-LD attribute kinds the surfaces map (DM-05). An `ngsi_ld_kind`
/// naming another kind (one a later NGSI-LD version adds) degrades that slot
/// alone, see [`known_kind`] (T-5207).
pub const KINDS: [&str; 8] = [
    "Property",
    "Relationship",
    "GeoProperty",
    "LanguageProperty",
    "VocabProperty",
    "ListProperty",
    "ListRelationship",
    "JsonProperty",
];

/// Whether the surfaces map `kind` (one of [`KINDS`]). A slot of another kind
/// is served as an untyped, read-only, unfiltered value with its kind stated,
/// and the rest of the schema is unaffected (T-5207, mapping-contract rule 4).
///
/// ```
/// assert!(linkml_model::known_kind("VocabProperty"));
/// assert!(!linkml_model::known_kind("TimeSeriesProperty"));
/// ```
#[must_use]
pub fn known_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

/// The annotation tag that states a slot's NGSI-LD kind (DM-05).
pub const KIND_TAG: &str = "ngsi_ld_kind";

/// The annotation tag that narrows a slot's Edm type where the LinkML subset
/// has no type of its own (T-3720, T-3722): see [`edm_types`].
pub const EDM_TYPE_TAG: &str = "edm_type";

/// The annotation tags whose value is `"true"` or `"false"` (T-3722): a
/// slot's `odata_computed`, `odata_immutable`; a class's
/// `odata_insertable`, `odata_updatable`, `odata_deletable` and
/// [`QB_DSD_TAG`].
pub const FLAG_TAGS: [&str; 6] = [
    "odata_computed",
    "odata_immutable",
    "odata_insertable",
    "odata_updatable",
    "odata_deletable",
    QB_DSD_TAG,
];

/// The class annotation that makes a class an RDF Data Cube Data Structure
/// Definition (DM-60): `"true"`, and its observations are its Entities.
pub const QB_DSD_TAG: &str = "qb_dsd";

/// The slot annotation that names a slot's role in a Data Structure
/// Definition (DM-60): `dimension` or `measure`.
pub const QB_COMPONENT_TAG: &str = "qb_component";

/// The Edm types an `edm_type` annotation may name on a slot of range `b`.
///
/// ```
/// use linkml_model::{edm_types, Builtin};
/// assert!(edm_types(Builtin::Integer).contains(&"Edm.Int16"));
/// assert!(edm_types(Builtin::Boolean).is_empty());
/// ```
#[must_use]
pub fn edm_types(b: Builtin) -> &'static [&'static str] {
    match b {
        Builtin::Integer => &["Edm.Byte", "Edm.SByte", "Edm.Int16", "Edm.Int32"],
        Builtin::Float | Builtin::Double => &["Edm.Single"],
        Builtin::String => &["Edm.Guid", "Edm.Duration", "Edm.Binary"],
        _ => &[],
    }
}

/// A schema that passed every load check: names, references, ranges, IRIs,
/// limits. The writers render only a `Model`, so none of them can fail.
///
/// ```
/// use linkml_model::{Limits, Model};
/// let json = br#"{"id": "https://example.org/s", "name": "s",
///   "classes": {"Room": {"slots": ["temperature"]}},
///   "slots": {"temperature": {"range": "float"}}}"#;
/// let m = Model::read(json, &Limits { max_bytes: 4096, max_depth: 8, max_elements: 64 }).unwrap();
/// assert_eq!(m.class_iri("Room"), "https://example.org/s/Room");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    schema: Schema,
}

impl Model {
    /// Reads resolved LinkML JSON: size and nesting checked before the parse,
    /// unknown metaslots refused, then every check of [`Model::new`].
    ///
    /// # Errors
    ///
    /// [`Error::LimitExceeded`] past a bound, [`Error::NotImplemented`] for a
    /// metaslot or range outside the subset, [`Error::Malformed`] otherwise.
    ///
    /// ```
    /// use linkml_model::{Error, Limits, Model};
    /// let limits = Limits { max_bytes: 4096, max_depth: 8, max_elements: 8 };
    /// let err = Model::read(br#"{"id": "https://e.org/s", "name": "s", "imports": []}"#, &limits);
    /// assert!(matches!(err, Err(Error::NotImplemented(_))));
    /// ```
    pub fn read(json: &[u8], limits: &Limits) -> Result<Self, Error> {
        if json.len() > limits.max_bytes {
            return Err(Error::LimitExceeded {
                limit: "max_bytes",
                max: limits.max_bytes,
            });
        }
        if json_depth(json) > limits.max_depth {
            return Err(Error::LimitExceeded {
                limit: "max_depth",
                max: limits.max_depth,
            });
        }
        let mut value: serde_json::Value =
            serde_json::from_slice(json).map_err(|e| Error::Malformed(e.to_string()))?;
        crate::schema::normalize(&mut value);
        let schema = serde_json::from_value(value).map_err(|e| {
            // serde names an unknown field this way (`de::Error::unknown_field`)
            // and nothing else; it is LinkML outside the subset, not broken JSON.
            let text = e.to_string();
            if text.starts_with("unknown field") {
                Error::NotImplemented(text)
            } else {
                Error::Malformed(text)
            }
        })?;
        Self::new(schema, limits)
    }

    /// Checks a schema built in memory (a view's narrowed copy) as a load would.
    ///
    /// # Errors
    ///
    /// As [`Model::read`], without the byte and depth bounds.
    ///
    /// ```
    /// use linkml_model::{Limits, Model, Schema};
    /// let s: Schema = serde_json::from_str(r#"{"id": "https://e.org/s", "name": "s"}"#).unwrap();
    /// assert!(Model::new(s, &Limits { max_bytes: 0, max_depth: 0, max_elements: 0 }).is_ok());
    /// ```
    pub fn new(schema: Schema, limits: &Limits) -> Result<Self, Error> {
        count(&schema, limits)?;
        validate(&schema)?;
        Ok(Self { schema })
    }

    /// The schema as loaded.
    ///
    /// ```
    /// # let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "s"}"#,
    /// #     &linkml_model::Limits { max_bytes: 99, max_depth: 2, max_elements: 0 }).unwrap();
    /// assert_eq!(m.schema().name, "s");
    /// ```
    #[must_use]
    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// The namespace of every element without its own URI: the default
    /// prefix's IRI, else the schema `id` followed by `/`.
    ///
    /// ```
    /// # let m = linkml_model::Model::read(br#"{"id": "https://e.org/s#", "name": "s"}"#,
    /// #     &linkml_model::Limits { max_bytes: 99, max_depth: 2, max_elements: 0 }).unwrap();
    /// assert_eq!(m.namespace(), "https://e.org/s#");
    /// ```
    #[must_use]
    pub fn namespace(&self) -> String {
        namespace(&self.schema)
    }

    /// A CURIE, an absolute IRI or a bare name, as an absolute IRI.
    ///
    /// ```
    /// # let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "s",
    /// #   "prefixes": {"schema": {"prefix_prefix": "schema", "prefix_reference": "https://schema.org/"}}}"#,
    /// #     &linkml_model::Limits { max_bytes: 999, max_depth: 4, max_elements: 9 }).unwrap();
    /// assert_eq!(m.expand("schema:name"), "https://schema.org/name");
    /// assert_eq!(m.expand("urn:x:y"), "urn:x:y");
    /// assert_eq!(m.expand("not one"), "not one");
    /// ```
    ///
    /// A value that expands to no IRI comes back as it is. The writers pass
    /// only values the load expanded, so they never print one.
    #[must_use]
    pub fn expand(&self, value: &str) -> String {
        expand(&self.schema, value).unwrap_or_else(|| value.to_owned())
    }

    /// The IRI of a class: its `class_uri`, else the namespace and its name.
    ///
    /// ```
    /// # let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "s",
    /// #   "classes": {"Room": {"class_uri": "https://e.org/Room"}}}"#,
    /// #     &linkml_model::Limits { max_bytes: 999, max_depth: 4, max_elements: 9 }).unwrap();
    /// assert_eq!(m.class_iri("Room"), "https://e.org/Room");
    /// ```
    #[must_use]
    pub fn class_iri(&self, name: &str) -> String {
        let uri = self
            .schema
            .classes
            .get(name)
            .and_then(|c| c.class_uri.as_deref());
        self.expand(uri.unwrap_or(name))
    }

    /// The IRI of a slot: its `slot_uri`, else the namespace and its name.
    ///
    /// ```
    /// # let m = linkml_model::Model::read(br#"{"id": "https://e.org/s/", "name": "s",
    /// #   "slots": {"name": {}}}"#,
    /// #     &linkml_model::Limits { max_bytes: 999, max_depth: 4, max_elements: 9 }).unwrap();
    /// assert_eq!(m.slot_iri("name"), "https://e.org/s/name");
    /// ```
    #[must_use]
    pub fn slot_iri(&self, name: &str) -> String {
        let uri = self
            .schema
            .slots
            .get(name)
            .and_then(|s| s.slot_uri.as_deref());
        self.expand(uri.unwrap_or(name))
    }

    /// The same model with the IRI `class` gives a class and `slot` gives a slot, by name and
    /// own IRI, in place of its own; a name either answers `None` for keeps its IRI. For a
    /// reader that knows the schema's names under other IRIs, a view's among them.
    ///
    /// ```
    /// # let m = linkml_model::Model::read(br#"{"id": "https://e.org/s/", "name": "s",
    /// #   "classes": {"Room": {}}, "slots": {"name": {}}}"#,
    /// #     &linkml_model::Limits { max_bytes: 999, max_depth: 4, max_elements: 9 }).unwrap();
    /// let r = m.renamed(|c, _| Some(format!("https://o.org/{c}")), |_, _| None);
    /// assert_eq!(r.class_iri("Room"), "https://o.org/Room");
    /// assert_eq!(r.slot_iri("name"), "https://e.org/s/name");
    /// ```
    #[must_use]
    pub fn renamed(
        &self,
        class: impl Fn(&str, &str) -> Option<String>,
        slot: impl Fn(&str, &str) -> Option<String>,
    ) -> Self {
        let mut schema = self.schema.clone();
        for (name, c) in &mut schema.classes {
            if let Some(iri) = class(name, &self.class_iri(name)) {
                c.class_uri = Some(iri);
            }
        }
        for (name, s) in &mut schema.slots {
            if let Some(iri) = slot(name, &self.slot_iri(name)) {
                s.slot_uri = Some(iri);
            }
        }
        Self { schema }
    }

    /// The model under the IRIs `shown` (type IRI to Attribute IRIs) knows its classes and
    /// slots by (T-3752): a class or slot whose own IRI `shown` lacks, but whose name under
    /// `expand` it holds, takes that IRI. A reader whose names are short names read under
    /// another @context than the schema's vocabulary (a broker view's, ADR-0020) then still
    /// finds them.
    ///
    /// ```
    /// # use std::collections::{BTreeMap, BTreeSet};
    /// let m = linkml_model::Model::read(br#"{"id": "https://e.org/s/", "name": "s",
    ///   "classes": {"Car": {"class_uri": "https://sdm.org/Car"}}, "slots": {"speed": {}}}"#,
    ///   &linkml_model::Limits { max_bytes: 999, max_depth: 4, max_elements: 9 }).unwrap();
    /// let shown = BTreeMap::from([("v:Car".to_owned(), BTreeSet::from(["v:speed".to_owned()]))]);
    /// let viewed = m.as_shown(&shown, |name| format!("v:{name}"));
    /// assert_eq!(viewed.class_iri("Car"), "v:Car");
    /// assert_eq!(viewed.slot_iri("speed"), "v:speed");
    /// ```
    #[must_use]
    pub fn as_shown(
        &self,
        shown: &BTreeMap<String, std::collections::BTreeSet<String>>,
        expand: impl Fn(&str) -> String,
    ) -> Self {
        let attrs: std::collections::BTreeSet<&String> = shown.values().flatten().collect();
        let by_name = |known: &dyn Fn(&String) -> bool, name: &str, own: &str| {
            let short = expand(name);
            (!known(&own.to_owned()) && known(&short)).then_some(short)
        };
        self.renamed(
            |name, own| by_name(&|iri| shown.contains_key(iri), name, own),
            |name, own| by_name(&|iri| attrs.contains(iri), name, own),
        )
    }

    /// What a slot's range names; `string` when it states none.
    ///
    /// ```
    /// # use linkml_model::{Range, Slot};
    /// # let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "s",
    /// #   "classes": {"Room": {}}}"#,
    /// #     &linkml_model::Limits { max_bytes: 999, max_depth: 4, max_elements: 9 }).unwrap();
    /// let s = Slot { range: Some("Room".into()), ..Slot::default() };
    /// assert_eq!(m.range(&s), Range::Class("Room"));
    /// ```
    #[must_use]
    pub fn range(&self, slot: &Slot) -> Range<'_> {
        range(&self.schema, slot.range.as_deref()).unwrap_or(Range::Type(Builtin::String))
    }

    /// The NGSI-LD kind (DM-05): the `ngsi_ld_kind` annotation, else
    /// `Relationship` for a class range not `inlined` and `Property` for any other: an
    /// inlined class is an object value, not a link (T-5206).
    ///
    /// ```
    /// # use linkml_model::Slot;
    /// # let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "s"}"#,
    /// #     &linkml_model::Limits { max_bytes: 999, max_depth: 4, max_elements: 9 }).unwrap();
    /// assert_eq!(m.kind(&Slot::default()), "Property");
    /// ```
    #[must_use]
    pub fn kind<'s>(&self, slot: &'s Slot) -> &'s str {
        match slot.annotations.get(KIND_TAG) {
            Some(a) => &a.value,
            None if matches!(self.range(slot), Range::Class(_)) && slot.inlined != Some(true) => {
                "Relationship"
            }
            None => "Property",
        }
    }

    /// The slots of a class with its ancestors' first, each with the
    /// `slot_usage` of its ancestors and then its own applied. Empty for a
    /// class the schema does not hold.
    ///
    /// ```
    /// # let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "s",
    /// #   "classes": {"Thing": {"slots": ["name"]},
    /// #               "Room": {"is_a": "Thing", "slots": ["size"],
    /// #                        "slot_usage": {"name": {"required": true}}}},
    /// #   "slots": {"name": {}, "size": {"range": "integer"}}}"#,
    /// #     &linkml_model::Limits { max_bytes: 999, max_depth: 8, max_elements: 99 }).unwrap();
    /// let slots = m.class_slots("Room");
    /// assert_eq!(slots[0].0, "name");
    /// assert_eq!(slots[0].1.required, Some(true));
    /// assert_eq!(slots[1].0, "size");
    /// ```
    #[must_use]
    pub fn class_slots(&self, class: &str) -> Vec<(&str, Slot)> {
        class_slots(&self.schema, class)
    }

    /// Every class IRI with the IRIs of every slot it has, inherited ones
    /// included: what a view of `*` shows (ADR-0023 rule 12), in the shape
    /// a surface intersects a view's narrowing with.
    #[must_use]
    pub fn slot_iris_by_class(&self) -> BTreeMap<String, std::collections::BTreeSet<String>> {
        self.schema
            .classes
            .keys()
            .map(|class| {
                let slots = self
                    .class_slots(class)
                    .iter()
                    .map(|(slot, _)| self.slot_iri(slot))
                    .collect();
                (self.class_iri(class), slots)
            })
            .collect()
    }
}

/// The class and its ancestors, root first; stops at a missing parent and
/// after as many steps as there are classes, so a cycle cannot loop.
fn lineage<'a>(schema: &'a Schema, class: &'a str) -> Vec<&'a str> {
    let mut chain = Vec::new();
    let mut next = Some(class);
    while let Some(name) = next {
        if chain.len() > schema.classes.len() {
            break;
        }
        let Some((key, c)) = schema.classes.get_key_value(name) else {
            break;
        };
        chain.push(key.as_str());
        next = c.is_a.as_deref();
    }
    chain.reverse();
    chain
}

fn class_slots<'a>(schema: &'a Schema, class: &str) -> Vec<(&'a str, Slot)> {
    let lineage = lineage(schema, class);
    let mut names: Vec<&str> = Vec::new();
    for c in lineage.iter().filter_map(|n| schema.classes.get(*n)) {
        for s in &c.slots {
            if !names.contains(&s.as_str()) {
                names.push(s);
            }
        }
    }
    names
        .into_iter()
        .filter_map(|name| {
            let (key, base) = schema.slots.get_key_value(name)?;
            let mut slot = base.clone();
            for usage in lineage
                .iter()
                .filter_map(|n| schema.classes.get(*n)?.slot_usage.get(name))
            {
                refine(&mut slot, usage);
            }
            Some((key.as_str(), slot))
        })
        .collect()
}

/// `slot_usage` over a slot: every metaslot the usage states wins.
fn refine(slot: &mut Slot, usage: &Slot) {
    fn over<T: Clone>(base: &mut Option<T>, usage: Option<&T>) {
        if let Some(v) = usage {
            *base = Some(v.clone());
        }
    }
    over(&mut slot.slot_uri, usage.slot_uri.as_ref());
    over(&mut slot.range, usage.range.as_ref());
    over(&mut slot.required, usage.required.as_ref());
    over(&mut slot.multivalued, usage.multivalued.as_ref());
    over(&mut slot.identifier, usage.identifier.as_ref());
    over(&mut slot.inlined, usage.inlined.as_ref());
    over(&mut slot.inverse, usage.inverse.as_ref());
    over(&mut slot.unit, usage.unit.as_ref());
    over(&mut slot.description, usage.description.as_ref());
    over(&mut slot.rank, usage.rank.as_ref());
    over(&mut slot.title, usage.title.as_ref());
    slot.annotations.extend(
        usage
            .annotations
            .iter()
            .map(|(k, v)| (k.clone(), v.clone())),
    );
}

fn range<'a>(schema: &'a Schema, name: Option<&str>) -> Option<Range<'a>> {
    let name = name.unwrap_or("string");
    if let Some((k, _)) = schema.classes.get_key_value(name) {
        return Some(Range::Class(k));
    }
    if let Some((k, _)) = schema.enums.get_key_value(name) {
        return Some(Range::Enum(k));
    }
    Builtin::from_name(name).map(Range::Type)
}

/// Brace and bracket nesting of a JSON byte stream, string-aware: a scan
/// before the parse (the broker's `bounds::json_depth`, which this crate may
/// not depend on, rule M1).
fn json_depth(bytes: &[u8]) -> usize {
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

fn count(schema: &Schema, limits: &Limits) -> Result<(), Error> {
    let slot_parts = |s: &Slot| {
        s.annotations
            .len()
            .saturating_add(s.unit.as_ref().map_or(0, |u| u.exact_mappings.len()))
    };
    let mut parts = vec![
        schema.prefixes.len(),
        schema.classes.len(),
        schema.slots.len(),
        schema.enums.len(),
    ];
    for c in schema.classes.values() {
        parts.push(c.slots.len());
        parts.push(c.slot_usage.len());
        parts.extend(c.slot_usage.values().map(slot_parts));
    }
    parts.extend(schema.slots.values().map(slot_parts));
    parts.extend(schema.enums.values().map(|e| e.permissible_values.len()));
    // Lengths of collections already in memory: their sum cannot reach
    // usize::MAX, and saturating keeps the bound exact up to it.
    let total = parts.into_iter().fold(0usize, usize::saturating_add);
    if total > limits.max_elements {
        return Err(Error::LimitExceeded {
            limit: "max_elements",
            max: limits.max_elements,
        });
    }
    Ok(())
}

fn malformed(why: String) -> Result<(), Error> {
    Err(Error::Malformed(why))
}

/// A name the writers may print: not empty, no control character. A
/// newline or an escape in a name is refused here, at load, not at render
/// (threat LK3).
fn check_name(what: &str, name: &str) -> Result<(), Error> {
    if name.is_empty() || name.chars().any(char::is_control) {
        return malformed(format!(
            "{what} {name:?}: a name is non-empty and holds no control character"
        ));
    }
    Ok(())
}

/// The optional `name` (or `text`, `prefix_prefix`, `tag`) inside an
/// element equals the key it is filed under.
fn check_key(what: &str, key: &str, inner: Option<&str>) -> Result<(), Error> {
    check_name(what, key)?;
    match inner {
        Some(n) if n != key => malformed(format!("{what} {key:?} names itself {n:?}")),
        _ => Ok(()),
    }
}

/// An absolute IRI as Turtle and JSON-LD print it between `<` and `>`: a
/// scheme, a colon, and no space, control character or `<>"{}|^`\`.
fn is_iri(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once(':') else {
        return false;
    };
    let mut s = scheme.chars();
    s.next().is_some_and(|c| c.is_ascii_alphabetic())
        && s.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.is_empty()
        && !value
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "<>\"{}|^`\\".contains(c))
}

fn namespace(schema: &Schema) -> String {
    let declared = schema
        .default_prefix
        .as_deref()
        .and_then(|p| schema.prefixes.get(p))
        .map(|p| p.prefix_reference.clone());
    declared.unwrap_or_else(|| {
        if schema.id.ends_with('/') || schema.id.ends_with('#') {
            schema.id.clone()
        } else {
            format!("{}/", schema.id)
        }
    })
}

fn expand(schema: &Schema, value: &str) -> Option<String> {
    let iri = match value.split_once(':') {
        Some((p, local)) => match schema.prefixes.get(p) {
            Some(prefix) => format!("{}{local}", prefix.prefix_reference),
            None => value.to_owned(),
        },
        None => format!("{}{value}", namespace(schema)),
    };
    is_iri(&iri).then_some(iri)
}

fn check_iri(schema: &Schema, what: &str, value: &str) -> Result<(), Error> {
    match expand(schema, value) {
        Some(_) => Ok(()),
        None => malformed(format!(
            "{what}: {value:?} is not an IRI, a CURIE on a declared prefix or an IRI-safe name"
        )),
    }
}

fn check_slot(schema: &Schema, what: &str, slot: &Slot) -> Result<(), Error> {
    if let Some(uri) = &slot.slot_uri {
        check_iri(schema, what, uri)?;
    }
    if let Some(r) = &slot.range {
        check_name(what, r)?;
        if range(schema, Some(r)).is_none() {
            return Err(Error::NotImplemented(format!(
                "{what}: range {r:?} is not a class, an enum or a built-in type of the subset"
            )));
        }
    }
    if let Some(inv) = &slot.inverse {
        if !schema.slots.contains_key(inv) {
            return malformed(format!("{what}: inverse {inv:?} is no slot"));
        }
    }
    if let Some(unit) = &slot.unit {
        for m in &unit.exact_mappings {
            check_iri(schema, what, m)?;
        }
    }
    for (tag, a) in &slot.annotations {
        check_key(&format!("{what} annotation"), tag, Some(&a.tag))?;
        if tag == KIND_TAG
            && !(known_kind(&a.value)
                || a.value.starts_with(|c: char| c.is_ascii_uppercase())
                    && a.value.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            return malformed(format!(
                "{what}: {KIND_TAG} {:?} is no NGSI-LD kind name",
                a.value
            ));
        }
        let allowed = match range(schema, slot.range.as_deref()) {
            Some(Range::Type(b)) => edm_types(b),
            _ => &[],
        };
        if tag == EDM_TYPE_TAG && !allowed.contains(&a.value.as_str()) {
            return malformed(format!(
                "{what}: {EDM_TYPE_TAG} {:?} is not one of {allowed:?} for its range",
                a.value
            ));
        }
        if tag == QB_COMPONENT_TAG && !matches!(a.value.as_str(), "dimension" | "measure") {
            return malformed(format!(
                "{what}: {QB_COMPONENT_TAG} {:?} is neither \"dimension\" nor \"measure\"",
                a.value
            ));
        }
        check_flag(what, tag, &a.value)?;
    }
    Ok(())
}

/// A tag of [`FLAG_TAGS`] holds `"true"` or `"false"`.
fn check_flag(what: &str, tag: &str, value: &str) -> Result<(), Error> {
    if FLAG_TAGS.contains(&tag) && !matches!(value, "true" | "false") {
        return malformed(format!(
            "{what}: {tag} {value:?} is neither \"true\" nor \"false\""
        ));
    }
    Ok(())
}

/// At most one identifier per class, ancestors included, and it is one
/// plain value: a single-valued `string`, `integer`, `uri` or `uriorcurie`
/// Property (T-3720).
fn check_identifier(schema: &Schema, what: &str, slots: &[(&str, Slot)]) -> Result<(), Error> {
    let ids: Vec<&(&str, Slot)> = slots
        .iter()
        .filter(|(_, s)| s.identifier == Some(true))
        .collect();
    if let [_, second, ..] = ids.as_slice() {
        return malformed(format!("{what}: a second identifier {:?}", second.0));
    }
    let Some((name, slot)) = ids.first() else {
        return Ok(());
    };
    let plain = matches!(
        range(schema, slot.range.as_deref()),
        Some(Range::Type(
            Builtin::String | Builtin::Integer | Builtin::Uri | Builtin::Uriorcurie
        ))
    );
    // A key is compared as its literal: a GUID or an integer, not a
    // duration or a binary.
    let keyable = slot
        .annotations
        .get(EDM_TYPE_TAG)
        .is_none_or(|a| !matches!(a.value.as_str(), "Edm.Duration" | "Edm.Binary"));
    let kind = slot
        .annotations
        .get(KIND_TAG)
        .map_or("Property", |a| a.value.as_str());
    if !plain || !keyable || slot.multivalued == Some(true) || kind != "Property" {
        return malformed(format!(
            "{what}: identifier {name:?} must be a single-valued string, integer, uri or uriorcurie Property"
        ));
    }
    Ok(())
}

fn validate(schema: &Schema) -> Result<(), Error> {
    check_name("schema name", &schema.name)?;
    if !is_iri(&schema.id) {
        return malformed(format!("schema id {:?} is not an absolute IRI", schema.id));
    }
    for (key, p) in &schema.prefixes {
        check_key("prefix", key, Some(&p.prefix_prefix))?;
        if key.contains(':') || !is_iri(&p.prefix_reference) {
            return malformed(format!(
                "prefix {key:?}: needs a colon-free name and an absolute IRI"
            ));
        }
    }
    if let Some(p) = &schema.default_prefix {
        if !schema.prefixes.contains_key(p) {
            return malformed(format!("default_prefix {p:?} is not declared"));
        }
    }
    let types: BTreeMap<&str, &str> = schema
        .classes
        .keys()
        .map(|k| (k.as_str(), "class"))
        .chain(schema.enums.keys().map(|k| (k.as_str(), "enum")))
        .collect();
    if types.len() != schema.classes.len().saturating_add(schema.enums.len()) {
        return malformed("a class and an enum share a name".to_owned());
    }
    if let Some(b) = types.keys().find(|k| Builtin::from_name(k).is_some()) {
        return malformed(format!(
            "{b:?} is a built-in type and cannot name a class or an enum"
        ));
    }
    if let Some(n) = schema
        .slots
        .keys()
        .find(|k| schema.classes.contains_key(*k))
    {
        return Err(Error::NotImplemented(format!(
            "{n:?} names a class and a slot: one JSON-LD term cannot stand for both"
        )));
    }
    for (key, slot) in &schema.slots {
        check_key("slot", key, slot.name.as_deref())?;
        check_iri(
            schema,
            &format!("slot {key:?}"),
            slot.slot_uri.as_deref().unwrap_or(key),
        )?;
        check_slot(schema, &format!("slot {key:?}"), slot)?;
    }
    for (key, class) in &schema.classes {
        check_key("class", key, class.name.as_deref())?;
        let what = format!("class {key:?}");
        check_iri(schema, &what, class.class_uri.as_deref().unwrap_or(key))?;
        if let Some(parent) = &class.is_a {
            if !schema.classes.contains_key(parent) {
                return malformed(format!("{what}: is_a {parent:?} is no class"));
            }
        }
        if lineage(schema, key).len() > schema.classes.len() {
            return malformed(format!("{what}: is_a forms a cycle"));
        }
        if let Some(s) = class.slots.iter().find(|s| !schema.slots.contains_key(*s)) {
            return malformed(format!("{what}: slot {s:?} is not declared"));
        }
        for (tag, a) in &class.annotations {
            check_key(&format!("{what} annotation"), tag, Some(&a.tag))?;
            check_flag(&what, tag, &a.value)?;
        }
        let own = class_slots(schema, key);
        for (name, usage) in &class.slot_usage {
            check_key(&format!("{what} slot_usage"), name, usage.name.as_deref())?;
            if !own.iter().any(|(n, _)| n == name) {
                return malformed(format!(
                    "{what}: slot_usage {name:?} is not one of its slots"
                ));
            }
            check_slot(schema, &format!("{what} slot_usage {name:?}"), usage)?;
        }
        check_identifier(schema, &what, &own)?;
    }
    for (key, e) in &schema.enums {
        check_key("enum", key, e.name.as_deref())?;
        for (text, v) in &e.permissible_values {
            let what = format!("enum {key:?} value");
            check_key(&what, text, v.text.as_deref())?;
            if let Some(m) = &v.meaning {
                check_iri(schema, &what, m)?;
            }
        }
    }
    Ok(())
}
