//! `json_schema` of Bento 1.22 (xeipuuv/gojsonschema v1.2.0): the keywords of drafts 4-7
//! that pipelines use, with gojsonschema's messages as Bento prints them (field, a space,
//! the description lowercased). Only local `$ref`s (`#`, `#/definitions/..`, `#/$defs/..`)
//! resolve: a remote reference would be an outbound call from a mapping.
//! ponytail: `anyOf`/`oneOf` report the aggregate message only, not the closest branch's
//! errors; `format`, `dependencies` and `if/then/else` are not checked.

use serde_json::{Map, Value as Json};

use crate::value::Value;

const MAX_DEPTH: usize = 128;

pub fn check(target: &Value, schema_text: &str) -> Result<(), String> {
    let schema: Json = serde_json::from_str(schema_text)
        .map_err(|e| format!("failed to parse json schema definition: {e}"))?;
    if !matches!(schema, Json::Object(_) | Json::Bool(_)) {
        return Err(
            "failed to parse json schema definition: Invalid type. Expected: object".into(),
        );
    }
    let mut v = Validator {
        root: &schema,
        errors: Vec::new(),
    };
    v.validate(&schema, &target.to_json_value(), "(root)", 0)?;
    if v.errors.is_empty() {
        return Ok(());
    }
    Err(v
        .errors
        .iter()
        .map(|(field, desc, property)| {
            let lower = desc.to_lowercase();
            let text = match property {
                Some(p) => format!(
                    "{p}{}",
                    lower.strip_prefix(&p.to_lowercase()).unwrap_or(&lower)
                ),
                None => lower,
            };
            format!("{field} {text}")
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

struct Validator<'s> {
    root: &'s Json,
    /// (field, description, property detail)
    errors: Vec<(String, String, Option<String>)>,
}

fn type_of(v: &Json) -> &'static str {
    match v {
        Json::Null => "null",
        Json::Bool(_) => "boolean",
        Json::Number(n)
            if n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0) =>
        {
            "integer"
        }
        Json::Number(_) => "number",
        Json::String(_) => "string",
        Json::Array(_) => "array",
        Json::Object(_) => "object",
    }
}

fn child(field: &str, key: &str) -> String {
    if field == "(root)" {
        key.to_string()
    } else {
        format!("{field}.{key}")
    }
}

fn num(v: &Json) -> Option<f64> {
    v.as_f64()
}

fn fmt_num(f: f64) -> String {
    crate::value::format_go_float(f)
}

impl<'s> Validator<'s> {
    fn err(&mut self, field: &str, desc: String) {
        self.errors.push((field.to_string(), desc, None));
    }

    fn resolve(&self, reference: &str) -> Result<&'s Json, String> {
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| format!("failed to parse json schema definition: remote reference {reference} is not allowed"))?;
        self.root.pointer(pointer).ok_or_else(|| {
            format!("failed to parse json schema definition: Object has no key '{reference}'")
        })
    }

    fn passes(&self, schema: &'s Json, value: &Json, depth: usize) -> Result<bool, String> {
        let mut sub = Validator {
            root: self.root,
            errors: Vec::new(),
        };
        sub.validate(schema, value, "(root)", depth + 1)?;
        Ok(sub.errors.is_empty())
    }

    fn validate(
        &mut self,
        schema: &'s Json,
        value: &Json,
        field: &str,
        depth: usize,
    ) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("json schema: exceeded maximum nesting".to_string());
        }
        let s = match schema {
            Json::Bool(true) => return Ok(()),
            Json::Bool(false) => {
                self.err(field, "False always fails validation".into());
                return Ok(());
            }
            Json::Object(s) => s,
            _ => return Ok(()),
        };
        if let Some(Json::String(r)) = s.get("$ref") {
            let target = self.resolve(r)?;
            return self.validate(target, value, field, depth + 1);
        }
        if let Some(t) = s.get("type") {
            let allowed: Vec<&str> = match t {
                Json::String(one) => vec![one.as_str()],
                Json::Array(many) => many.iter().filter_map(Json::as_str).collect(),
                _ => Vec::new(),
            };
            let given = type_of(value);
            let ok = allowed
                .iter()
                .any(|a| *a == given || (*a == "number" && given == "integer"));
            if !allowed.is_empty() && !ok {
                self.err(
                    field,
                    format!(
                        "Invalid type. Expected: {}, given: {given}",
                        allowed.join(",")
                    ),
                );
                return Ok(());
            }
        }
        if let Some(Json::Array(options)) = s.get("enum") {
            if !options.contains(value) {
                let list: Vec<String> = options.iter().map(Json::to_string).collect();
                self.err(
                    field,
                    format!("{field} must be one of the following: {}", list.join(", ")),
                );
            }
        }
        if let Some(c) = s.get("const") {
            if c != value {
                self.err(field, format!("{field} does not match: {c}"));
            }
        }
        self.combinators(s, value, field, depth)?;
        match value {
            Json::Object(obj) => self.object(s, obj, field, depth)?,
            Json::Array(items) => self.array(s, items, field, depth)?,
            Json::String(text) => self.string(s, text, field)?,
            Json::Number(_) => self.number(s, value, field),
            _ => {}
        }
        Ok(())
    }

    fn combinators(
        &mut self,
        s: &'s Map<String, Json>,
        value: &Json,
        field: &str,
        depth: usize,
    ) -> Result<(), String> {
        if let Some(Json::Array(all)) = s.get("allOf") {
            let before = self.errors.len();
            for sub in all {
                self.validate(sub, value, field, depth + 1)?;
            }
            if self.errors.len() > before {
                self.err(field, "Must validate all the schemas (allOf)".into());
            }
        }
        if let Some(Json::Array(any)) = s.get("anyOf") {
            let mut ok = false;
            for sub in any {
                ok |= self.passes(sub, value, depth)?;
            }
            if !ok {
                self.err(field, "Must validate at least one schema (anyOf)".into());
            }
        }
        if let Some(Json::Array(one)) = s.get("oneOf") {
            let mut n = 0;
            for sub in one {
                n += usize::from(self.passes(sub, value, depth)?);
            }
            if n != 1 {
                self.err(
                    field,
                    "Must validate one and only one schema (oneOf)".into(),
                );
            }
        }
        if let Some(not) = s.get("not") {
            if self.passes(not, value, depth)? {
                self.err(field, "Must not validate the schema (not)".into());
            }
        }
        Ok(())
    }

    fn object(
        &mut self,
        s: &'s Map<String, Json>,
        obj: &Map<String, Json>,
        field: &str,
        depth: usize,
    ) -> Result<(), String> {
        if let Some(min) = s.get("minProperties").and_then(num) {
            if (obj.len() as f64) < min {
                self.err(
                    field,
                    format!("Must have at least {} properties", fmt_num(min)),
                );
            }
        }
        if let Some(max) = s.get("maxProperties").and_then(num) {
            if (obj.len() as f64) > max {
                self.err(
                    field,
                    format!("Must have at most {} properties", fmt_num(max)),
                );
            }
        }
        if let Some(Json::Array(req)) = s.get("required") {
            for name in req.iter().filter_map(Json::as_str) {
                if !obj.contains_key(name) {
                    self.errors.push((
                        field.to_string(),
                        format!("{name} is required"),
                        Some(name.to_string()),
                    ));
                }
            }
        }
        let props = s.get("properties").and_then(Json::as_object);
        let patterns = s.get("patternProperties").and_then(Json::as_object);
        // gojsonschema reports unknown keys before it walks the property schemas.
        let mut extra = Vec::new();
        for key in obj.keys() {
            let named = props.is_some_and(|p| p.contains_key(key));
            let mut patterned = false;
            for pattern in patterns.into_iter().flatten().map(|(p, _)| p) {
                let re = regex::Regex::new(pattern)
                    .map_err(|e| format!("failed to parse json schema definition: {e}"))?;
                patterned |= re.is_match(key);
            }
            if !named && !patterned {
                extra.push(key);
            }
        }
        if s.get("additionalProperties") == Some(&Json::Bool(false)) {
            for key in &extra {
                self.errors.push((
                    field.to_string(),
                    format!("Additional property {key} is not allowed"),
                    Some((*key).clone()),
                ));
            }
        }
        for (key, val) in obj {
            if let Some(sub) = props.and_then(|p| p.get(key)) {
                self.validate(sub, val, &child(field, key), depth + 1)?;
            }
            for (pattern, sub) in patterns.into_iter().flatten() {
                let re = regex::Regex::new(pattern)
                    .map_err(|e| format!("failed to parse json schema definition: {e}"))?;
                if re.is_match(key) {
                    self.validate(sub, val, &child(field, key), depth + 1)?;
                }
            }
            if let Some(more @ Json::Object(_)) = s.get("additionalProperties") {
                if extra.contains(&key) {
                    self.validate(more, val, &child(field, key), depth + 1)?;
                }
            }
        }
        Ok(())
    }

    fn array(
        &mut self,
        s: &'s Map<String, Json>,
        items: &[Json],
        field: &str,
        depth: usize,
    ) -> Result<(), String> {
        match s.get("items") {
            Some(Json::Array(tuple)) => {
                for (i, (sub, item)) in tuple.iter().zip(items).enumerate() {
                    self.validate(sub, item, &child(field, &i.to_string()), depth + 1)?;
                }
            }
            Some(sub) => {
                for (i, item) in items.iter().enumerate() {
                    self.validate(sub, item, &child(field, &i.to_string()), depth + 1)?;
                }
            }
            None => {}
        }
        if let Some(min) = s.get("minItems").and_then(num) {
            if (items.len() as f64) < min {
                self.err(
                    field,
                    format!("Array must have at least {} items", fmt_num(min)),
                );
            }
        }
        if let Some(max) = s.get("maxItems").and_then(num) {
            if (items.len() as f64) > max {
                self.err(
                    field,
                    format!("Array must have at most {} items", fmt_num(max)),
                );
            }
        }
        if s.get("uniqueItems") == Some(&Json::Bool(true)) {
            'outer: for i in 0..items.len() {
                for j in i + 1..items.len() {
                    if items[i] == items[j] {
                        self.err(field, format!("array items[{i},{j}] must be unique"));
                        break 'outer;
                    }
                }
            }
        }
        Ok(())
    }

    fn string(&mut self, s: &Map<String, Json>, text: &str, field: &str) -> Result<(), String> {
        let len = text.chars().count() as f64;
        if let Some(min) = s.get("minLength").and_then(num) {
            if len < min {
                self.err(
                    field,
                    format!(
                        "String length must be greater than or equal to {}",
                        fmt_num(min)
                    ),
                );
            }
        }
        if let Some(max) = s.get("maxLength").and_then(num) {
            if len > max {
                self.err(
                    field,
                    format!(
                        "String length must be less than or equal to {}",
                        fmt_num(max)
                    ),
                );
            }
        }
        if let Some(Json::String(p)) = s.get("pattern") {
            let re = regex::Regex::new(p)
                .map_err(|e| format!("failed to parse json schema definition: {e}"))?;
            if !re.is_match(text) {
                self.err(field, format!("Does not match pattern '{p}'"));
            }
        }
        Ok(())
    }

    fn number(&mut self, s: &Map<String, Json>, value: &Json, field: &str) {
        let Some(n) = num(value) else { return };
        let bound = |k: &str| s.get(k).and_then(num);
        // Draft 4 spells exclusive bounds as booleans beside minimum/maximum.
        let excl = |k: &str| s.get(k) == Some(&Json::Bool(true));
        if let Some(min) = bound("minimum") {
            if excl("exclusiveMinimum") && n <= min {
                self.err(field, format!("Must be greater than {}", fmt_num(min)));
            } else if n < min {
                self.err(
                    field,
                    format!("Must be greater than or equal to {}", fmt_num(min)),
                );
            }
        }
        if let Some(max) = bound("maximum") {
            if excl("exclusiveMaximum") && n >= max {
                self.err(field, format!("Must be less than {}", fmt_num(max)));
            } else if n > max {
                self.err(
                    field,
                    format!("Must be less than or equal to {}", fmt_num(max)),
                );
            }
        }
        if let Some(min) = bound("exclusiveMinimum").filter(|m| n <= *m) {
            self.err(field, format!("Must be greater than {}", fmt_num(min)));
        }
        if let Some(max) = bound("exclusiveMaximum").filter(|m| n >= *m) {
            self.err(field, format!("Must be less than {}", fmt_num(max)));
        }
        if let Some(m) = bound("multipleOf").filter(|m| *m > 0.0) {
            let q = n / m;
            if (q - q.round()).abs() > 1e-9 {
                self.err(field, format!("Must be a multiple of {}", fmt_num(m)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(schema: &str, doc: serde_json::Value) -> Result<(), String> {
        check(&Value::from_json_value(doc), schema)
    }

    #[test]
    fn messages_match_gojsonschema_as_bento_prints_them() {
        let s = r#"{"type":"object","required":["Name"],"properties":{"Name":{"type":"string"},"age":{"type":"integer","minimum":0}},"additionalProperties":false}"#;
        assert_eq!(
            run(s, serde_json::json!({"age": -1, "x": 1})),
            // gojsonschema prepends the property to a message that does not start with it.
            Err("(root) Name is required\n(root) xadditional property x is not allowed\nage must be greater than or equal to 0".into())
        );
        assert_eq!(run(s, serde_json::json!({"Name": "a", "age": 3})), Ok(()));
    }

    #[test]
    fn remote_refs_are_refused() {
        let s = r#"{"$ref":"https://example.com/schema.json"}"#;
        assert!(run(s, serde_json::json!({}))
            .unwrap_err()
            .contains("remote reference"));
        let local = r##"{"definitions":{"s":{"type":"string"}},"properties":{"a":{"$ref":"#/definitions/s"}}}"##;
        assert_eq!(
            run(local, serde_json::json!({"a": 1})),
            Err("a invalid type. expected: string, given: integer".into())
        );
    }
}
