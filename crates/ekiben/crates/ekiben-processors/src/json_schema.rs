//! `json_schema` (`internal/impl/jsonschema/processor.go`): checks each message's JSON against a
//! JSON Schema and fails the message with the violations, one `<field> <description>` line each,
//! as Bento's gojsonschema writes them. The Portal renders one per data-model class as the
//! validation stage before every write (crates/portal `pipeline_validation::stage`).
//!
//! Draft-07 validation keywords: type, enum, const, properties, required, additionalProperties,
//! patternProperties, min/maxProperties, items (one schema or a list), additionalItems,
//! min/maxItems, uniqueItems, contains, minimum, maximum, exclusiveMinimum, exclusiveMaximum,
//! multipleOf, min/maxLength, pattern, format (date-time, date, time, uri, email; others pass, as
//! in gojsonschema), allOf, anyOf, oneOf, not, if/then/else, propertyNames, dependencies, and
//! local `$ref` (`#`, `#/definitions/..`, `#/$defs/..`). A remote `$ref` is refused when the
//! stream is built, so a schema is never half applied. Unknown keywords are annotations.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use ekiben_core::{Batch, ComponentConfig, CoreError, ProcContext, Processor, Registry};
use regex::Regex;
use serde_json::Value;

pub fn register(registry: &Registry) {
    registry.register_processor("json_schema", |conf, ctx| {
        Ok(Arc::new(JsonSchemaProcessor::from_config(conf, ctx)?))
    });
}

pub struct JsonSchemaProcessor {
    schema: Schema,
}

impl JsonSchemaProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let text = match (
            conf.config.get("schema").and_then(Value::as_str),
            conf.config.get("schema_path").and_then(Value::as_str),
        ) {
            (Some(s), None) if !s.is_empty() => s.to_string(),
            (None, Some(p)) if !p.is_empty() => read_schema_path(p, ctx.file_root.as_deref())?,
            (Some(_), Some(_)) => {
                return Err(CoreError::config(
                    "json_schema: set either schema or schema_path, not both",
                ))
            }
            _ => {
                return Err(CoreError::config(
                    "json_schema: a schema or schema_path is required",
                ))
            }
        };
        let root: Value = serde_json::from_str(&text)
            .map_err(|e| CoreError::config(format!("json_schema: the schema is not JSON: {e}")))?;
        Ok(Self {
            schema: Schema::compile(root).map_err(CoreError::config)?,
        })
    }
}

/// `schema_path` is `file://<path>` or a path, read inside the stream's file root when one is set.
fn read_schema_path(path: &str, file_root: Option<&std::path::Path>) -> Result<String, CoreError> {
    let rel = path.strip_prefix("file://").unwrap_or(path);
    let full = match file_root {
        Some(root) => {
            let p = std::path::Path::new(rel);
            if p.is_absolute()
                || p.components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(CoreError::config(
                    "json_schema: schema_path must stay inside the file root",
                ));
            }
            root.join(p)
        }
        None => std::path::PathBuf::from(rel),
    };
    std::fs::read_to_string(&full)
        .map_err(|e| CoreError::config(format!("json_schema: cannot read schema_path: {e}")))
}

#[async_trait]
impl Processor for JsonSchemaProcessor {
    fn processor_type(&self) -> &'static str {
        "json_schema"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        for part in batch.iter_mut() {
            let doc = serde_json::from_slice::<Value>(&part.as_bytes_ref())
                .map_err(|e| format!("failed to parse message as JSON: {e}"));
            match doc {
                Ok(doc) => {
                    let errors = self.schema.validate(&doc);
                    if !errors.is_empty() {
                        ctx.on_error(part, errors.join("\n"));
                    }
                }
                Err(e) => ctx.on_error(part, e),
            }
        }
        Ok(vec![batch])
    }
}

/// A checked schema: the document, its patterns compiled once.
pub struct Schema {
    root: Value,
    patterns: HashMap<String, Regex>,
}

const UNSUPPORTED: &[&str] = &[
    "$recursiveRef",
    "$dynamicRef",
    "unevaluatedProperties",
    "unevaluatedItems",
];

impl Schema {
    pub fn compile(root: Value) -> Result<Self, String> {
        let mut patterns = HashMap::new();
        check(&root, &root, &mut patterns, 0)?;
        Ok(Self { root, patterns })
    }

    /// Every violation of `doc`, as `<field> <description>` lines (empty when it passes).
    pub fn validate(&self, doc: &Value) -> Vec<String> {
        let mut errors = Vec::new();
        self.walk(&self.root, doc, "(root)", &mut errors, 0);
        errors
            .into_iter()
            .map(|(field, desc)| format!("{} {}", field_name(&field), desc))
            .collect()
    }

    fn resolve<'a>(&'a self, reference: &str) -> Option<&'a Value> {
        let pointer = reference.strip_prefix('#')?;
        if pointer.is_empty() {
            return Some(&self.root);
        }
        let decoded = pointer
            .split('/')
            .map(|seg| percent_decode(seg).replace("~1", "/").replace("~0", "~"))
            .collect::<Vec<_>>()
            .join("/");
        self.root.pointer(&decoded)
    }

    fn walk(
        &self,
        schema: &Value,
        doc: &Value,
        at: &str,
        out: &mut Vec<(String, String)>,
        depth: usize,
    ) {
        if depth > MAX_DEPTH {
            out.push((at.into(), "schema references nest too deeply".into()));
            return;
        }
        let obj = match schema {
            Value::Bool(true) => return,
            Value::Bool(false) => {
                out.push((at.into(), "false always fails validation".into()));
                return;
            }
            Value::Object(o) => o,
            _ => return,
        };
        // Draft-07: a `$ref` stands for its whole schema; its siblings are ignored.
        if let Some(r) = obj.get("$ref").and_then(Value::as_str) {
            if let Some(target) = self.resolve(r) {
                self.walk(target, doc, at, out, depth + 1);
            }
            return;
        }
        self.combinators(obj, doc, at, out, depth);
        if let Some(t) = obj.get("type") {
            let types: Vec<&str> = match t {
                Value::String(s) => vec![s.as_str()],
                Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            if !types.is_empty() && !types.iter().any(|t| is_type(doc, t)) {
                let expected = if types.len() == 1 {
                    types[0].to_string()
                } else {
                    format!("[{}]", types.join(", "))
                };
                out.push((
                    at.into(),
                    format!(
                        "invalid type. expected: {expected}, given: {}",
                        type_of(doc)
                    ),
                ));
                return;
            }
        }
        if let Some(c) = obj.get("const") {
            if !json_eq(c, doc) {
                out.push((
                    at.into(),
                    format!("{} does not match: {}", field_name(at), c),
                ));
            }
        }
        if let Some(Value::Array(allowed)) = obj.get("enum") {
            if !allowed.iter().any(|a| json_eq(a, doc)) {
                let list: Vec<String> = allowed.iter().map(Value::to_string).collect();
                out.push((
                    at.into(),
                    format!(
                        "{} must be one of the following: {}",
                        field_name(at),
                        list.join(", ")
                    ),
                ));
            }
        }
        match doc {
            Value::Object(map) => self.object(obj, map, at, out, depth),
            Value::Array(items) => self.array(obj, items, at, out, depth),
            Value::String(s) => self.string(obj, s, at, out),
            Value::Number(_) => number(obj, doc, at, out),
            _ => {}
        }
    }

    fn combinators(
        &self,
        obj: &serde_json::Map<String, Value>,
        doc: &Value,
        at: &str,
        out: &mut Vec<(String, String)>,
        depth: usize,
    ) {
        let passes = |s: &Value| {
            let mut e = Vec::new();
            self.walk(s, doc, at, &mut e, depth + 1);
            e.is_empty()
        };
        if let Some(Value::Array(all)) = obj.get("allOf") {
            let before = out.len();
            for s in all {
                self.walk(s, doc, at, out, depth + 1);
            }
            if out.len() > before {
                out.push((at.into(), "must validate all the schemas (allof)".into()));
            }
        }
        if let Some(Value::Array(any)) = obj.get("anyOf") {
            if !any.iter().any(passes) {
                out.push((
                    at.into(),
                    "must validate at least one schema (anyof)".into(),
                ));
            }
        }
        if let Some(Value::Array(one)) = obj.get("oneOf") {
            if one.iter().filter(|s| passes(s)).count() != 1 {
                out.push((
                    at.into(),
                    "must validate one and only one schema (oneof)".into(),
                ));
            }
        }
        if let Some(not) = obj.get("not") {
            if passes(not) {
                out.push((at.into(), "must not validate the schema (not)".into()));
            }
        }
        if let Some(cond) = obj.get("if") {
            let branch = if passes(cond) { "then" } else { "else" };
            if let Some(s) = obj.get(branch) {
                let before = out.len();
                self.walk(s, doc, at, out, depth + 1);
                if out.len() > before {
                    let verdict = if branch == "then" {
                        "valid"
                    } else {
                        "not valid"
                    };
                    out.push((
                        at.into(),
                        format!("must validate \"{branch}\" as \"if\" was {verdict}"),
                    ));
                }
            }
        }
    }

    fn object(
        &self,
        obj: &serde_json::Map<String, Value>,
        map: &serde_json::Map<String, Value>,
        at: &str,
        out: &mut Vec<(String, String)>,
        depth: usize,
    ) {
        if let Some(n) = obj.get("minProperties").and_then(Value::as_u64) {
            if (map.len() as u64) < n {
                out.push((at.into(), format!("must have at least {n} properties")));
            }
        }
        if let Some(n) = obj.get("maxProperties").and_then(Value::as_u64) {
            if map.len() as u64 > n {
                out.push((at.into(), format!("must have at most {n} properties")));
            }
        }
        if let Some(Value::Array(req)) = obj.get("required") {
            for name in req.iter().filter_map(Value::as_str) {
                if !map.contains_key(name) {
                    out.push((at.into(), format!("{name} is required")));
                }
            }
        }
        let props = obj.get("properties").and_then(Value::as_object);
        let pattern_props = obj.get("patternProperties").and_then(Value::as_object);
        for (key, value) in map {
            let child = format!("{at}.{key}");
            let mut matched = false;
            if let Some(s) = props.and_then(|p| p.get(key)) {
                matched = true;
                self.walk(s, value, &child, out, depth + 1);
            }
            if let Some(pp) = pattern_props {
                for (pattern, s) in pp {
                    if self
                        .patterns
                        .get(pattern)
                        .is_some_and(|re| re.is_match(key))
                    {
                        matched = true;
                        self.walk(s, value, &child, out, depth + 1);
                    }
                }
            }
            if !matched {
                match obj.get("additionalProperties") {
                    Some(Value::Bool(false)) => out.push((
                        at.into(),
                        format!("additional property {key} is not allowed"),
                    )),
                    Some(s @ Value::Object(_)) => self.walk(s, value, &child, out, depth + 1),
                    _ => {}
                }
            }
            if let Some(names) = obj.get("propertyNames") {
                let mut e = Vec::new();
                self.walk(names, &Value::String(key.clone()), at, &mut e, depth + 1);
                if !e.is_empty() {
                    out.push((at.into(), format!("property name \"{key}\" does not match")));
                }
            }
        }
        if let Some(Value::Object(deps)) = obj.get("dependencies") {
            for (key, dep) in deps {
                if !map.contains_key(key) {
                    continue;
                }
                match dep {
                    Value::Array(names) => {
                        for name in names.iter().filter_map(Value::as_str) {
                            if !map.contains_key(name) {
                                out.push((at.into(), format!("has a dependency on {name}")));
                            }
                        }
                    }
                    s => self.walk(s, &Value::Object(map.clone()), at, out, depth + 1),
                }
            }
        }
    }

    fn array(
        &self,
        obj: &serde_json::Map<String, Value>,
        items: &[Value],
        at: &str,
        out: &mut Vec<(String, String)>,
        depth: usize,
    ) {
        if let Some(n) = obj.get("minItems").and_then(Value::as_u64) {
            if (items.len() as u64) < n {
                out.push((at.into(), format!("array must have at least {n} items")));
            }
        }
        if let Some(n) = obj.get("maxItems").and_then(Value::as_u64) {
            if items.len() as u64 > n {
                out.push((at.into(), format!("array must have at most {n} items")));
            }
        }
        if obj.get("uniqueItems") == Some(&Value::Bool(true)) {
            'outer: for i in 0..items.len() {
                for j in i + 1..items.len() {
                    if json_eq(&items[i], &items[j]) {
                        out.push((at.into(), format!("array items[{i},{j}] must be unique")));
                        break 'outer;
                    }
                }
            }
        }
        match obj.get("items") {
            Some(Value::Array(list)) => {
                for (i, item) in items.iter().enumerate() {
                    let child = format!("{at}.{i}");
                    match list.get(i) {
                        Some(s) => self.walk(s, item, &child, out, depth + 1),
                        None => match obj.get("additionalItems") {
                            Some(Value::Bool(false)) => {
                                out.push((
                                    at.into(),
                                    "no additional items allowed on array".into(),
                                ));
                                break;
                            }
                            Some(s @ Value::Object(_)) => {
                                self.walk(s, item, &child, out, depth + 1)
                            }
                            _ => {}
                        },
                    }
                }
            }
            Some(s) => {
                for (i, item) in items.iter().enumerate() {
                    self.walk(s, item, &format!("{at}.{i}"), out, depth + 1);
                }
            }
            None => {}
        }
        if let Some(c) = obj.get("contains") {
            let any = items.iter().any(|item| {
                let mut e = Vec::new();
                self.walk(c, item, at, &mut e, depth + 1);
                e.is_empty()
            });
            if !any {
                out.push((at.into(), "at least one of the items must match".into()));
            }
        }
    }

    fn string(
        &self,
        obj: &serde_json::Map<String, Value>,
        s: &str,
        at: &str,
        out: &mut Vec<(String, String)>,
    ) {
        let len = s.chars().count() as u64;
        if let Some(n) = obj.get("minLength").and_then(Value::as_u64) {
            if len < n {
                out.push((
                    at.into(),
                    format!("string length must be greater than or equal to {n}"),
                ));
            }
        }
        if let Some(n) = obj.get("maxLength").and_then(Value::as_u64) {
            if len > n {
                out.push((
                    at.into(),
                    format!("string length must be less than or equal to {n}"),
                ));
            }
        }
        if let Some(p) = obj.get("pattern").and_then(Value::as_str) {
            if self.patterns.get(p).is_some_and(|re| !re.is_match(s)) {
                out.push((at.into(), format!("does not match pattern '{p}'")));
            }
        }
        if let Some(f) = obj.get("format").and_then(Value::as_str) {
            if !format_ok(f, s) {
                out.push((at.into(), format!("does not match format '{f}'")));
            }
        }
    }
}

const MAX_DEPTH: usize = 64;

fn number(
    obj: &serde_json::Map<String, Value>,
    doc: &Value,
    at: &str,
    out: &mut Vec<(String, String)>,
) {
    let Some(v) = doc.as_f64() else { return };
    let get = |k: &str| obj.get(k).and_then(Value::as_f64);
    let show = |k: &str| obj.get(k).map(Value::to_string).unwrap_or_default();
    if let Some(m) = get("multipleOf") {
        if m > 0.0 && ((v / m) - (v / m).round()).abs() > 1e-9 {
            out.push((
                at.into(),
                format!("must be a multiple of {}", show("multipleOf")),
            ));
        }
    }
    if let Some(min) = get("minimum") {
        if v < min {
            out.push((
                at.into(),
                format!("must be greater than or equal to {}", show("minimum")),
            ));
        }
    }
    if let Some(max) = get("maximum") {
        if v > max {
            out.push((
                at.into(),
                format!("must be less than or equal to {}", show("maximum")),
            ));
        }
    }
    if let Some(min) = get("exclusiveMinimum") {
        if v <= min {
            out.push((
                at.into(),
                format!("must be greater than {}", show("exclusiveMinimum")),
            ));
        }
    }
    if let Some(max) = get("exclusiveMaximum") {
        if v >= max {
            out.push((
                at.into(),
                format!("must be less than {}", show("exclusiveMaximum")),
            ));
        }
    }
}

/// Walks the schema once: patterns compile, every `$ref` is local and resolves, no keyword
/// this validator lacks is present.
fn check(
    node: &Value,
    root: &Value,
    patterns: &mut HashMap<String, Regex>,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("json_schema: the schema nests too deeply".into());
    }
    let Value::Object(obj) = node else {
        return Ok(());
    };
    for k in UNSUPPORTED {
        if obj.contains_key(*k) {
            return Err(format!("json_schema: keyword {k} is not supported"));
        }
    }
    if let Some(r) = obj.get("$ref") {
        let r = r.as_str().ok_or("json_schema: $ref must be a string")?;
        let resolvable = r == "#"
            || (r.starts_with("#/") && {
                let decoded = r[1..]
                    .split('/')
                    .map(|seg| percent_decode(seg).replace("~1", "/").replace("~0", "~"))
                    .collect::<Vec<_>>()
                    .join("/");
                root.pointer(&decoded).is_some()
            });
        if !resolvable {
            return Err(format!(
                "json_schema: $ref {r} is not a local reference that resolves"
            ));
        }
    }
    if let Some(p) = obj.get("pattern").and_then(Value::as_str) {
        compile_pattern(p, patterns)?;
    }
    for (key, value) in obj {
        match key.as_str() {
            "properties" | "patternProperties" | "definitions" | "$defs" | "dependencies" => {
                if let Value::Object(children) = value {
                    for (name, child) in children {
                        if key == "patternProperties" {
                            compile_pattern(name, patterns)?;
                        }
                        check(child, root, patterns, depth + 1)?;
                    }
                }
            }
            "items" | "allOf" | "anyOf" | "oneOf" => match value {
                Value::Array(list) => {
                    for child in list {
                        check(child, root, patterns, depth + 1)?;
                    }
                }
                child => check(child, root, patterns, depth + 1)?,
            },
            "additionalProperties"
            | "additionalItems"
            | "not"
            | "if"
            | "then"
            | "else"
            | "contains"
            | "propertyNames" => check(value, root, patterns, depth + 1)?,
            _ => {}
        }
    }
    Ok(())
}

fn compile_pattern(p: &str, patterns: &mut HashMap<String, Regex>) -> Result<(), String> {
    if !patterns.contains_key(p) {
        let re = Regex::new(p)
            .map_err(|e| format!("json_schema: pattern {p:?} does not compile: {e}"))?;
        patterns.insert(p.to_string(), re);
    }
    Ok(())
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// gojsonschema's `Field()`: the root is `(root)`, anything below it drops that prefix.
fn field_name(at: &str) -> &str {
    at.strip_prefix("(root).").unwrap_or(at)
}

fn is_integer(v: &Value) -> bool {
    v.is_i64()
        || v.is_u64()
        || v.as_f64()
            .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
}

fn is_type(v: &Value, t: &str) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => is_integer(v),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        _ => true,
    }
}

fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) if is_integer(v) => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// JSON equality with 1 and 1.0 the same number, as JSON Schema compares.
fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_i64(), y.as_i64()) {
            (Some(x), Some(y)) => x == y,
            _ => x.as_f64() == y.as_f64(),
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| json_eq(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w)))
        }
        _ => a == b,
    }
}

fn format_ok(format: &str, s: &str) -> bool {
    let b = s.as_bytes();
    match format {
        // Go's time.Parse(time.RFC3339, ..): an upper-case T and Z, a 4-digit year.
        "date-time" => {
            b.len() >= 20
                && b[10] == b'T'
                && date_ok(&s[..10])
                && !s.contains('z')
                && chrono::DateTime::parse_from_rfc3339(s).is_ok()
        }
        "date" => date_ok(s),
        "time" => chrono::DateTime::parse_from_rfc3339(&format!("1970-01-01T{s}")).is_ok(),
        // Go's url.Parse with a scheme: a scheme, then no space or control character.
        "uri" => {
            let Some((scheme, _)) = s.split_once(':') else {
                return false;
            };
            let mut chars = scheme.chars();
            chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
                && !s.chars().any(|c| c.is_control() || c == ' ')
        }
        "email" => {
            let mut parts = s.splitn(2, '@');
            matches!((parts.next(), parts.next()), (Some(l), Some(d)) if !l.is_empty() && !d.is_empty() && !d.contains('@'))
        }
        _ => true,
    }
}

fn date_ok(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn errors(schema: Value, doc: Value) -> Vec<String> {
        Schema::compile(schema).unwrap().validate(&doc)
    }

    #[test]
    fn schema_path_stays_inside_the_file_root() {
        let root = std::env::temp_dir().join(format!("json-schema-root-{}", std::process::id()));
        std::fs::create_dir_all(root.join("schemas")).unwrap();
        std::fs::write(root.join("schemas/a.json"), "{}").unwrap();
        std::fs::write(std::env::temp_dir().join("json-schema-outside.json"), "{}").unwrap();
        assert_eq!(read_schema_path("file://schemas/a.json", Some(&root)).unwrap(), "{}");
        for escape in [
            "../json-schema-outside.json",
            "schemas/../../json-schema-outside.json",
            "file:///etc/passwd",
            "/etc/passwd",
        ] {
            let err = read_schema_path(escape, Some(&root)).expect_err(escape);
            assert!(err.to_string().contains("inside the file root"), "{escape}: {err}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_record_of_its_class_passes() {
        let schema = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "definitions": {"P": {"type": "object", "required": ["type", "value"],
                "properties": {"type": {"const": "Property"}, "value": {"type": "number", "minimum": 0}}}},
            "type": "object", "required": ["id", "type"],
            "properties": {
                "id": {"type": "string", "format": "uri"},
                "type": {"const": "Counter"},
                "count": {"$ref": "#/definitions/P"},
                "at": {"type": "string", "format": "date-time"}
            },
            "additionalProperties": false
        });
        let ok = json!({"id": "urn:ngsi-ld:Counter:a", "type": "Counter",
            "count": {"type": "Property", "value": 3}, "at": "2026-11-08T10:00:00Z"});
        assert!(errors(schema.clone(), ok).is_empty());
        let bad = json!({"id": "not a uri", "type": "Other", "count": {"type": "Property", "value": -1},
            "at": "2026-11-08 10:00", "extra": 1});
        let got = errors(schema, bad);
        assert_eq!(
            got,
            vec![
                "id does not match format 'uri'",
                "type type does not match: \"Counter\"",
                "count.value must be greater than or equal to 0",
                "at does not match format 'date-time'",
                "(root) additional property extra is not allowed",
            ],
            "{got:#?}"
        );
    }

    #[test]
    fn required_type_enum_and_any_of_report_as_gojsonschema() {
        let schema = json!({"type": "object", "required": ["name"], "properties": {
            "kind": {"enum": ["a", "b"]},
            "n": {"type": "integer"},
            "v": {"anyOf": [{"type": "string"}, {"type": "null"}]}
        }});
        let got = errors(schema, json!({"kind": "c", "n": 1.5, "v": 3}));
        assert_eq!(
            got,
            vec![
                "(root) name is required",
                "kind kind must be one of the following: \"a\", \"b\"",
                "n invalid type. expected: integer, given: number",
                "v must validate at least one schema (anyof)",
            ]
        );
        assert_eq!(
            errors(json!({"type": "integer"}), json!(2.0)),
            Vec::<String>::new()
        );
        assert_eq!(
            errors(json!({"type": ["string", "null"]}), json!(1)),
            vec!["(root) invalid type. expected: [string, null], given: integer"]
        );
    }

    #[test]
    fn a_remote_ref_or_a_bad_pattern_is_refused_at_build() {
        assert!(Schema::compile(json!({"$ref": "https://example.org/s.json"})).is_err());
        assert!(Schema::compile(json!({"$ref": "#/definitions/missing"})).is_err());
        assert!(Schema::compile(json!({"pattern": "("})).is_err());
        assert!(Schema::compile(json!({"unevaluatedProperties": false})).is_err());
    }

    #[test]
    fn date_formats_follow_go() {
        assert!(format_ok("date-time", "2026-11-08T10:00:00.5+01:00"));
        assert!(!format_ok("date-time", "2026-11-08t10:00:00Z"));
        assert!(!format_ok("date-time", "2026-11-08T10:00:00z"));
        assert!(!format_ok("date-time", "2026-11-08"));
        assert!(format_ok("date", "2026-02-28"));
        assert!(!format_ok("date", "2026-02-30"));
        assert!(!format_ok("date", "26-02-01"));
        assert!(format_ok("uri", "urn:ngsi-ld:X:1"));
        assert!(!format_ok("uri", "no scheme"));
    }

    #[test]
    fn a_self_reference_stops_at_the_depth_limit() {
        let schema =
            json!({"definitions": {"a": {"$ref": "#/definitions/a"}}, "$ref": "#/definitions/a"});
        let got = errors(schema, json!(1));
        assert_eq!(got, vec!["(root) schema references nest too deeply"]);
    }
}
