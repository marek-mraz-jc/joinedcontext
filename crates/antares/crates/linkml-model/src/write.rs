// SPDX-License-Identifier: EUPL-1.2
//! The writers for odata-linkml-mapping.md rows L2 to L6: same model, same
//! bytes. Maps are printed in key order, lists in schema order. Every name,
//! IRI and text the schema supplies is quoted and escaped for the format it
//! lands in, so no value can change a document's structure (threat LK3).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::{json, Map, Value};

use crate::model::{Builtin, Model, Range, QB_COMPONENT_TAG, QB_DSD_TAG};
use crate::schema::{Annotation, Slot, DEFAULT_RANGE, IMPORTS};

// ---------------------------------------------------------------- YAML (L2)

/// A YAML node of the emitted document; the shape is fixed, so its depth is.
enum Node {
    Str(String),
    Bool(bool),
    Int(i64),
    /// Never empty: an empty list is left out, as LinkML reads a missing
    /// list (the round-trip property test fails on a `null` here).
    List(Vec<String>),
    Map(Vec<(String, Node)>),
}

/// A double-quoted YAML scalar: `\`, `"`, control characters and the YAML
/// line breaks are escaped, nothing else can end or restructure it.
fn yaml_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn render(out: &mut String, indent: usize, map: &[(String, Node)]) {
    let pad = " ".repeat(indent);
    for (key, node) in map {
        let _ = write!(out, "{pad}{}:", yaml_quote(key));
        match node {
            Node::Str(s) => {
                let _ = writeln!(out, " {}", yaml_quote(s));
            }
            Node::Bool(b) => {
                let _ = writeln!(out, " {b}");
            }
            Node::Int(i) => {
                let _ = writeln!(out, " {i}");
            }
            Node::List(items) => {
                out.push('\n');
                for item in items {
                    let _ = writeln!(out, "{pad}  - {}", yaml_quote(item));
                }
            }
            Node::Map(m) if m.is_empty() => out.push_str(" {}\n"),
            Node::Map(m) => {
                out.push('\n');
                render(out, indent.saturating_add(2), m);
            }
        }
    }
}

/// Pushes `key: value` when the value is there.
fn opt(map: &mut Vec<(String, Node)>, key: &str, value: Option<&String>) {
    if let Some(v) = value {
        map.push((key.to_owned(), Node::Str(v.clone())));
    }
}

fn flag(map: &mut Vec<(String, Node)>, key: &str, value: Option<bool>) {
    if let Some(v) = value {
        map.push((key.to_owned(), Node::Bool(v)));
    }
}

/// `annotations:` in LinkML's expanded `tag`/`value` form, when there are any.
fn annotations(map: &mut Vec<(String, Node)>, tagged: &BTreeMap<String, Annotation>) {
    if tagged.is_empty() {
        return;
    }
    let body = tagged
        .iter()
        .map(|(k, a)| {
            let pair = vec![
                ("tag".to_owned(), Node::Str(a.tag.clone())),
                ("value".to_owned(), Node::Str(a.value.clone())),
            ];
            (k.clone(), Node::Map(pair))
        })
        .collect();
    map.push(("annotations".to_owned(), Node::Map(body)));
}

fn slot_node(slot: &Slot) -> Node {
    let mut m = Vec::new();
    opt(&mut m, "name", slot.name.as_ref());
    opt(&mut m, "slot_uri", slot.slot_uri.as_ref());
    opt(&mut m, "range", slot.range.as_ref());
    flag(&mut m, "required", slot.required);
    flag(&mut m, "multivalued", slot.multivalued);
    flag(&mut m, "identifier", slot.identifier);
    flag(&mut m, "inlined", slot.inlined);
    opt(&mut m, "inverse", slot.inverse.as_ref());
    if let Some(u) = &slot.unit {
        let mut unit = Vec::new();
        opt(&mut unit, "ucum_code", u.ucum_code.as_ref());
        opt(&mut unit, "symbol", u.symbol.as_ref());
        if !u.exact_mappings.is_empty() {
            unit.push((
                "exact_mappings".to_owned(),
                Node::List(u.exact_mappings.clone()),
            ));
        }
        m.push(("unit".to_owned(), Node::Map(unit)));
    }
    annotations(&mut m, &slot.annotations);
    opt(&mut m, "description", slot.description.as_ref());
    if let Some(rank) = slot.rank {
        m.push(("rank".to_owned(), Node::Int(rank)));
    }
    opt(&mut m, "title", slot.title.as_ref());
    Node::Map(m)
}

/// The LinkML YAML of the model (row L2), in LinkML's expanded forms
/// (`prefix_prefix`/`prefix_reference`, `tag`/`value`), every key and every
/// string double-quoted.
///
/// ```
/// # let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "a: b\n- c"}"#,
/// #     &linkml_model::Limits { max_bytes: 999, max_depth: 4, max_elements: 9 });
/// # assert!(m.is_err(), "a newline in a name is refused at load");
/// let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "a: b #c"}"#,
///     &linkml_model::Limits { max_bytes: 999, max_depth: 4, max_elements: 9 }).unwrap();
/// assert_eq!(linkml_model::yaml(&m), "\"id\": \"https://e.org/s\"\n\"name\": \"a: b #c\"\n\
///     \"imports\":\n  - \"linkml:types\"\n\"default_range\": \"string\"\n");
/// ```
#[must_use]
pub fn yaml(model: &Model) -> String {
    let s = model.schema();
    let mut top = vec![
        ("id".to_owned(), Node::Str(s.id.clone())),
        ("name".to_owned(), Node::Str(s.name.clone())),
    ];
    opt(&mut top, "version", s.version.as_ref());
    // what the subset implies, declared for every LinkML reader
    top.push(("imports".to_owned(), Node::List(vec![IMPORTS.to_owned()])));
    top.push((
        "default_range".to_owned(),
        Node::Str(DEFAULT_RANGE.to_owned()),
    ));
    if !s.prefixes.is_empty() {
        let prefixes = s
            .prefixes
            .iter()
            .map(|(k, p)| {
                let body = vec![
                    (
                        "prefix_prefix".to_owned(),
                        Node::Str(p.prefix_prefix.clone()),
                    ),
                    (
                        "prefix_reference".to_owned(),
                        Node::Str(p.prefix_reference.clone()),
                    ),
                ];
                (k.clone(), Node::Map(body))
            })
            .collect();
        top.push(("prefixes".to_owned(), Node::Map(prefixes)));
    }
    opt(&mut top, "default_prefix", s.default_prefix.as_ref());
    if !s.classes.is_empty() {
        let classes = s
            .classes
            .iter()
            .map(|(k, c)| {
                let mut m = Vec::new();
                opt(&mut m, "name", c.name.as_ref());
                opt(&mut m, "class_uri", c.class_uri.as_ref());
                opt(&mut m, "is_a", c.is_a.as_ref());
                if !c.slots.is_empty() {
                    m.push(("slots".to_owned(), Node::List(c.slots.clone())));
                }
                if !c.slot_usage.is_empty() {
                    let usage = c
                        .slot_usage
                        .iter()
                        .map(|(n, u)| (n.clone(), slot_node(u)))
                        .collect();
                    m.push(("slot_usage".to_owned(), Node::Map(usage)));
                }
                annotations(&mut m, &c.annotations);
                opt(&mut m, "description", c.description.as_ref());
                (k.clone(), Node::Map(m))
            })
            .collect();
        top.push(("classes".to_owned(), Node::Map(classes)));
    }
    if !s.slots.is_empty() {
        let slots = s
            .slots
            .iter()
            .map(|(k, v)| (k.clone(), slot_node(v)))
            .collect();
        top.push(("slots".to_owned(), Node::Map(slots)));
    }
    if !s.enums.is_empty() {
        let enums = s
            .enums
            .iter()
            .map(|(k, e)| {
                let mut m = Vec::new();
                opt(&mut m, "name", e.name.as_ref());
                opt(&mut m, "description", e.description.as_ref());
                if !e.permissible_values.is_empty() {
                    let values = e
                        .permissible_values
                        .iter()
                        .map(|(t, v)| {
                            let mut pv = Vec::new();
                            opt(&mut pv, "text", v.text.as_ref());
                            opt(&mut pv, "description", v.description.as_ref());
                            opt(&mut pv, "meaning", v.meaning.as_ref());
                            annotations(&mut pv, &v.annotations);
                            (t.clone(), Node::Map(pv))
                        })
                        .collect();
                    m.push(("permissible_values".to_owned(), Node::Map(values)));
                }
                (k.clone(), Node::Map(m))
            })
            .collect();
        top.push(("enums".to_owned(), Node::Map(enums)));
    }
    let mut out = String::new();
    render(&mut out, 0, &top);
    out
}

// --------------------------------------------------------- JSON Schema (L3)

fn typed(kind: &str, format: Option<&str>) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("type".to_owned(), Value::String(kind.to_owned()));
    if let Some(f) = format {
        m.insert("format".to_owned(), Value::String(f.to_owned()));
    }
    m
}

fn builtin_schema(b: Builtin) -> Map<String, Value> {
    match b {
        Builtin::String | Builtin::Uriorcurie => typed("string", None),
        Builtin::Integer => typed("integer", None),
        Builtin::Float | Builtin::Double | Builtin::Decimal => typed("number", None),
        Builtin::Boolean => typed("boolean", None),
        Builtin::Date => typed("string", Some("date")),
        Builtin::Datetime => typed("string", Some("date-time")),
        Builtin::Time => typed("string", Some("time")),
        Builtin::Uri => typed("string", Some("uri")),
    }
}

/// Whether the slot holds a `GeoJSON` geometry (NGSI-LD 4.7), whatever its
/// range says.
fn geo(model: &Model, slot: &Slot) -> bool {
    model.kind(slot) == "GeoProperty"
}

/// Whether the slot holds an object inlined, not a link: an inlined class, or
/// a class range under a kind other than a Relationship's (T-5206 finding 3).
fn inlined(model: &Model, slot: &Slot) -> bool {
    matches!(model.range(slot), Range::Class(_))
        && (slot.inlined == Some(true)
            || !matches!(model.kind(slot), "Relationship" | "ListRelationship"))
}

fn slot_schema(model: &Model, slot: &Slot) -> Value {
    let kind = model.kind(slot);
    let base = match model.range(slot) {
        // A kind no surface maps yet: any value (T-5207, mapping-contract rule 4).
        _ if !crate::known_kind(model.kind(slot)) => Map::new(),
        _ if geo(model, slot) => typed("object", None),
        // keyValues: a LanguageProperty is its map of language to text (NGSI-LD 4.5.18).
        _ if kind == "LanguageProperty" => {
            let mut m = typed("object", None);
            m.insert("additionalProperties".to_owned(), json!({"type": "string"}));
            m
        }
        // Any JSON value.
        _ if kind == "JsonProperty" => Map::new(),
        _ if inlined(model, slot) => typed("object", None),
        Range::Type(b) => builtin_schema(b),
        // A relationship holds the target entity's id.
        Range::Class(_) => typed("string", Some("uri")),
        Range::Enum(e) => {
            let values = model
                .schema()
                .enums
                .get(e)
                .map(|e| {
                    e.permissible_values
                        .keys()
                        .cloned()
                        .map(Value::String)
                        .collect()
                })
                .unwrap_or_default();
            let mut m = typed("string", None);
            m.insert("enum".to_owned(), Value::Array(values));
            m
        }
    };
    let listed = matches!(kind, "ListProperty" | "ListRelationship");
    let mut out = if slot.multivalued == Some(true) || listed {
        let mut m = typed("array", None);
        m.insert("items".to_owned(), Value::Object(base));
        m
    } else {
        base
    };
    out.insert("x-ngsi-ld-kind".to_owned(), Value::String(kind.to_owned()));
    if let Some(d) = &slot.description {
        out.insert("description".to_owned(), Value::String(d.clone()));
    }
    Value::Object(out)
}

/// JSON Schema draft-07 (row L3): one definition per class with its
/// inherited slots folded in, each property carrying `x-ngsi-ld-kind`
/// (DM-05).
///
/// ```
/// let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "s",
///     "classes": {"Room": {"slots": ["size"]}}, "slots": {"size": {"range": "integer"}}}"#,
///     &linkml_model::Limits { max_bytes: 999, max_depth: 8, max_elements: 9 }).unwrap();
/// let v: serde_json::Value = serde_json::from_str(&linkml_model::json_schema(&m)).unwrap();
/// assert_eq!(v["definitions"]["Room"]["properties"]["size"]["type"], "integer");
/// ```
#[must_use]
pub fn json_schema(model: &Model) -> String {
    let s = model.schema();
    let mut definitions = Map::new();
    for (name, class) in &s.classes {
        let slots = model.class_slots(name);
        let mut properties = Map::new();
        let mut required = Vec::new();
        for (slot_name, slot) in &slots {
            properties.insert((*slot_name).to_owned(), slot_schema(model, slot));
            if slot.required == Some(true) {
                required.push(Value::String((*slot_name).to_owned()));
            }
        }
        let mut def = Map::new();
        def.insert("type".to_owned(), json!("object"));
        def.insert("properties".to_owned(), Value::Object(properties));
        if !required.is_empty() {
            def.insert("required".to_owned(), Value::Array(required));
        }
        if let Some(d) = &class.description {
            def.insert("description".to_owned(), Value::String(d.clone()));
        }
        definitions.insert(name.clone(), Value::Object(def));
    }
    let doc = json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": s.id,
        "title": s.name,
        "definitions": definitions,
    });
    format!("{doc:#}\n")
}

// ------------------------------------------------------- JSON-LD context (L4)

/// The namespace the `xsd:` of [`Builtin::xsd`] stands for.
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// The JSON-LD `@context` (row L4): every class and slot name mapped to its
/// full IRI, and nothing else: no prefixes, so no term outside the model is
/// named (threat LK6). A Relationship's value is the target's `@id`, a typed
/// value carries its XSD datatype, so an Entity in keyValues reads as the
/// SHACL shapes (L5) describe it; a string, an enum and a `GeoJSON` geometry
/// stay as they are.
///
/// ```
/// let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "s",
///     "classes": {"Room": {}}, "slots": {"name": {}, "size": {"range": "float"}, "in": {"range": "Room"}}}"#,
///     &linkml_model::Limits { max_bytes: 999, max_depth: 8, max_elements: 9 }).unwrap();
/// let v: serde_json::Value = serde_json::from_str(&linkml_model::context(&m)).unwrap();
/// assert_eq!(v["@context"]["name"], "https://e.org/s/name");
/// assert_eq!(v["@context"]["size"]["@type"], "http://www.w3.org/2001/XMLSchema#float");
/// assert_eq!(v["@context"]["in"]["@type"], "@id");
/// ```
#[must_use]
pub fn context(model: &Model) -> String {
    let s = model.schema();
    let mut terms = Map::new();
    for (name, slot) in &s.slots {
        let iri = model.slot_iri(name);
        // The kind first, then the range (TS 104 175 5.2.6.4.6 to 5.2.6.4.10, T-5206 finding 3).
        let kind = model.kind(slot);
        let coerced = match (kind, model.range(slot)) {
            ("VocabProperty", _) => Some("@vocab".to_owned()),
            ("JsonProperty", _) => Some("@json".to_owned()),
            ("Relationship" | "ListRelationship", _) => Some("@id".to_owned()),
            // A kind no surface maps yet stays untyped (T-5207, mapping-contract rule 4).
            (k, Range::Type(b))
                if crate::known_kind(k)
                    && k != "LanguageProperty"
                    && b != Builtin::String
                    && !geo(model, slot) =>
            {
                Some(b.xsd().replacen("xsd:", XSD, 1))
            }
            // a GeoProperty, a LanguageProperty, any other kind over a class (it holds the
            // object, `inlined`), a string and an enum stay untyped
            _ => None,
        };
        let container = match kind {
            "LanguageProperty" => Some("@language"),
            "ListProperty" | "ListRelationship" => Some("@list"),
            _ => None,
        };
        let term = match (coerced, container) {
            (None, None) => Value::String(iri),
            (ty, container) => {
                let mut t = Map::new();
                t.insert("@id".to_owned(), Value::String(iri));
                t.extend(ty.map(|ty| ("@type".to_owned(), Value::String(ty))));
                t.extend(container.map(|c| ("@container".to_owned(), json!(c))));
                Value::Object(t)
            }
        };
        terms.insert(name.clone(), term);
    }
    for name in s.classes.keys() {
        terms.insert(name.clone(), Value::String(model.class_iri(name)));
    }
    let doc = json!({ "@context": terms });
    format!("{doc:#}\n")
}

// --------------------------------------------------------- Turtle (L5, L6)

/// A Turtle string literal.
fn lit(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn turtle_header(what: &str, extra: &[(&str, &str)]) -> String {
    let mut out = format!(
        "# {what}, rendered from the LinkML model.\n\
         @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
         @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n"
    );
    for (p, iri) in extra {
        let _ = writeln!(out, "@prefix {p}: <{iri}> .");
    }
    out
}

/// The IRI of a slot as refined in a class (`slot_usage` may set `slot_uri`).
fn refined_iri(model: &Model, name: &str, slot: &Slot) -> String {
    slot.slot_uri
        .as_deref()
        .map_or_else(|| model.slot_iri(name), |u| model.expand(u))
}

/// One level of Turtle indentation.
const IN: &str = "  ";

/// A name as an IRI segment: every byte outside RFC 3986's unreserved set
/// percent-encoded.
fn percent(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// SHACL shapes (row L5): one open node shape per class, named in the
/// schema's namespace (never minted under a foreign class IRI), one property shape
/// per slot with its inherited ones and `slot_usage` applied. Never
/// `sh:closed`: a view narrows what is described, not what exists.
///
/// ```
/// let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "s",
///     "classes": {"Room": {"slots": ["size"]}}, "slots": {"size": {"range": "integer", "required": true}}}"#,
///     &linkml_model::Limits { max_bytes: 999, max_depth: 8, max_elements: 9 }).unwrap();
/// let ttl = linkml_model::shacl(&m);
/// assert!(ttl.contains("sh:datatype xsd:integer"));
/// assert!(ttl.contains("sh:minCount 1"));
/// ```
#[must_use]
pub fn shacl(model: &Model) -> String {
    let mut out = turtle_header("SHACL shapes", &[("sh", "http://www.w3.org/ns/shacl#")]);
    let ns = model.namespace();
    // The two display groups (DM-103): a class's key attributes, and the rest.
    let _ = write!(
        out,
        "\n<{ns}SummaryGroup>\n  a sh:PropertyGroup ;\n  sh:order 0 ;\n  rdfs:label \"Základné\"@sk, \"Summary\"@en .\n\
         \n<{ns}DetailGroup>\n  a sh:PropertyGroup ;\n  sh:order 1 ;\n  rdfs:label \"Detail\"@sk, \"Detail\"@en .\n"
    );
    for name in model.schema().classes.keys() {
        let _ = write!(
            out,
            "\n<{}{}Shape>\n  a sh:NodeShape ;\n  sh:targetClass <{}> ;\n  sh:closed false",
            model.namespace(),
            percent(name),
            model.class_iri(name)
        );
        for (order, summary, slot_name, slot) in display_order(model, name) {
            let _ = write!(
                out,
                " ;\n  sh:property [\n{IN}{IN}sh:path <{}> ;\n{IN}{IN}sh:name {}",
                refined_iri(model, slot_name, &slot),
                lit(slot_name)
            );
            for (lang, text) in labels(&slot) {
                let _ = write!(out, ", {}@{lang}", lit(&text));
            }
            let group = if summary { "Summary" } else { "Detail" };
            let _ = write!(
                out,
                " ;\n{IN}{IN}sh:order {order} ;\n{IN}{IN}sh:group <{ns}{group}Group>"
            );
            match model.range(&slot) {
                // a GeoJSON geometry: a node, whatever the range says
                _ if geo(model, &slot) => {}
                Range::Type(b) => {
                    let _ = write!(out, " ;\n{IN}{IN}sh:datatype {}", b.xsd());
                }
                Range::Class(c) => {
                    let _ = write!(
                        out,
                        " ;\n{IN}{IN}sh:nodeKind sh:IRI ;\n{IN}{IN}sh:class <{}>",
                        model.class_iri(c)
                    );
                }
                Range::Enum(e) => {
                    let values: Vec<String> = model
                        .schema()
                        .enums
                        .get(e)
                        .map(|e| e.permissible_values.keys().map(|v| lit(v)).collect())
                        .unwrap_or_default();
                    let _ = write!(out, " ;\n{IN}{IN}sh:in ( {} )", values.join(" "));
                }
            }
            if slot.required == Some(true) {
                let _ = write!(out, " ;\n{IN}{IN}sh:minCount 1");
            }
            if slot.multivalued != Some(true) {
                let _ = write!(out, " ;\n{IN}{IN}sh:maxCount 1");
            }
            if let Some(d) = &slot.description {
                let _ = write!(out, " ;\n{IN}{IN}rdfs:comment {}", lit(d));
            }
            out.push_str("\n  ]");
        }
        out.push_str(" .\n");
    }
    out
}

/// The slots shown last unless ranked, in this order (DM-98).
const SYSTEM: &[&str] = &["location", "dateCreated", "dateModified", "type", "id"];

/// How many slots are a class's key attributes when it marks none (DM-99).
const FIRST: usize = 4;

/// A tagged value as a flag: `true` as the model tools write it.
fn marked(slot: &Slot, tag: &str) -> bool {
    slot.annotations.get(tag).is_some_and(|a| a.value == "true")
}

/// The slot's `label` annotation, JSON text of a language map, as
/// (language, text) pairs; a tag that is no language tag is skipped so it
/// cannot end the literal (DM-100).
fn labels(slot: &Slot) -> Vec<(String, String)> {
    let Some(label) = slot.annotations.get("label") else {
        return Vec::new();
    };
    let map: BTreeMap<String, String> = serde_json::from_str(&label.value).unwrap_or_default();
    map.into_iter()
        .filter(|(lang, _)| {
            !lang.is_empty()
                && lang.len() <= 35
                && lang.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .collect()
}

/// The display order of a class's slots (DM-98, DM-99, the order
/// `jc_core::display_order` computes): ranked ones first, lower first; then
/// its own slots as written and the inherited ones, nearest ancestor first;
/// `location`, `dateCreated`, `dateModified`, `type`, `id` last unless
/// ranked. Each comes with its place and whether it is a key attribute: the
/// slots annotated `summary`, else the first four that are not `hidden`.
fn display_order<'a>(model: &'a Model, class: &str) -> Vec<(usize, bool, &'a str, Slot)> {
    let schema = model.schema();
    let mut written: Vec<&str> = Vec::new();
    let mut next = Some(class);
    // At most one step per class, so an `is_a` cycle cannot loop.
    for _ in 0..=schema.classes.len() {
        let Some(declared) = next.and_then(|n| schema.classes.get(n)) else {
            break;
        };
        for slot in &declared.slots {
            if !written.contains(&slot.as_str()) {
                written.push(slot);
            }
        }
        next = declared.is_a.as_deref();
    }
    let mut slots = model.class_slots(class);
    slots.sort_by_key(|(name, slot)| {
        let place = written.iter().position(|w| w == name).unwrap_or(usize::MAX);
        match (slot.rank, SYSTEM.iter().position(|s| s == name)) {
            (Some(rank), _) => (0, rank, place),
            (None, Some(at)) => (2, 0, at),
            (None, None) => (1, 0, place),
        }
    });
    let any = slots.iter().any(|(_, slot)| marked(slot, "summary"));
    let mut first = 0;
    slots
        .into_iter()
        .enumerate()
        .map(|(order, (name, slot))| {
            let summary = if any {
                marked(&slot, "summary")
            } else if !marked(&slot, "hidden") && first < FIRST {
                first = first.saturating_add(1);
                true
            } else {
                false
            };
            (order, summary, name, slot)
        })
        .collect()
}

/// The `rdfs:range` of a slot: an `xsd:` datatype, a class IRI, or
/// `xsd:string` for an enum.
fn rdf_range(model: &Model, slot: &Slot) -> String {
    match model.range(slot) {
        Range::Type(b) => b.xsd().to_owned(),
        Range::Class(c) => format!("<{}>", model.class_iri(c)),
        Range::Enum(_) => "xsd:string".to_owned(),
    }
}

/// The OWL ontology (row L6, `model.owl.ttl`): the schema as an
/// `owl:Ontology`, classes with `rdfs:subClassOf`, slots as object or
/// datatype properties with `owl:inverseOf`.
///
/// ```
/// let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "s",
///     "classes": {"Room": {}}, "slots": {"in": {"range": "Room"}}}"#,
///     &linkml_model::Limits { max_bytes: 999, max_depth: 8, max_elements: 9 }).unwrap();
/// assert!(linkml_model::owl(&m).contains("<https://e.org/s/in> a owl:ObjectProperty"));
/// ```
#[must_use]
pub fn owl(model: &Model) -> String {
    let s = model.schema();
    let roles = cube_roles(model);
    let mut out = turtle_header("OWL ontology", &[("owl", "http://www.w3.org/2002/07/owl#")]);
    if s.classes.keys().any(|c| is_dsd(model, c)) {
        let _ = writeln!(out, "@prefix qb: <{QB}> .");
    }
    let _ = write!(
        out,
        "\n<{}> a owl:Ontology ;\n  rdfs:label {}",
        s.id,
        lit(&s.name)
    );
    if let Some(v) = &s.version {
        let _ = write!(out, " ;\n  owl:versionInfo {}", lit(v));
    }
    out.push_str(" .\n");
    for (name, class) in &s.classes {
        let _ = write!(
            out,
            "\n<{}> a owl:Class ;\n  rdfs:label {}",
            model.class_iri(name),
            lit(name)
        );
        if let Some(p) = &class.is_a {
            let _ = write!(out, " ;\n  rdfs:subClassOf <{}>", model.class_iri(p));
        }
        if is_dsd(model, name) {
            out.push_str(" ;\n  rdfs:subClassOf qb:Observation");
        }
        if let Some(d) = &class.description {
            let _ = write!(out, " ;\n  rdfs:comment {}", lit(d));
        }
        out.push_str(" .\n");
    }
    for (name, slot) in &s.slots {
        let kind = if matches!(model.range(slot), Range::Class(_)) {
            "owl:ObjectProperty"
        } else {
            "owl:DatatypeProperty"
        };
        let kind = match roles.get(name.as_str()) {
            Some(role) => format!("{kind}, {role}"),
            None => kind.to_owned(),
        };
        let _ = write!(
            out,
            "\n<{}> a {kind} ;\n  rdfs:label {} ;\n  rdfs:range {}",
            model.slot_iri(name),
            lit(name),
            rdf_range(model, slot)
        );
        if let Some(inv) = &slot.inverse {
            let _ = write!(out, " ;\n  owl:inverseOf <{}>", model.slot_iri(inv));
        }
        if let Some(d) = &slot.description {
            let _ = write!(out, " ;\n  rdfs:comment {}", lit(d));
        }
        out.push_str(" .\n");
    }
    out
}

/// The W3C RDF Data Cube vocabulary (DM-60).
const QB: &str = "http://purl.org/linked-data/cube#";

/// Whether class `name` is a Data Structure Definition: annotated
/// `qb_dsd: "true"` (DM-60).
fn is_dsd(model: &Model, name: &str) -> bool {
    model
        .schema()
        .classes
        .get(name)
        .and_then(|c| c.annotations.get(QB_DSD_TAG))
        .is_some_and(|a| a.value == "true")
}

/// A slot's role in a cube, as the key a structure names it by and its
/// property type; `None` for a slot without `qb_component`.
fn component(slot: &Slot) -> Option<(&'static str, &'static str)> {
    // the load refuses any value but these two
    if slot.annotations.get(QB_COMPONENT_TAG)?.value == "dimension" {
        Some(("qb:dimension", "qb:DimensionProperty"))
    } else {
        Some(("qb:measure", "qb:MeasureProperty"))
    }
}

/// The cube property type of each slot that is a component of some Data
/// Structure Definition of the model, by slot name.
fn cube_roles(model: &Model) -> BTreeMap<&str, &'static str> {
    model
        .schema()
        .classes
        .keys()
        .filter(|c| is_dsd(model, c))
        .flat_map(|c| model.class_slots(c))
        .filter_map(|(name, slot)| Some((name, component(&slot)?.1)))
        .collect()
}

/// The RDF Data Cube structure (row L6, `model.qb.ttl`, DM-60): one
/// `qb:DataStructureDefinition` per class that is one, its components the
/// slots annotated `qb_component`, the class a subclass of
/// `qb:Observation`. No `qb:order`: the vocabulary makes it optional, and a
/// narrowed model may have dropped components. `None` when no class is a
/// Data Structure Definition: the document does not exist then, it is not
/// an empty cube.
///
/// ```
/// let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "s",
///     "classes": {"Count": {"slots": ["year", "n"],
///                           "annotations": {"qb_dsd": {"tag": "qb_dsd", "value": "true"}}}},
///     "slots": {"year": {"range": "integer",
///                        "annotations": {"qb_component": {"tag": "qb_component", "value": "dimension"}}},
///               "n": {"range": "integer",
///                     "annotations": {"qb_component": {"tag": "qb_component", "value": "measure"}}}}}"#,
///     &linkml_model::Limits { max_bytes: 999, max_depth: 8, max_elements: 99 }).unwrap();
/// let ttl = linkml_model::qb(&m).unwrap();
/// assert!(ttl.contains("<https://e.org/s/CountStructure> a qb:DataStructureDefinition"));
/// assert!(ttl.contains("[ qb:measure <https://e.org/s/n> ]"));
/// ```
#[must_use]
pub fn qb(model: &Model) -> Option<String> {
    let s = model.schema();
    let cubes: Vec<&String> = s.classes.keys().filter(|c| is_dsd(model, c)).collect();
    if cubes.is_empty() {
        return None;
    }
    let mut out = turtle_header("RDF Data Cube structure", &[("qb", QB)]);
    for name in cubes {
        let iri = model.class_iri(name);
        let components: Vec<(&str, Slot, (&str, &str))> = model
            .class_slots(name)
            .into_iter()
            .filter_map(|(n, slot)| {
                let role = component(&slot)?;
                Some((n, slot, role))
            })
            .collect();
        let _ = write!(
            out,
            "\n<{iri}Structure> a qb:DataStructureDefinition ;\n  rdfs:label {}",
            lit(name)
        );
        if let Some(d) = s.classes.get(name).and_then(|c| c.description.as_ref()) {
            let _ = write!(out, " ;\n  rdfs:comment {}", lit(d));
        }
        let entries: Vec<String> = components
            .iter()
            .map(|(n, slot, (key, _))| {
                format!("{IN}{IN}[ {key} <{}> ]", refined_iri(model, n, slot))
            })
            .collect();
        if entries.is_empty() {
            out.push_str(" .\n");
        } else {
            let _ = write!(out, " ;\n  qb:component\n{} .\n", entries.join(" ,\n"));
        }
        let _ = writeln!(out, "\n<{iri}> rdfs:subClassOf qb:Observation .");
        for (n, slot, (_, kind)) in &components {
            let _ = write!(
                out,
                "\n<{}> a rdf:Property, {kind} ;\n  rdfs:label {} ;\n  rdfs:range {}",
                refined_iri(model, n, slot),
                lit(n),
                rdf_range(model, slot)
            );
            if let Some(d) = &slot.description {
                let _ = write!(out, " ;\n  rdfs:comment {}", lit(d));
            }
            out.push_str(" .\n");
        }
    }
    Some(out)
}

/// The RDFS rendering (row L6, `model.rdf.ttl`): classes, subclasses,
/// properties and their ranges.
///
/// ```
/// let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "s",
///     "classes": {"Room": {}}}"#,
///     &linkml_model::Limits { max_bytes: 999, max_depth: 8, max_elements: 9 }).unwrap();
/// assert!(linkml_model::rdfs(&m).contains("<https://e.org/s/Room> a rdfs:Class"));
/// ```
#[must_use]
pub fn rdfs(model: &Model) -> String {
    let s = model.schema();
    let mut out = turtle_header("RDF Schema", &[]);
    for (name, class) in &s.classes {
        let _ = write!(
            out,
            "\n<{}> a rdfs:Class ;\n  rdfs:label {}",
            model.class_iri(name),
            lit(name)
        );
        if let Some(p) = &class.is_a {
            let _ = write!(out, " ;\n  rdfs:subClassOf <{}>", model.class_iri(p));
        }
        out.push_str(" .\n");
    }
    for (name, slot) in &s.slots {
        let _ = write!(
            out,
            "\n<{}> a rdf:Property ;\n  rdfs:label {} ;\n  rdfs:range {} .\n",
            model.slot_iri(name),
            lit(name),
            rdf_range(model, slot)
        );
    }
    out
}

// ------------------------------------------------------------ Markdown (L6)

/// Text as inert Markdown: punctuation that opens a link, an emphasis, a
/// code span, an HTML tag or a table cell is backslash-escaped, a line
/// break becomes a space.
fn md(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.push(' ');
        } else {
            if "\\`*_[]()<>|#!&".contains(c) {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out
}

fn yes(v: Option<bool>) -> &'static str {
    if v == Some(true) {
        "yes"
    } else {
        "no"
    }
}

/// The Markdown description (row L6, `model.md`): one section per class
/// with a table of its slots, then the enums.
///
/// ```
/// let m = linkml_model::Model::read(br#"{"id": "https://e.org/s", "name": "s",
///     "classes": {"Room": {"description": "see [x](javascript:y)"}}}"#,
///     &linkml_model::Limits { max_bytes: 999, max_depth: 8, max_elements: 9 }).unwrap();
/// assert!(linkml_model::markdown(&m).contains("see \\[x\\]\\(javascript:y\\)"));
/// ```
#[must_use]
pub fn markdown(model: &Model) -> String {
    let s = model.schema();
    let mut out = format!("# {}\n\n`{}`", md(&s.name), s.id);
    if let Some(v) = &s.version {
        let _ = write!(out, ", version {}", md(v));
    }
    out.push('\n');
    for (name, class) in &s.classes {
        let _ = write!(out, "\n## {}\n\n", md(name));
        if let Some(d) = &class.description {
            let _ = write!(out, "{}\n\n", md(d));
        }
        if let Some(p) = &class.is_a {
            let _ = write!(out, "Is a: {}\n\n", md(p));
        }
        let slots = model.class_slots(name);
        if slots.is_empty() {
            out.push_str("No slots.\n");
            continue;
        }
        out.push_str("| Slot | Range | Kind | Required | Multivalued | Description |\n|---|---|---|---|---|---|\n");
        for (slot_name, slot) in &slots {
            let range = match model.range(slot) {
                Range::Type(b) => b.name(),
                Range::Class(c) | Range::Enum(c) => c,
            };
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} |",
                md(slot_name),
                md(range),
                md(model.kind(slot)),
                yes(slot.required),
                yes(slot.multivalued),
                md(slot.description.as_deref().unwrap_or(""))
            );
        }
    }
    for (name, e) in &s.enums {
        let _ = write!(out, "\n## {} (enum)\n\n", md(name));
        if let Some(d) = &e.description {
            let _ = write!(out, "{}\n\n", md(d));
        }
        for (text, v) in &e.permissible_values {
            let _ = write!(out, "- {}", md(text));
            if let Some(d) = &v.description {
                let _ = write!(out, ": {}", md(d));
            }
            out.push('\n');
        }
    }
    out
}
