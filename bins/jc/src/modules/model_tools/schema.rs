//! A LinkML document read the way Model Tools' Python loader reads it: YAML parsed under the
//! module's limits, every element checked against the metaslots linkml-runtime 1.11.1 accepts,
//! the import closure resolved from the request alone, and the closure merged into the main
//! schema as `SchemaView.merge_imports` does. [`View`] answers what the generators ask of
//! `SchemaView`: the classes and slots, a class's slots, a slot induced in a class, an element's
//! IRI.

use super::fields;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

/// One LinkML element (class, slot, enum, ...) with its metaslots, normalised.
pub(super) type Obj = Map<String, Value>;

/// The shared core every NGSI-LD model imports (DM-09), as the Python image ships it.
const NGSI_LD_CORE: &str = include_str!("ngsi-ld-core.linkml.yaml");

/// `linkml:types`: its prefixes and the types with their URIs (linkml-runtime 1.11.1
/// `linkml_model/model/schema/types.yaml`; none of them declares a `typeof`).
const LINKML_TYPES_PREFIXES: [(&str, &str); 4] = [
    ("linkml", "https://w3id.org/linkml/"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ("shex", "http://www.w3.org/ns/shex#"),
    ("schema", "http://schema.org/"),
];
const LINKML_TYPES: [(&str, &str); 19] = [
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

/// A platform model an import names at a pinned major (DM-76, common.py `MODEL_IMPORT`).
#[must_use]
pub(super) fn is_model_import(name: &str) -> bool {
    let Some((kind, rest)) = name.split_once('.') else {
        return false;
    };
    if kind != "org" && kind != "project" {
        return false;
    }
    let Some((label, major)) = rest.rsplit_once(".v") else {
        return false;
    };
    let label_ok = !label.is_empty()
        && label.len() <= 63
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-');
    let major_ok = !major.is_empty()
        && major.bytes().all(|b| b.is_ascii_digit())
        && (major == "0" || !major.starts_with('0'));
    label_ok && major_ok
}

/// How much one request may parse: nesting depth and nodes, summed over the source and every
/// import. The body cap bounds the text; these bound what the text expands to (aliases).
#[derive(Debug, Clone, Copy)]
pub(super) struct Limits {
    pub max_depth: usize,
    pub max_elements: usize,
}

/// YAML text to JSON under `limits`, counting nodes into `used`. Keys become text as Python's
/// `str()` writes them.
///
/// The nodes are counted while the parser hands them over, aliases expanded: serde_norway's own
/// guard (jumps up to 100 times the events) lets a small text expand to millions of nodes, which
/// a count over a finished value would only see after they are in memory.
pub(super) fn parse_yaml(text: &str, limits: Limits, used: &mut usize) -> Result<Value, String> {
    use serde::de::DeserializeSeed;
    let mut problem = None;
    let seed = Node {
        depth: 0,
        limits,
        used,
        problem: &mut problem,
    };
    let parsed = seed.deserialize(serde_norway::Deserializer::from_str(text));
    match (parsed, problem) {
        (_, Some(problem)) => Err(problem),
        (Ok(v), None) => Ok(v),
        (Err(e), None) => Err(e.to_string()),
    }
}

/// One YAML node on its way to JSON. A refusal of ours is kept in `problem` and the parser is
/// stopped with a bare error, so the answer says ours and not the parser's wrapping of it.
struct Node<'a> {
    depth: usize,
    limits: Limits,
    used: &'a mut usize,
    problem: &'a mut Option<String>,
}

impl Node<'_> {
    fn refuse<E: serde::de::Error>(self, message: String) -> E {
        *self.problem = Some(message.clone());
        E::custom(message)
    }

    fn child(&mut self) -> Node<'_> {
        Node {
            depth: self.depth + 1,
            limits: self.limits,
            used: self.used,
            problem: self.problem,
        }
    }
}

impl<'de> serde::de::DeserializeSeed<'de> for Node<'_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for Node<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a LinkML document")
    }

    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Value, E> {
        self.leaf(Value::Bool(v))
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Value, E> {
        self.leaf(Value::from(v))
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Value, E> {
        self.leaf(Value::from(v))
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Value, E> {
        self.leaf(serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Value, E> {
        self.leaf(Value::String(v.to_owned()))
    }

    fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Value, E> {
        self.leaf(Value::String(v))
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        self.leaf(Value::Null)
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Value, E> {
        self.leaf(Value::Null)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(mut self, mut seq: A) -> Result<Value, A::Error> {
        self.enter()?;
        let mut out = Vec::new();
        while let Some(v) = seq.next_element_seed(self.child())? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(mut self, mut map: A) -> Result<Value, A::Error> {
        self.enter()?;
        let mut out = Map::new();
        while let Some(key) = map.next_key_seed(self.child())? {
            let key = match key {
                Value::String(s) => s,
                other => py_str(&other),
            };
            if out.contains_key(&key) {
                return Err(self.refuse(format!("Duplicate key: \"{key}\"")));
            }
            let value = map.next_value_seed(self.child())?;
            out.insert(key, value);
        }
        Ok(Value::Object(out))
    }

    /// A local tag (`!name`). ponytail: a `!!name` tag outside the core schema
    /// (`!!python/object`) reaches the visitor as its plain text, which Python refuses; serde_norway
    /// does not hand that tag over.
    fn visit_enum<A: serde::de::EnumAccess<'de>>(self, _: A) -> Result<Value, A::Error> {
        Err(self.refuse("a YAML tag is not part of a LinkML document".to_owned()))
    }
}

impl Node<'_> {
    /// Counts this node and checks its depth.
    fn count<E: serde::de::Error>(&mut self) -> Result<(), E> {
        *self.used += 1;
        if *self.used > self.limits.max_elements {
            let message = format!(
                "the model has more than {} YAML nodes, more than Model Tools reads",
                self.limits.max_elements
            );
            *self.problem = Some(message.clone());
            return Err(E::custom(message));
        }
        if self.depth > self.limits.max_depth {
            let message = format!(
                "the model nests deeper than {} levels, deeper than Model Tools reads",
                self.limits.max_depth
            );
            *self.problem = Some(message.clone());
            return Err(E::custom(message));
        }
        Ok(())
    }

    fn leaf<E: serde::de::Error>(mut self, v: Value) -> Result<Value, E> {
        self.count()?;
        Ok(v)
    }

    fn enter<E: serde::de::Error>(&mut self) -> Result<(), E> {
        self.count()
    }
}

// ------------------------------------------------------------------ Python's renderings

/// Python's `str()` of a loaded YAML value.
#[must_use]
pub(super) fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Null => "None".to_owned(),
        Value::Number(n) => py_number(n),
        other => py_json(other),
    }
}

fn py_number(n: &serde_json::Number) -> String {
    if n.is_f64() {
        n.as_f64().map_or_else(|| n.to_string(), py_float)
    } else {
        n.to_string()
    }
}

/// Python's `repr()` of a float: the shortest round-trip digits, fixed notation from 1e-4 up to
/// 1e16, scientific with a signed two-digit exponent beyond.
fn py_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_owned();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    let sci = format!("{f:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (sign, mantissa) = mantissa
        .strip_prefix('-')
        .map_or(("", mantissa), |m| ("-", m));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    if (-4..16).contains(&exp) {
        let point = exp + 1;
        let body = if point <= 0 {
            format!(
                "0.{}{digits}",
                "0".repeat(usize::try_from(-point).unwrap_or(0))
            )
        } else {
            let point = usize::try_from(point).unwrap_or(0);
            if digits.len() <= point {
                format!("{digits}{}.0", "0".repeat(point - digits.len()))
            } else {
                format!("{}.{}", &digits[..point], &digits[point..])
            }
        };
        format!("{sign}{body}")
    } else {
        let (first, rest) = digits.split_at(1);
        let mantissa = if rest.is_empty() {
            first.to_owned()
        } else {
            format!("{first}.{rest}")
        };
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{sign}{mantissa}e{esign}{:02}", exp.abs())
    }
}

/// Python's `json.dumps(value)`: `, ` and `: ` separators, every non-ASCII character escaped.
#[must_use]
pub(super) fn py_json(v: &Value) -> String {
    let mut out = String::new();
    py_json_into(v, &mut out);
    out
}

fn py_json_into(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&py_number(n)),
        Value::String(s) => py_json_str(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                py_json_into(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                py_json_str(k, out);
                out.push_str(": ");
                py_json_into(item, out);
            }
            out.push('}');
        }
    }
}

fn py_json_str(s: &str, out: &mut String) {
    use std::fmt::Write as _;
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Python's truthiness of a loaded value.
#[must_use]
pub(super) fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// linkml-runtime's `is_empty`: None, or an empty list or map.
fn is_empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
        Some(_) => false,
    }
}

/// `formatutils.underscore`.
#[must_use]
pub(super) fn underscore(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.trim().chars() {
        if c.is_whitespace() {
            if !in_space {
                out.push('_');
            }
            in_space = true;
            continue;
        }
        in_space = false;
        match c {
            ',' => {}
            '-' => out.push('_'),
            c => out.push(c),
        }
    }
    out
}

/// `formatutils.camelcase`.
#[must_use]
pub(super) fn camelcase(text: &str) -> String {
    let cleaned = text.trim().replace(',', "");
    let mut spaced = String::with_capacity(cleaned.len());
    let mut in_us = false;
    for c in cleaned.chars() {
        if c == '_' {
            if !in_us {
                spaced.push(' ');
            }
            in_us = true;
        } else {
            in_us = false;
            spaced.push(c);
        }
    }
    spaced
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect::<String>()
            })
        })
        .collect()
}

/// `formatutils.lcamelcase`.
fn lcamelcase(text: &str) -> String {
    let s = camelcase(text);
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}

/// SchemaLoader's name of an attribute promoted to a slot (`mangled_attribute_name`).
#[must_use]
pub(super) fn mangled_attribute_name(class: &str, attribute: &str) -> String {
    format!("{}__{}", lcamelcase(class), underscore(attribute))
}

/// SchemaLoader's name of a class's slot_usage slot (`mergeutils.slot_usage_name`).
#[must_use]
pub(super) fn slot_usage_name(slot: &str, class: &str) -> String {
    format!("{}_{}", underscore(class), underscore(slot))
}

// ------------------------------------------------------------------ the elements

/// One schema of the closure, its elements normalised and checked.
#[derive(Debug, Clone, Default)]
pub(super) struct Schema {
    pub id: String,
    pub name: String,
    pub default_prefix: Option<String>,
    pub default_range: Option<String>,
    pub version: Option<String>,
    pub prefixes: Vec<(String, String)>,
    pub emit_prefixes: Vec<String>,
    pub imports: Vec<String>,
    pub classes: Map<String, Value>,
    pub slots: Map<String, Value>,
    pub enums: Map<String, Value>,
    pub types: Map<String, Value>,
    pub subsets: Map<String, Value>,
}

fn unexpected(class: &str, key: &str) -> String {
    format!("{class}.__init__() got an unexpected keyword argument '{key}'")
}

fn check_keys(obj: &Obj, (class, allowed): (&str, &[&str])) -> Result<(), String> {
    match obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(key) => Err(unexpected(class, key)),
        None => Ok(()),
    }
}

/// A list metaslot: a list of texts, or one text standing for a list of one.
fn text_list(v: &Value) -> Vec<Value> {
    match v {
        Value::Null => Vec::new(),
        Value::Array(items) => items
            .iter()
            .filter(|i| !i.is_null())
            .map(|i| Value::String(py_str(i)))
            .collect(),
        other => vec![Value::String(py_str(other))],
    }
}

const LIST_METASLOTS: [&str; 12] = [
    "slots",
    "mixins",
    "exact_mappings",
    "close_mappings",
    "related_mappings",
    "narrow_mappings",
    "broad_mappings",
    "mappings",
    "id_prefixes",
    "union_of",
    "aliases",
    "in_subset",
];

/// Annotations in their long form, `{tag: {tag, value[, annotations]}}`.
fn annotations(v: &Value) -> Result<Value, String> {
    let Value::Object(map) = v else {
        return match v {
            Value::Null => Ok(Value::Object(Map::new())),
            _ => Err("annotations are a map of tag to value".to_owned()),
        };
    };
    let mut out = Map::new();
    for (tag, entry) in map {
        let mut long = Map::new();
        match entry {
            Value::Object(fields) => {
                check_keys(fields, fields::ANNOTATION)?;
                long.insert("tag".to_owned(), Value::String(tag.clone()));
                long.insert(
                    "value".to_owned(),
                    fields.get("value").cloned().unwrap_or(Value::Null),
                );
                if let Some(nested) = fields.get("annotations") {
                    long.insert("annotations".to_owned(), annotations(nested)?);
                }
            }
            other => {
                long.insert("tag".to_owned(), Value::String(tag.clone()));
                long.insert("value".to_owned(), other.clone());
            }
        }
        out.insert(tag.clone(), Value::Object(long));
    }
    Ok(Value::Object(out))
}

/// A map of named elements, each normalised by `each`; a null entry is an empty element.
fn named(
    v: Option<&Value>,
    what: &str,
    each: &dyn Fn(&mut Obj) -> Result<(), String>,
) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    match v {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (name, element) in map {
                let mut obj = match element {
                    Value::Object(o) => o.clone(),
                    Value::Null => Map::new(),
                    _ => return Err(format!("{what} '{name}' is not a map of metaslots")),
                };
                each(&mut obj)?;
                out.insert(name.clone(), Value::Object(obj));
            }
        }
        // A list of elements, each naming itself, as LinkML also reads them.
        Some(Value::Array(items)) => {
            for element in items {
                let Value::Object(o) = element else {
                    return Err(format!("{what} entries are maps"));
                };
                let mut obj = o.clone();
                let Some(name) = obj.get("name").map(py_str) else {
                    return Err(format!("{what} entries name themselves"));
                };
                each(&mut obj)?;
                out.insert(name, Value::Object(obj));
            }
        }
        Some(_) => return Err(format!("{what} are a map of name to definition")),
    }
    Ok(out)
}

fn common(obj: &mut Obj) -> Result<(), String> {
    for key in LIST_METASLOTS {
        if let Some(v) = obj.get(key) {
            let list = text_list(v);
            obj.insert(key.to_owned(), Value::Array(list));
        }
    }
    if let Some(v) = obj.get("annotations") {
        let long = annotations(v)?;
        obj.insert("annotations".to_owned(), long);
    }
    Ok(())
}

fn slot_like(obj: &mut Obj) -> Result<(), String> {
    check_keys(obj, fields::SLOT)?;
    common(obj)?;
    if let Some(unit) = obj.get("unit") {
        match unit {
            Value::Object(u) => {
                check_keys(u, fields::UNIT)?;
                let mut u = u.clone();
                if let Some(m) = u.get("exact_mappings") {
                    let list = text_list(m);
                    u.insert("exact_mappings".to_owned(), Value::Array(list));
                }
                obj.insert("unit".to_owned(), Value::Object(u));
            }
            Value::Null => {
                obj.remove("unit");
            }
            _ => return Err("a unit is a map (ucum_code, symbol, exact_mappings, ...)".to_owned()),
        }
    }
    if let Some(any_of) = obj.get("any_of") {
        let Value::Array(items) = any_of else {
            return Err("any_of is a list of slot expressions".to_owned());
        };
        for item in items {
            if let Value::Object(o) = item {
                check_keys(o, fields::ANONYMOUS_SLOT_EXPRESSION)?;
            }
        }
    }
    for key in ["range", "slot_uri", "inverse", "is_a", "alias"] {
        if let Some(v) = obj.get(key).filter(|v| !v.is_null() && !v.is_string()) {
            let text = py_str(v);
            obj.insert(key.to_owned(), Value::String(text));
        }
    }
    Ok(())
}

fn class_like(obj: &mut Obj) -> Result<(), String> {
    check_keys(obj, fields::CLASS)?;
    common(obj)?;
    let usage = named(obj.get("slot_usage"), "slot_usage", &slot_like)?;
    obj.insert("slot_usage".to_owned(), Value::Object(usage));
    let attributes = named(obj.get("attributes"), "attribute", &slot_like)?;
    obj.insert("attributes".to_owned(), Value::Object(attributes));
    if !obj.contains_key("slots") {
        obj.insert("slots".to_owned(), Value::Array(Vec::new()));
    }
    Ok(())
}

fn enum_like(obj: &mut Obj) -> Result<(), String> {
    check_keys(obj, fields::ENUM)?;
    common(obj)?;
    let pv = named(obj.get("permissible_values"), "permissible value", &|pv| {
        check_keys(pv, fields::PERMISSIBLE_VALUE)?;
        common(pv)
    })?;
    obj.insert("permissible_values".to_owned(), Value::Object(pv));
    Ok(())
}

impl Schema {
    /// One document of the closure, checked as linkml-runtime's loader checks it.
    pub(super) fn from_document(doc: &Value) -> Result<Self, String> {
        let Value::Object(top) = doc else {
            return Err(format!(
                "linkml_runtime.linkml_model.meta.SchemaDefinition() argument after ** must be a mapping, not {}",
                match doc {
                    Value::Array(_) => "list",
                    Value::String(_) => "extended_str",
                    Value::Bool(_) => "bool",
                    Value::Number(_) => "number",
                    _ => "NoneType",
                }
            ));
        };
        check_keys(top, fields::SCHEMA)?;
        let text = |k: &str| top.get(k).filter(|v| !v.is_null()).map(py_str);
        let id = text("id").ok_or_else(|| "SchemaDefinition.id must be supplied".to_owned())?;
        let name = text("name").unwrap_or_else(|| {
            id.rsplit(['/', '#'])
                .find(|s| !s.is_empty())
                .unwrap_or(&id)
                .to_owned()
        });
        let mut prefixes = Vec::new();
        match top.get("prefixes") {
            None | Some(Value::Null) => {}
            Some(Value::Object(map)) => {
                for (prefix, reference) in map {
                    let reference = match reference {
                        Value::Object(p) => {
                            check_keys(p, fields::PREFIX)?;
                            p.get("prefix_reference").map(py_str).ok_or_else(|| {
                                "Prefix.prefix_reference must be supplied".to_owned()
                            })?
                        }
                        other => py_str(other),
                    };
                    prefixes.push((prefix.clone(), reference));
                }
            }
            Some(_) => return Err("prefixes are a map of prefix to IRI".to_owned()),
        }
        let imports = top
            .get("imports")
            .map(text_list)
            .unwrap_or_default()
            .iter()
            .map(py_str)
            .collect();
        let emit_prefixes = top
            .get("emit_prefixes")
            .map(text_list)
            .unwrap_or_default()
            .iter()
            .map(py_str)
            .collect();
        Ok(Self {
            default_prefix: text("default_prefix"),
            default_range: text("default_range"),
            version: text("version"),
            classes: named(top.get("classes"), "class", &class_like)?,
            slots: named(top.get("slots"), "slot", &slot_like)?,
            enums: named(top.get("enums"), "enum", &enum_like)?,
            types: named(top.get("types"), "type", &|t| {
                check_keys(t, fields::TYPE)?;
                common(t)
            })?,
            subsets: named(top.get("subsets"), "subset", &|s| {
                check_keys(s, fields::SUBSET)?;
                common(s)
            })?,
            id,
            name,
            prefixes,
            emit_prefixes,
            imports,
        })
    }

    fn linkml_types() -> Self {
        let mut types = Map::new();
        for (name, uri) in LINKML_TYPES {
            let mut t = Map::new();
            t.insert("uri".to_owned(), Value::String(uri.to_owned()));
            types.insert(name.to_owned(), Value::Object(t));
        }
        Self {
            id: "https://w3id.org/linkml/types".to_owned(),
            name: "types".to_owned(),
            default_prefix: Some("linkml".to_owned()),
            default_range: Some("string".to_owned()),
            prefixes: LINKML_TYPES_PREFIXES
                .iter()
                .map(|(p, r)| ((*p).to_owned(), (*r).to_owned()))
                .collect(),
            types,
            ..Self::default()
        }
    }
}

// ------------------------------------------------------------------ the merged view

/// Where an element was declared: the schema whose default prefix names it.
#[derive(Debug, Clone)]
struct Origin {
    id: String,
    default_prefix: Option<String>,
}

/// The merged model and what `SchemaView` answers about it.
#[derive(Debug)]
pub(super) struct View {
    /// The main schema with the closure merged in (imports emptied).
    pub schema: Schema,
    /// `SchemaView.namespaces()`: the merged prefixes, then each schema of the closure in the
    /// order it was loaded, a later schema overriding an earlier value.
    namespaces: Vec<(String, String)>,
    class_origin: HashMap<String, Origin>,
    slot_origin: HashMap<String, Origin>,
}

/// The texts a request may import, by name: the shipped core, `linkml:types` and the platform
/// models the caller resolved.
pub(super) struct Imports<'a> {
    pub models: &'a BTreeMap<String, String>,
    pub limits: Limits,
}

fn closure_order(
    main: &str,
    imports_of: &mut dyn FnMut(&str) -> Result<Vec<String>, String>,
) -> Result<(Vec<String>, Vec<String>), String> {
    // imports_closure: a FILO walk, each schema appended in front, duplicates dropped keeping
    // the first; `loaded` is the order schema_map learns them in.
    let mut closure: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut visited = HashSet::new();
    let mut loaded = vec![main.to_owned()];
    let mut todo = vec![main.to_owned()];
    while let Some(sn) = todo.pop() {
        if !loaded.contains(&sn) {
            loaded.push(sn.clone());
        }
        if !visited.contains(&sn) {
            for i in imports_of(&sn)? {
                if i != sn {
                    todo.push(i);
                }
            }
        }
        closure.push_front(sn.clone());
        visited.insert(sn);
    }
    let mut seen = HashSet::new();
    let order = closure
        .into_iter()
        .filter(|s| seen.insert(s.clone()))
        .collect();
    Ok((order, loaded))
}

impl View {
    /// Loads `source` with its imports and merges them (`SchemaView` + `merge_imports`).
    pub(super) fn load(source: &str, imports: &Imports<'_>) -> Result<Self, String> {
        let mut used = 0usize;
        let main = Schema::from_document(&parse_yaml(source, imports.limits, &mut used)?)?;
        let mut map: HashMap<String, Schema> = HashMap::new();
        let main_name = main.name.clone();
        map.insert(main_name.clone(), main);
        let (order, loaded) = closure_order(&main_name, &mut |name| {
            if !map.contains_key(name) {
                let schema = match name {
                    "linkml:types" => Schema::linkml_types(),
                    "ngsi-ld-core" => Schema::from_document(&parse_yaml(
                        NGSI_LD_CORE,
                        imports.limits,
                        &mut used,
                    )?)?,
                    other => match imports.models.get(other) {
                        Some(text) => {
                            Schema::from_document(&parse_yaml(text, imports.limits, &mut used)?)?
                        }
                        None => {
                            return Err(format!(
                                "import '{other}' is not one Model Tools resolves: a model \
                                 imports linkml:types, ngsi-ld-core and the platform models the \
                                 request resolves (org.{{name}}.v{{major}}, \
                                 project.{{name}}.v{{major}}, DM-76); nothing is fetched"
                            ));
                        }
                    },
                };
                map.insert(name.to_owned(), schema);
            }
            Ok(map.get(name).map(|s| s.imports.clone()).unwrap_or_default())
        })?;
        let mut merged = map.get(&main_name).cloned().unwrap_or_default();
        let origin_of = |s: &Schema| Origin {
            id: s.id.clone(),
            default_prefix: s.default_prefix.clone(),
        };
        let mut class_origin = HashMap::new();
        let mut slot_origin = HashMap::new();
        for name in merged.classes.keys() {
            class_origin.insert(name.clone(), origin_of(&merged));
        }
        for name in merged.slots.keys() {
            slot_origin.insert(name.clone(), origin_of(&merged));
        }
        for name in order.iter().filter(|n| **n != main_name) {
            let Some(other) = map.get(name) else { continue };
            for (p, r) in &other.prefixes {
                if !merged.prefixes.iter().any(|(q, _)| q == p) {
                    merged.prefixes.push((p.clone(), r.clone()));
                }
            }
            for (k, v) in &other.classes {
                if !merged.classes.contains_key(k) {
                    merged.classes.insert(k.clone(), v.clone());
                    class_origin.insert(k.clone(), origin_of(other));
                }
            }
            for (k, v) in &other.slots {
                if !merged.slots.contains_key(k) {
                    merged.slots.insert(k.clone(), v.clone());
                    slot_origin.insert(k.clone(), origin_of(other));
                }
            }
            for (target, source) in [
                (&mut merged.enums, &other.enums),
                (&mut merged.subsets, &other.subsets),
                (&mut merged.types, &other.types),
            ] {
                for (k, v) in source {
                    if !target.contains_key(k) {
                        target.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        merged.imports.clear();
        let mut namespaces = merged.prefixes.clone();
        for name in loaded.iter().filter(|n| **n != main_name) {
            let Some(other) = map.get(name) else { continue };
            for (p, r) in &other.prefixes {
                match namespaces.iter_mut().find(|(q, _)| q == p) {
                    Some(entry) => entry.1.clone_from(r),
                    None => namespaces.push((p.clone(), r.clone())),
                }
            }
        }
        Ok(Self {
            schema: merged,
            namespaces,
            class_origin,
            slot_origin,
        })
    }

    #[must_use]
    pub(super) fn classes(&self) -> impl Iterator<Item = (&String, &Obj)> {
        self.schema
            .classes
            .iter()
            .filter_map(|(k, v)| v.as_object().map(|o| (k, o)))
    }

    #[must_use]
    pub(super) fn class(&self, name: &str) -> Option<&Obj> {
        self.schema.classes.get(name).and_then(Value::as_object)
    }

    fn class_strict(&self, name: &str) -> Result<&Obj, String> {
        self.class(name)
            .ok_or_else(|| format!("No such class: \"{name}\""))
    }

    #[must_use]
    pub(super) fn enums(&self) -> impl Iterator<Item = (&String, &Obj)> {
        self.schema
            .enums
            .iter()
            .filter_map(|(k, v)| v.as_object().map(|o| (k, o)))
    }

    #[must_use]
    pub(super) fn is_class(&self, name: &str) -> bool {
        self.schema.classes.contains_key(name)
    }

    #[must_use]
    pub(super) fn is_enum(&self, name: &str) -> bool {
        self.schema.enums.contains_key(name)
    }

    #[must_use]
    pub(super) fn is_type(&self, name: &str) -> bool {
        self.schema.types.contains_key(name)
    }

    #[must_use]
    pub(super) fn type_def(&self, name: &str) -> Option<&Obj> {
        self.schema.types.get(name).and_then(Value::as_object)
    }

    /// `all_slots()`: the schema's slots, then every class attribute whose name no slot holds.
    #[must_use]
    pub(super) fn all_slots(&self) -> Vec<(String, &Obj)> {
        let mut out: Vec<(String, &Obj)> = self
            .schema
            .slots
            .iter()
            .filter_map(|(k, v)| v.as_object().map(|o| (k.clone(), o)))
            .collect();
        for (_, class) in self.classes() {
            for (name, attr) in attributes(class) {
                if !out.iter().any(|(n, _)| n == name) {
                    out.push((name.clone(), attr));
                }
            }
        }
        out
    }

    /// `get_slot(name, attributes=...)`.
    fn get_slot(&self, name: &str, with_attributes: bool) -> Option<&Obj> {
        if let Some(slot) = self.schema.slots.get(name).and_then(Value::as_object) {
            return Some(slot);
        }
        if !with_attributes {
            return None;
        }
        self.classes()
            .filter_map(|(_, c)| attributes(c).find(|(n, _)| *n == name).map(|(_, a)| a))
            .last()
    }

    /// `class_ancestors(name)`: reflexive, mixins then is_a, depth first.
    pub(super) fn class_ancestors(&self, name: &str) -> Result<Vec<String>, String> {
        closure(name, |n| {
            let class = self.class_strict(n)?;
            Ok(parents(class))
        })
    }

    fn slot_ancestors(&self, name: &str) -> Result<Vec<String>, String> {
        closure(name, |n| {
            let slot = self
                .get_slot(n, true)
                .ok_or_else(|| format!("No such slot: \"{n}\""))?;
            Ok(parents(slot))
        })
    }

    /// `class_slots(name)`: the slots and attributes of the class and its ancestors, in order,
    /// each once.
    pub(super) fn class_slots(&self, name: &str) -> Result<Vec<String>, String> {
        let mut out: Vec<String> = Vec::new();
        for ancestor in self.class_ancestors(name)? {
            let class = self.class_strict(&ancestor)?;
            let own = texts(class.get("slots"));
            let attrs = attributes(class).map(|(n, _)| n.clone());
            for slot in own.into_iter().chain(attrs) {
                if !out.contains(&slot) {
                    out.push(slot);
                }
            }
        }
        Ok(out)
    }

    /// `induced_slot(slot, class)`: the slot with everything inherited and every slot_usage of
    /// the class's ancestors applied.
    pub(super) fn induced_slot(&self, slot_name: &str, class: Option<&str>) -> Result<Obj, String> {
        let mut from_attribute = false;
        let slot = match class {
            Some(class) => {
                let mut slot = self.get_slot(slot_name, false);
                for ancestor in self.class_ancestors(class)? {
                    let a = self.class_strict(&ancestor)?;
                    if let Some((_, attr)) = attributes(a).find(|(n, _)| *n == slot_name) {
                        slot = Some(attr);
                        from_attribute = true;
                        break;
                    }
                }
                slot
            }
            None => self.get_slot(slot_name, true),
        };
        let Some(slot) = slot else {
            return Err(format!(
                "No such slot {slot_name} as an attribute of {} ancestors or as a slot definition \
                 in the schema",
                class.unwrap_or("None")
            ));
        };
        let mut induced = slot.clone();
        if !from_attribute {
            for ancestor in self.slot_ancestors(slot_name)?.iter().rev() {
                let Some(anc) = self.get_slot(ancestor, false) else {
                    continue;
                };
                for metaslot in INHERITED_SLOT_METASLOTS {
                    if truthy(anc.get(metaslot)) {
                        if let Some(v) = anc.get(metaslot) {
                            induced.insert(metaslot.to_owned(), v.clone());
                        }
                    }
                }
            }
        }
        let propagated = match class {
            Some(class) => self.class_ancestors(class)?,
            None => Vec::new(),
        };
        let usages: Vec<&Obj> = propagated
            .iter()
            .rev()
            .filter_map(|an| self.class(an))
            .filter_map(|a| {
                a.get("slot_usage")
                    .and_then(Value::as_object)
                    .and_then(|u| u.get(slot_name))
                    .and_then(Value::as_object)
            })
            .collect();
        let mut keys: Vec<String> = induced.keys().cloned().collect();
        for usage in &usages {
            for k in usage.keys() {
                if !keys.contains(k) {
                    keys.push(k.clone());
                }
            }
        }
        if !keys.iter().any(|k| k == "range") {
            keys.push("range".to_owned());
        }
        for metaslot in keys {
            let mut v = induced.get(&metaslot).filter(|v| !v.is_null()).cloned();
            for usage in &usages {
                let v2 = usage.get(&metaslot).filter(|v| !v.is_null());
                match (&v, v2) {
                    (None, _) => v = v2.cloned(),
                    (Some(current), Some(other))
                        if metaslot == "maximum_value" || metaslot == "minimum_value" =>
                    {
                        if let (Some(a), Some(b)) = (current.as_f64(), other.as_f64()) {
                            let keep_other = if metaslot == "maximum_value" {
                                b < a
                            } else {
                                b > a
                            };
                            if keep_other {
                                v = Some(other.clone());
                            }
                        }
                    }
                    (Some(_), other) if !is_empty(other) => v = other.cloned(),
                    _ => {}
                }
            }
            if v.is_none() && metaslot == "range" {
                v = self.schema.default_range.clone().map(Value::String);
            }
            if let Some(v) = v {
                induced.insert(metaslot, v);
            }
        }
        if truthy(induced.get("inlined_as_list")) {
            induced.insert("inlined".to_owned(), Value::Bool(true));
        }
        if truthy(induced.get("identifier")) || truthy(induced.get("key")) {
            induced.insert("required".to_owned(), Value::Bool(true));
        }
        Ok(induced)
    }

    /// The origin's default prefix and id of an element `get_uri` builds an IRI for.
    fn origin(&self, class: bool, name: &str) -> Origin {
        let table = if class {
            &self.class_origin
        } else {
            &self.slot_origin
        };
        table.get(name).cloned().unwrap_or_else(|| {
            // An attribute: the schema of the class declaring it.
            let owner = self
                .classes()
                .find(|(_, c)| attributes(c).any(|(n, _)| n == name))
                .and_then(|(c, _)| self.class_origin.get(c).cloned());
            owner.unwrap_or(Origin {
                id: self.schema.id.clone(),
                default_prefix: self.schema.default_prefix.clone(),
            })
        })
    }

    /// `get_uri(element, expand=...)` of a class (`class_uri`) or slot (`slot_uri`).
    #[must_use]
    pub(super) fn get_uri(&self, class: bool, name: &str, element: &Obj, expand: bool) -> String {
        let declared = element
            .get(if class { "class_uri" } else { "slot_uri" })
            .filter(|v| !v.is_null())
            .map(py_str);
        let uri = declared.unwrap_or_else(|| {
            let local = if class {
                camelcase(name)
            } else {
                underscore(name)
            };
            let origin = self.origin(class, name);
            let pfx = origin.default_prefix.unwrap_or_else(|| "None".to_owned());
            let id_sfx = if origin.id.ends_with('/') || origin.id.ends_with('#') {
                origin.id.clone()
            } else {
                format!("{}/", origin.id)
            };
            if pfx == id_sfx {
                format!("{pfx}{local}")
            } else {
                format!("{pfx}:{local}")
            }
        });
        if expand { self.expand_curie(&uri) } else { uri }
    }

    /// `expand_curie`: a `prefix:local` whose prefix the view knows, else the text as it is.
    #[must_use]
    pub(super) fn expand_curie(&self, uri: &str) -> String {
        let parts: Vec<&str> = uri.split(':').collect();
        if let [pfx, local] = parts.as_slice() {
            if let Some((_, ns)) = self.namespaces.iter().find(|(p, _)| p == pfx) {
                return format!("{ns}{local}");
            }
        }
        uri.to_owned()
    }

    /// `namespaces().curie_for(uri, default_ok=False)`: the first longest namespace the IRI
    /// starts with.
    #[must_use]
    pub(super) fn curie_for(&self, uri: &str) -> Option<String> {
        curie_for(&self.namespaces, uri)
    }
}

/// `Namespaces.curie_for`: the first longest matching namespace, every occurrence of it
/// replaced (Python's `str.replace`).
#[must_use]
pub(super) fn curie_for(namespaces: &[(String, String)], uri: &str) -> Option<String> {
    let mut best: Option<&(String, String)> = None;
    for entry in namespaces {
        if uri.starts_with(entry.1.as_str()) && entry.1.len() > best.map_or(0, |b| b.1.len()) {
            best = Some(entry);
        }
    }
    best.map(|(prefix, ns)| uri.replace(ns.as_str(), &format!("{prefix}:")))
}

/// SlotDefinition `_inherited_slots` (linkml-runtime 1.11.1).
const INHERITED_SLOT_METASLOTS: [&str; 31] = [
    "domain",
    "inherited",
    "readonly",
    "ifabsent",
    "list_elements_unique",
    "list_elements_ordered",
    "shared",
    "key",
    "identifier",
    "designates_type",
    "role",
    "relational_role",
    "range",
    "required",
    "recommended",
    "multivalued",
    "inlined",
    "inlined_as_list",
    "minimum_value",
    "maximum_value",
    "pattern",
    "structured_pattern",
    "value_presence",
    "equals_string",
    "equals_string_in",
    "equals_number",
    "equals_expression",
    "exact_cardinality",
    "minimum_cardinality",
    "maximum_cardinality",
    "array",
];

/// `_closure` of `parents`, reflexive and depth first.
fn closure(
    start: &str,
    parents: impl Fn(&str) -> Result<Vec<String>, String>,
) -> Result<Vec<String>, String> {
    let mut rv = vec![start.to_owned()];
    let mut visited = HashSet::new();
    let mut todo = vec![start.to_owned()];
    while let Some(i) = todo.pop() {
        visited.insert(i.clone());
        for v in parents(&i)? {
            if !visited.contains(&v) && !rv.contains(&v) {
                todo.push(v.clone());
                rv.push(v);
            }
        }
    }
    Ok(rv)
}

/// Mixins, then is_a.
fn parents(element: &Obj) -> Vec<String> {
    let mut out = texts(element.get("mixins"));
    if let Some(is_a) = element.get("is_a").filter(|v| !v.is_null()) {
        out.push(py_str(is_a));
    }
    out
}

/// A normalised list metaslot's texts.
#[must_use]
pub(super) fn texts(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().map(py_str).collect())
        .unwrap_or_default()
}

/// A class's attributes in order.
pub(super) fn attributes(class: &Obj) -> impl Iterator<Item = (&String, &Obj)> {
    class
        .get("attributes")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|m| m.iter())
        .filter_map(|(k, v)| v.as_object().map(|o| (k, o)))
}

/// One annotation's value as text, the way `annotation_value` reads it (`str(value)`).
#[must_use]
pub(super) fn annotation_value(element: &Obj, tag: &str) -> Option<String> {
    element
        .get("annotations")
        .and_then(Value::as_object)
        .and_then(|a| a.get(tag))
        .and_then(|entry| entry.get("value"))
        .filter(|v| !v.is_null())
        .map(py_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_renders_floats_and_json_its_own_way() {
        assert_eq!(py_float(1.0), "1.0");
        assert_eq!(py_float(2.5), "2.5");
        assert_eq!(py_float(1e16), "1e+16");
        assert_eq!(py_float(1e-5), "1e-05");
        assert_eq!(py_float(0.0001), "0.0001");
        assert_eq!(py_float(-123.456), "-123.456");
        assert_eq!(py_float(1234567890123456.0), "1234567890123456.0");
        let v: Value = serde_json::json!({"sk": "Názov", "n": [1, 2.5, true, null], "e": "😀"});
        assert_eq!(
            py_json(&v),
            r#"{"sk": "N\u00e1zov", "n": [1, 2.5, true, null], "e": "\ud83d\ude00"}"#
        );
    }

    #[test]
    fn names_are_underscored_and_camelcased_as_linkml_does() {
        assert_eq!(underscore(" pm2-5 a,b  c "), "pm2_5_ab_c");
        assert_eq!(camelcase("air_quality observed"), "AirQualityObserved");
        assert_eq!(camelcase("AirQuality"), "AirQuality");
        assert_eq!(
            mangled_attribute_name("Air_Quality", "pm-10"),
            "airQuality__pm_10"
        );
        assert_eq!(slot_usage_name("pm-10", "Air Quality"), "Air_Quality_pm_10");
    }

    #[test]
    fn model_imports_are_org_or_project_at_a_major() {
        for ok in ["org.air.v1", "project.a-b2.v0", "org.x.v10"] {
            assert!(is_model_import(ok), "{ok}");
        }
        for bad in [
            "org.Air.v1",
            "org.-a.v1",
            "org.a.v01",
            "team.a.v1",
            "org.a",
            "org..v1",
        ] {
            assert!(!is_model_import(bad), "{bad}");
        }
    }
}
