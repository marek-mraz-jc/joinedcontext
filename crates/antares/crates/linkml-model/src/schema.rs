// SPDX-License-Identifier: EUPL-1.2
//! The LinkML subset as plain data (odata-linkml-mapping.md section 9): the
//! metaslots the surfaces read, in the shape of LinkML's JSON serialisation.
//! Every struct denies unknown fields, so a metaslot outside the subset fails
//! the load instead of being dropped (threat LK4). [`normalize`] first drops the
//! listed non-semantic metaslots a LinkML generator writes, so real `gen-linkml`
//! output loads (T-5207, mapping-contract rule 2).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value};

/// A schema: `id`, `name`, `version`, `prefixes`, `default_prefix`, and its
/// classes, slots and enums keyed by name. The two declarations the subset
/// implies, [`IMPORTS`] and [`DEFAULT_RANGE`], are read when they are
/// exactly those and refused otherwise (LK4); the writer declares them, so
/// a LinkML tool reads the schema the broker means.
///
/// ```
/// let s: linkml_model::Schema =
///     serde_json::from_str(r#"{"id": "https://example.org/s", "name": "s"}"#).unwrap();
/// assert!(s.classes.is_empty());
/// let declared: Result<linkml_model::Schema, _> = serde_json::from_str(
///     r#"{"id": "https://e.org/s", "name": "s", "imports": ["linkml:types"], "default_range": "string"}"#);
/// assert!(declared.is_ok());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawSchema")]
pub struct Schema {
    /// The schema IRI.
    pub id: String,
    /// The schema name.
    pub name: String,
    /// The schema version.
    #[serde(default)]
    pub version: Option<String>,
    /// CURIE prefixes, keyed by prefix.
    #[serde(default)]
    pub prefixes: BTreeMap<String, Prefix>,
    /// The prefix of every element without its own URI.
    #[serde(default)]
    pub default_prefix: Option<String>,
    /// Classes, keyed by name.
    #[serde(default)]
    pub classes: BTreeMap<String, Class>,
    /// Slots, keyed by name.
    #[serde(default)]
    pub slots: BTreeMap<String, Slot>,
    /// Enums, keyed by name.
    #[serde(default)]
    pub enums: BTreeMap<String, Enum>,
}

/// The built-in types every range outside the schema's own names resolves
/// to: the one import the subset reads.
pub const IMPORTS: &str = "linkml:types";

/// The range of a slot that names none.
pub const DEFAULT_RANGE: &str = "string";

/// [`Schema`] as read, with the two declarations the subset implies.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSchema {
    /// The schema IRI.
    pub id: String,
    /// The schema name.
    pub name: String,
    /// The schema version.
    #[serde(default)]
    pub version: Option<String>,
    /// CURIE prefixes, keyed by prefix.
    #[serde(default)]
    pub prefixes: BTreeMap<String, Prefix>,
    /// The prefix of every element without its own URI.
    #[serde(default)]
    pub default_prefix: Option<String>,
    /// Classes, keyed by name.
    #[serde(default)]
    pub classes: BTreeMap<String, Class>,
    /// Slots, keyed by name.
    #[serde(default)]
    pub slots: BTreeMap<String, Slot>,
    /// Enums, keyed by name.
    #[serde(default)]
    pub enums: BTreeMap<String, Enum>,
    #[serde(default)]
    imports: Option<serde_json::Value>,
    #[serde(default)]
    default_range: Option<serde_json::Value>,
}

impl TryFrom<RawSchema> for Schema {
    type Error = String;

    fn try_from(raw: RawSchema) -> Result<Self, String> {
        // worded as serde words an unknown field, which the load reports
        // as outside the subset (`Error::NotImplemented`)
        if raw
            .imports
            .is_some_and(|i| i != serde_json::json!([IMPORTS]))
        {
            return Err(format!(
                "unknown field `imports`: the subset reads [{IMPORTS:?}] alone"
            ));
        }
        if raw.default_range.is_some_and(|r| r != DEFAULT_RANGE) {
            return Err(format!(
                "unknown field `default_range`: the subset reads {DEFAULT_RANGE:?} alone"
            ));
        }
        Ok(Self {
            id: raw.id,
            name: raw.name,
            version: raw.version,
            prefixes: raw.prefixes,
            default_prefix: raw.default_prefix,
            classes: raw.classes,
            slots: raw.slots,
            enums: raw.enums,
        })
    }
}

/// One prefix in LinkML's JSON form.
///
/// ```
/// let p: linkml_model::Prefix = serde_json::from_str(
///     r#"{"prefix_prefix": "schema", "prefix_reference": "https://schema.org/"}"#,
/// ).unwrap();
/// assert_eq!(p.prefix_reference, "https://schema.org/");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prefix {
    /// The prefix; equal to its key.
    pub prefix_prefix: String,
    /// The IRI the prefix stands for.
    pub prefix_reference: String,
}

/// A class: `class_uri`, `is_a`, `slots`, `slot_usage`, `description`.
///
/// ```
/// let c: linkml_model::Class =
///     serde_json::from_str(r#"{"is_a": "Thing", "slots": ["name"]}"#).unwrap();
/// assert_eq!(c.is_a.as_deref(), Some("Thing"));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Class {
    /// The class name; equal to its key when present.
    #[serde(default)]
    pub name: Option<String>,
    /// The class IRI or CURIE.
    #[serde(default)]
    pub class_uri: Option<String>,
    /// The parent class.
    #[serde(default)]
    pub is_a: Option<String>,
    /// The slots the class declares, in order.
    #[serde(default)]
    pub slots: Vec<String>,
    /// Per-class refinements of slots, keyed by slot name.
    #[serde(default)]
    pub slot_usage: BTreeMap<String, Slot>,
    /// Tagged values, keyed by tag; `odata_insertable`, `odata_updatable`
    /// and `odata_deletable` are read (T-3722).
    #[serde(default)]
    pub annotations: BTreeMap<String, Annotation>,
    /// Free text.
    #[serde(default)]
    pub description: Option<String>,
}

/// A slot, or a `slot_usage` refinement of one: every metaslot optional.
///
/// ```
/// let s: linkml_model::Slot =
///     serde_json::from_str(r#"{"range": "integer", "required": true}"#).unwrap();
/// assert_eq!(s.required, Some(true));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    /// The slot name; equal to its key when present.
    #[serde(default)]
    pub name: Option<String>,
    /// The slot IRI or CURIE.
    #[serde(default)]
    pub slot_uri: Option<String>,
    /// A class, an enum or a built-in type; `string` when absent.
    #[serde(default)]
    pub range: Option<String>,
    /// Whether every instance carries it.
    #[serde(default)]
    pub required: Option<bool>,
    /// Whether it holds several values.
    #[serde(default)]
    pub multivalued: Option<bool>,
    /// Whether its value identifies the instance among its class's (T-3720).
    #[serde(default)]
    pub identifier: Option<bool>,
    /// Whether a class-ranged value is nested instead of referenced.
    #[serde(default)]
    pub inlined: Option<bool>,
    /// The slot that states the same relation from the other end.
    #[serde(default)]
    pub inverse: Option<String>,
    /// The unit of a quantity.
    #[serde(default)]
    pub unit: Option<Unit>,
    /// Tagged values, keyed by tag; `ngsi_ld_kind` (DM-05), `edm_type` (T-3720),
    /// `odata_computed` and `odata_immutable` (T-3722) are read, and `summary`,
    /// `label` (JSON text of a language map) and `hidden` (DM-99…DM-101, T-4672).
    #[serde(default)]
    pub annotations: BTreeMap<String, Annotation>,
    /// Free text.
    #[serde(default)]
    pub description: Option<String>,
    /// Where the slot stands in its class's display order, lower first (DM-98).
    #[serde(default)]
    pub rank: Option<i64>,
    /// The slot's human-readable name.
    #[serde(default)]
    pub title: Option<String>,
}

/// A unit: `ucum_code`, `symbol`, `exact_mappings` (a `unece:` CURIE among them).
///
/// ```
/// let u: linkml_model::Unit = serde_json::from_str(r#"{"ucum_code": "Cel"}"#).unwrap();
/// assert_eq!(u.ucum_code.as_deref(), Some("Cel"));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    /// The UCUM code.
    #[serde(default)]
    pub ucum_code: Option<String>,
    /// The printed symbol.
    #[serde(default)]
    pub symbol: Option<String>,
    /// Equivalent unit IRIs or CURIEs.
    #[serde(default)]
    pub exact_mappings: Vec<String>,
}

/// One annotation in LinkML's JSON form.
///
/// ```
/// let a: linkml_model::Annotation =
///     serde_json::from_str(r#"{"tag": "ngsi_ld_kind", "value": "Property"}"#).unwrap();
/// assert_eq!(a.value, "Property");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Annotation {
    /// The tag; equal to its key.
    pub tag: String,
    /// The value, a string in this subset.
    pub value: String,
}

/// An enum: `description` and `permissible_values`.
///
/// ```
/// let e: linkml_model::Enum =
///     serde_json::from_str(r#"{"permissible_values": {"red": {}}}"#).unwrap();
/// assert!(e.permissible_values.contains_key("red"));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enum {
    /// The enum name; equal to its key when present.
    #[serde(default)]
    pub name: Option<String>,
    /// Free text.
    #[serde(default)]
    pub description: Option<String>,
    /// The values, keyed by text.
    #[serde(default)]
    pub permissible_values: BTreeMap<String, PermissibleValue>,
}

/// One permissible value: `text`, `description`, `meaning` and its `annotations`, such as the
/// `label` that names it in each language (T-4803).
///
/// ```
/// let v: linkml_model::PermissibleValue =
///     serde_json::from_str(r#"{"meaning": "https://example.org/Red"}"#).unwrap();
/// assert!(v.text.is_none());
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissibleValue {
    /// The value; equal to its key when present.
    #[serde(default)]
    pub text: Option<String>,
    /// Free text.
    #[serde(default)]
    pub description: Option<String>,
    /// The IRI or CURIE the value stands for.
    #[serde(default)]
    pub meaning: Option<String>,
    /// Tagged values, keyed by tag.
    #[serde(default)]
    pub annotations: BTreeMap<String, Annotation>,
}

/// Metaslots that change no meaning a surface reads: provenance, generator
/// stamps, mappings to other vocabularies, examples and editorial text
/// (T-5207). [`normalize`] drops them; every other unknown metaslot is refused.
pub const NON_SEMANTIC: [&str; 31] = [
    "@type",
    "aliases",
    "broad_mappings",
    "close_mappings",
    "comments",
    "contributors",
    "created_by",
    "created_on",
    "deprecated",
    "domain_of",
    "examples",
    "exact_mappings",
    "from_schema",
    "generation_date",
    "imported_from",
    "in_subset",
    "keywords",
    "last_updated_on",
    "metamodel_version",
    "modified_by",
    "narrow_mappings",
    "notes",
    "owner",
    "rank",
    "related_mappings",
    "see_also",
    "source_file",
    "source_file_date",
    "source_file_size",
    "title",
    "todos",
];

/// The built-in types of `linkml:types` with their URIs: a schema's `types`
/// holding exactly these (or some of them) is the import a generator merged in.
const MERGED_TYPES: [(&str, &str); 19] = [
    ("string", "xsd:string"),
    ("integer", "xsd:integer"),
    ("boolean", "xsd:boolean"),
    ("float", "xsd:float"),
    ("double", "xsd:double"),
    ("decimal", "xsd:decimal"),
    ("time", "xsd:time"),
    ("date", "xsd:date"),
    ("datetime", "xsd:dateTime"),
    ("date_or_datetime", "linkml:DateOrDatetime"),
    ("uriorcurie", "xsd:anyURI"),
    ("curie", "xsd:string"),
    ("uri", "xsd:anyURI"),
    ("ncname", "xsd:string"),
    ("objectidentifier", "shex:iri"),
    ("nodeidentifier", "shex:nonLiteral"),
    ("jsonpointer", "xsd:string"),
    ("jsonpath", "xsd:string"),
    ("sparqlpath", "xsd:string"),
];

/// Brings generator output to the subset's shape before the strict parse
/// (T-5207): drops [`NON_SEMANTIC`] metaslots where the element does not read
/// them, the merged `linkml:types` copy, a class's `attributes` when each one
/// repeats a slot the class or an ancestor lists (the generator's induced
/// copy), and writes a
/// scalar annotation (`odata_computed: true`, or the YAML shorthand) as a
/// `{tag, value}` string. Anything else stays, so a meaning-changing metaslot
/// (`any_of`, `mixins`, an own type) is still refused by name.
///
/// ```
/// let mut v = serde_json::json!({"id": "https://e.org/s", "name": "s", "@type": "SchemaDefinition",
///   "slots": {"n": {"from_schema": "x", "annotations": {"odata_computed": {"tag": "odata_computed", "value": true}}}}});
/// linkml_model::normalize(&mut v);
/// let s: linkml_model::Schema = serde_json::from_value(v).unwrap();
/// assert_eq!(s.slots["n"].annotations["odata_computed"].value, "true");
/// ```
pub fn normalize(schema: &mut Value) {
    let Some(top) = schema.as_object_mut() else {
        return;
    };
    strip(top, &[]);
    if top.get("types").is_some_and(merged_types) {
        top.remove("types");
    }
    let listed = inherited_slots(top);
    for (name, class) in members(top, "classes") {
        strip(class, &[]);
        annotations(class);
        let own = listed.get(name.as_str());
        let induced = class
            .get("attributes")
            .and_then(Value::as_object)
            .is_some_and(|a| a.keys().all(|k| own.is_some_and(|l| l.contains(k))));
        if induced {
            class.remove("attributes");
        }
        for (_, usage) in members(class, "slot_usage") {
            slot(usage);
        }
    }
    for (_, s) in members(top, "slots") {
        slot(s);
    }
    for (_, e) in members(top, "enums") {
        strip(e, &[]);
        for (_, pv) in members(e, "permissible_values") {
            strip(pv, &[]);
            annotations(pv);
        }
    }
}

/// Each class's slots with its ancestors' (by `is_a`, at most 64 deep), as the
/// raw document lists them.
fn inherited_slots(top: &Map<String, Value>) -> BTreeMap<String, Vec<String>> {
    let classes = top.get("classes").and_then(Value::as_object);
    let field = |c: &str, key: &str| {
        classes
            .and_then(|m| m.get(c))
            .and_then(|c| c.get(key))
            .cloned()
    };
    classes
        .into_iter()
        .flat_map(|m| m.keys())
        .map(|name| {
            let mut all = Vec::new();
            let mut at = Some(name.clone());
            for _ in 0..64 {
                let Some(c) = at else { break };
                if let Some(Value::Array(slots)) = field(&c, "slots") {
                    all.extend(slots.iter().filter_map(|s| s.as_str().map(str::to_owned)));
                }
                at = field(&c, "is_a").and_then(|p| p.as_str().map(str::to_owned));
            }
            (name.clone(), all)
        })
        .collect()
}

fn slot(s: &mut Map<String, Value>) {
    strip(s, &["rank", "title"]);
    annotations(s);
}

fn members<'a>(
    m: &'a mut Map<String, Value>,
    key: &str,
) -> impl Iterator<Item = (&'a String, &'a mut Map<String, Value>)> {
    m.get_mut(key)
        .and_then(Value::as_object_mut)
        .into_iter()
        .flat_map(|o| o.iter_mut())
        .filter_map(|(k, v)| v.as_object_mut().map(|v| (k, v)))
}

fn strip(m: &mut Map<String, Value>, reads: &[&str]) {
    m.retain(|k, _| !NON_SEMANTIC.contains(&k.as_str()) || reads.contains(&k.as_str()));
}

fn merged_types(types: &Value) -> bool {
    types.as_object().is_some_and(|t| {
        t.iter().all(|(name, def)| {
            MERGED_TYPES
                .iter()
                .any(|(n, uri)| n == name && def.get("uri").and_then(Value::as_str) == Some(*uri))
        })
    })
}

fn annotations(m: &mut Map<String, Value>) {
    for (tag, a) in m
        .get_mut("annotations")
        .and_then(Value::as_object_mut)
        .into_iter()
        .flat_map(|o| o.iter_mut())
    {
        let mut o = match a.take() {
            Value::Object(o) => o,
            scalar => Map::from_iter([
                ("tag".to_owned(), Value::String(tag.clone())),
                ("value".to_owned(), scalar),
            ]),
        };
        o.remove("@type");
        if let Some(v @ (Value::Bool(_) | Value::Number(_))) = o.get("value").cloned() {
            o.insert("value".to_owned(), Value::String(v.to_string()));
        }
        *a = Value::Object(o);
    }
}
