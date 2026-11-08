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

/// gojsonschema's format checkers (v1.2.0 `format_checkers.go`), which Bento's processor runs.
fn format_ok(format: &str, s: &str) -> bool {
    match format {
        // time.Parse with any of "15:04:05", "15:04:05Z07:00", "2006-01-02", RFC3339(Nano).
        "date-time" => {
            go_clock(s).is_some_and(|rest| rest.is_empty() || go_zone(rest))
                || go_date(s).is_some_and(|rest| {
                    rest.is_empty()
                        || rest
                            .strip_prefix('T')
                            .and_then(go_clock)
                            .is_some_and(go_zone)
                })
        }
        "date" => go_date(s) == Some(""),
        "time" => go_clock(s).is_some_and(|rest| rest.is_empty() || go_zone(rest)),
        // url.Parse succeeds with a scheme, and the text holds no backslash.
        "uri" => go_url_has_scheme(s) && !s.contains('\\'),
        "email" => go_mail_address(s),
        _ => true,
    }
}

fn digits(s: &str, n: usize) -> Option<(u32, &str)> {
    let head = s.get(..n)?;
    head.bytes()
        .all(|c| c.is_ascii_digit())
        .then(|| (head.parse().unwrap_or(0), &s[n..]))
}

/// Go's "15:04:05" with a fraction after the seconds (time.Parse takes one even when the layout
/// has none): the hour has one or two digits; the rest of the text is answered.
fn go_clock(s: &str) -> Option<&str> {
    let two = s.as_bytes().get(1).is_some_and(u8::is_ascii_digit);
    let (hour, rest) = digits(s, if two { 2 } else { 1 })?;
    let (min, rest) = digits(rest.strip_prefix(':')?, 2)?;
    let (sec, rest) = digits(rest.strip_prefix(':')?, 2)?;
    if hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    let b = rest.as_bytes();
    if b.len() >= 2 && matches!(b[0], b'.' | b',') && b[1].is_ascii_digit() {
        let n = 1 + b[1..].iter().take_while(|c| c.is_ascii_digit()).count();
        return Some(&rest[n..]);
    }
    Some(rest)
}

/// Go's "Z07:00" to the end of the text: `Z`, or a sign, two digits, a colon and two digits (Go
/// checks no range here).
fn go_zone(s: &str) -> bool {
    if s == "Z" {
        return true;
    }
    let b = s.as_bytes();
    b.len() == 6
        && matches!(b[0], b'+' | b'-')
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3] == b':'
        && b[4].is_ascii_digit()
        && b[5].is_ascii_digit()
}

/// Go's "2006-01-02", a day the month has; the rest of the text is answered.
fn go_date(s: &str) -> Option<&str> {
    let (year, rest) = digits(s, 4)?;
    let (month, rest) = digits(rest.strip_prefix('-')?, 2)?;
    let (day, rest) = digits(rest.strip_prefix('-')?, 2)?;
    chrono::NaiveDate::from_ymd_opt(year as i32, month, day)?;
    Some(rest)
}

/// Go's `unescape`: every `%` starts two hex digits; in a host (`encodeHost`) an escape below
/// %80 other than %25 is refused, and so is any ASCII byte a host may not hold.
fn go_unescape_ok(s: &str, host: bool) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if i + 2 >= b.len() || !b[i + 1].is_ascii_hexdigit() || !b[i + 2].is_ascii_hexdigit() {
                return false;
            }
            if host && (b[i + 1] as char).to_digit(16).unwrap_or(0) < 8 && &s[i..i + 3] != "%25" {
                return false;
            }
            i += 3;
            continue;
        }
        if host
            && b[i] < 0x80
            && !(b[i].is_ascii_alphanumeric() || b"-_.~!$&'()*+,;=:[]<>\"".contains(&b[i]))
        {
            return false;
        }
        i += 1;
    }
    true
}

/// Go's `url.Parse` (net/url) as far as it can fail, and whether it finds a scheme.
fn go_url_has_scheme(raw: &str) -> bool {
    if raw.bytes().any(|c| c < 0x20 || c == 0x7f) {
        return false;
    }
    let (rest, fragment) = raw.split_once('#').unwrap_or((raw, ""));
    if !go_unescape_ok(fragment, false) {
        return false;
    }
    // getScheme: a letter, then letters, digits, `+`, `-` or `.` up to the colon.
    let Some((scheme, rest)) = rest.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    if !(chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    {
        return false;
    }
    // The query is kept as sent; an opaque URL (no leading slash) is not looked into.
    let rest = rest.split_once('?').map_or(rest, |(r, _)| r);
    let Some(after) = rest.strip_prefix("//") else {
        return rest.starts_with('/') && go_unescape_ok(rest, false) || !rest.starts_with('/');
    };
    let (authority, path) = after.find('/').map_or((after, ""), |i| after.split_at(i));
    let host = match authority.rfind('@') {
        Some(at) => {
            let user = &authority[..at];
            let valid = user.bytes().all(|c| {
                c.is_ascii_alphanumeric() || b"-._:~!$&'()*+,;=%@".contains(&c) || c >= 0x80
            });
            if !valid || !go_unescape_ok(user, false) {
                return false;
            }
            &authority[at + 1..]
        }
        None => authority,
    };
    let port_ok = |p: &str| {
        p.is_empty()
            || p.strip_prefix(':')
                .is_some_and(|d| d.bytes().all(|c| c.is_ascii_digit()))
    };
    let name = if host.starts_with('[') {
        let Some(close) = host.rfind(']') else {
            return false;
        };
        if !port_ok(&host[close + 1..]) {
            return false;
        }
        &host[..=close]
    } else {
        match host.rfind(':') {
            Some(i) if !port_ok(&host[i..]) => return false,
            Some(i) => &host[..i],
            None => host,
        }
    };
    go_unescape_ok(name, true) && go_unescape_ok(path, false)
}

/// Go's `mail.ParseAddress`, for the forms an address takes: `local@domain`, or a display name
/// and `<local@domain>`. ponytail: comments and obsolete syntax are refused; upgrade path: port
/// net/mail's parser when a seed's schema needs them.
fn go_mail_address(s: &str) -> bool {
    let s = s.trim_matches(|c| c == ' ' || c == '\t');
    if let Some(inner) = s.strip_suffix('>') {
        let Some(open) = inner.rfind('<') else {
            return false;
        };
        let name = inner[..open].trim_end();
        return (name.is_empty() || go_phrase(name)) && go_addr_spec(&inner[open + 1..]);
    }
    go_addr_spec(s)
}

fn atext(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-/=?^_`{|}~".contains(c) || !c.is_ascii()
}

fn go_dot_atom(s: &str) -> bool {
    !s.is_empty() && s.split('.').all(|a| !a.is_empty() && a.chars().all(atext))
}

fn go_quoted(s: &str) -> bool {
    let Some(inner) = s.strip_prefix('"').and_then(|r| r.strip_suffix('"')) else {
        return false;
    };
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.next().is_none() => return false,
            '"' => return false,
            c if c.is_control() && c != '\t' => return false,
            _ => {}
        }
    }
    true
}

fn go_addr_spec(s: &str) -> bool {
    let Some(at) = s.rfind('@') else {
        return false;
    };
    let (local, domain) = (&s[..at], &s[at + 1..]);
    (go_dot_atom(local) || go_quoted(local))
        && (go_dot_atom(domain)
            || (domain.starts_with('[')
                && domain.ends_with(']')
                && !domain[1..domain.len() - 1].contains(['[', ']', '\\'])))
}

fn go_phrase(s: &str) -> bool {
    let mut rest = s;
    while !rest.is_empty() {
        let word = if let Some(quoted) = rest.strip_prefix('"') {
            match quoted.find('"') {
                Some(end) => &rest[..end + 2],
                None => return false,
            }
        } else {
            rest.split([' ', '\t']).next().unwrap_or(rest)
        };
        if !(go_quoted(word) || word.chars().all(|c| atext(c) || c == '.')) {
            return false;
        }
        rest = rest[word.len()..].trim_start_matches([' ', '\t']);
    }
    true
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
        assert_eq!(
            read_schema_path("file://schemas/a.json", Some(&root)).unwrap(),
            "{}"
        );
        for escape in [
            "../json-schema-outside.json",
            "schemas/../../json-schema-outside.json",
            "file:///etc/passwd",
            "/etc/passwd",
        ] {
            let err = read_schema_path(escape, Some(&root)).expect_err(escape);
            assert!(
                err.to_string().contains("inside the file root"),
                "{escape}: {err}"
            );
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
    fn formats_answer_as_bento_does() {
        // Each verdict is Bento 1.22's json_schema processor over the same text.
        for (format, text, ok) in [
            ("date-time", "2026-11-21", true),
            ("date-time", "2026-11-08T10:00:00Z", true),
            ("date-time", "2026-11-08T10:00:00.5+01:00", true),
            ("date-time", "2026-11-08t10:00:00Z", false),
            ("date-time", "2026-11-08T10:00:00z", false),
            ("date-time", "10:00:00", true),
            ("date-time", "10:00:00Z", true),
            ("date-time", "10:00:00+01:00", true),
            ("date-time", "1:00:00", true),
            ("date-time", "2026-11-08T1:00:00Z", true),
            ("date-time", "2026-11-08T10:00:00", false),
            ("date-time", "2026-11-08 10:00:00Z", false),
            ("date-time", "2026-02-30", false),
            ("date-time", "2026-02-30T10:00:00Z", false),
            ("date-time", "2026-11-08T24:00:00Z", false),
            ("date-time", "2026-11-08T10:00:60Z", false),
            ("date-time", "2026-11-08T10:00:00,5Z", true),
            ("date-time", "2026-11-08T10:00:00.123456789123Z", true),
            ("date-time", "2026-11-08T10:00:00+24:00", true),
            ("date-time", "2026-11-08T10:00:00+01:60", true),
            ("date-time", "2026-11-08T10:00:00+0100", false),
            ("date-time", "10:00:00.5", true),
            ("date-time", "10:00", false),
            ("date-time", "2026-1-08", false),
            ("date-time", "2026-11-8", false),
            ("date-time", "+2026-11-08T10:00:00Z", false),
            ("date-time", "2026-11-08T10:00:00.Z", false),
            ("date-time", "2026-11-08T10:00:00Z ", false),
            ("date-time", "", false),
            ("date-time", "2026-11-08T10:00:00-00:00", true),
            ("date-time", "99:00:00", false),
            ("date-time", "23:59:59.999Z", true),
            ("date", "2026-11-21", true),
            ("date", "2026-02-30", false),
            ("date", "2026-2-01", false),
            ("date", "26-02-01", false),
            ("date", "2026-11-21T00:00:00Z", false),
            ("date", "2026-11-21 ", false),
            ("date", "", false),
            ("date", "0000-01-01", true),
            ("time", "10:00:00", true),
            ("time", "10:00:00Z", true),
            ("time", "10:00:00+01:00", true),
            ("time", "1:00:00", true),
            ("time", "10:00:00.5", true),
            ("time", "10:00:00.5Z", true),
            ("time", "24:00:00", false),
            ("time", "10:00", false),
            ("time", "10:00:00z", false),
            ("time", "10:00:00 ", false),
            ("time", "", false),
            ("uri", "urn:ngsi-ld:X:1", true),
            ("uri", "no scheme", false),
            ("uri", "https://www.facebook.com/pages/Ko%C5%BEn%C3%", false),
            ("uri", "https://a/b%2", false),
            ("uri", "https://a/b%zz", false),
            ("uri", "https://a/b c", true),
            ("uri", "https://a b/", false),
            ("uri", "https://a/b\\c", false),
            ("uri", "http://[::1]:80/", true),
            ("uri", "http://a:port/", false),
            ("uri", "http://a:80x/", false),
            ("uri", "mailto:x@y", true),
            ("uri", "x:", true),
            ("uri", "1http://a", false),
            ("uri", "http://a/é", true),
            ("uri", "https://a/?q=%", true),
            ("uri", "https://a/#frag%", false),
            ("uri", "http://a/b\tc", false),
            ("uri", "//a/b", false),
            ("uri", "/rel", false),
            ("uri", "http://a%20b/", false),
            ("uri", "http://user:pa ss@a/", false),
            ("uri", "http://a:/", true),
            ("uri", "HTTP://A/", true),
            ("email", "a@b", true),
            ("email", "@b", false),
            ("email", "a@", false),
            ("email", "a@b@c", false),
            ("email", "a b@c", false),
            ("email", "a@b c", false),
            ("email", "<a@b>", true),
            ("email", "A <a@b>", true),
            ("email", "a@b.c", true),
            ("email", "a.@b", false),
            ("email", "\"a b\"@c", true),
            ("email", "", false),
        ] {
            assert_eq!(format_ok(format, text), ok, "{format} {text:?}");
        }
    }

    #[test]
    fn a_self_reference_stops_at_the_depth_limit() {
        let schema =
            json!({"definitions": {"a": {"$ref": "#/definitions/a"}}, "$ref": "#/definitions/a"});
        let got = errors(schema, json!(1));
        assert_eq!(got, vec!["(root) schema references nest too deeply"]);
    }
}
