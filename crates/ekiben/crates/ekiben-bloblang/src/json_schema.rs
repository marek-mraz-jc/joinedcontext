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
        // Draft 4 `dependencies`: a present key demands other keys, or a schema of the object.
        for (key, dep) in s
            .get("dependencies")
            .and_then(Json::as_object)
            .into_iter()
            .flatten()
        {
            if !obj.contains_key(key) {
                continue;
            }
            match dep {
                Json::Array(names) => {
                    for name in names.iter().filter_map(Json::as_str) {
                        if !obj.contains_key(name) {
                            self.err(field, format!("has a dependency on {name}"));
                        }
                    }
                }
                sub => self.validate(sub, &Json::Object(obj.clone()), field, depth + 1)?,
            }
        }
        let props = s.get("properties").and_then(Json::as_object);
        let patterns = s
            .get("patternProperties")
            .and_then(Json::as_object)
            .into_iter()
            .flatten()
            .map(|(p, sub)| {
                regex::Regex::new(p)
                    .map(|re| (re, sub))
                    .map_err(|e| format!("failed to parse json schema definition: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        // gojsonschema reports unknown keys before it walks the property schemas.
        let mut extra = Vec::new();
        for key in obj.keys() {
            let named = props.is_some_and(|p| p.contains_key(key));
            let patterned = patterns.iter().any(|(re, _)| re.is_match(key));
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
            for (re, sub) in &patterns {
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
    fn a_property_dependency_lists_what_it_needs() {
        let s = r#"{"dependencies":{"card":["billing"]}}"#;
        assert_eq!(run(s, serde_json::json!({"card": 1, "billing": 2})), Ok(()));
        assert_eq!(run(s, serde_json::json!({"other": 1})), Ok(()));
        let refused = run(s, serde_json::json!({"card": 1})).expect_err("billing is missing");
        assert!(refused.contains("has a dependency on billing"), "{refused}");
    }

    fn err(schema: &str, doc: serde_json::Value) -> String {
        run(schema, doc).expect_err("the document must be refused")
    }

    #[test]
    fn schema_text_must_parse_and_be_an_object_or_a_boolean() {
        assert!(
            err("{", serde_json::json!(1)).starts_with("failed to parse json schema definition: ")
        );
        assert_eq!(
            err("[1]", serde_json::json!(1)),
            "failed to parse json schema definition: Invalid type. Expected: object"
        );
        assert_eq!(run("true", serde_json::json!({"a": 1})), Ok(()));
        assert_eq!(
            err("false", serde_json::json!(1)),
            "(root) false always fails validation"
        );
        // A subschema that is neither object nor boolean constrains nothing.
        assert_eq!(
            run(r#"{"properties":{"a":7}}"#, serde_json::json!({"a": 1})),
            Ok(())
        );
    }

    /// A chain of `levels` definitions, each wrapping a `$ref` to the next in `key` (or a bare
    /// `$ref`), and a document nested as deep where the chain walks properties. Each `$ref` and each
    /// wrapper is one level, so serde_json's own nesting limit is never what refuses it.
    fn chain(key: &str, levels: usize) -> (String, serde_json::Value) {
        let mut defs = serde_json::Map::new();
        for i in 0..levels {
            let r = serde_json::json!({ "$ref": format!("#/definitions/d{}", i + 1) });
            let d = match key {
                "$ref" => r,
                "properties" => serde_json::json!({ "properties": { "a": r } }),
                "patternProperties" => serde_json::json!({ "patternProperties": { "^a$": r } }),
                "additionalProperties" => serde_json::json!({ "additionalProperties": r }),
                "items-schema" => serde_json::json!({ "items": r }),
                // A schema dependency validates the same object again, one level deeper.
                "dependencies" => serde_json::json!({ "dependencies": { "a": r } }),
                other => serde_json::json!({ other: [r] }),
            };
            defs.insert(format!("d{i}"), d);
        }
        defs.insert(format!("d{levels}"), serde_json::json!({}));
        let schema = serde_json::json!({ "definitions": defs, "$ref": "#/definitions/d0" });
        let doc = (0..levels).fold(serde_json::json!(1), |d, _| match key {
            // `items` as a list validates a tuple: the document nests arrays.
            "items" | "items-schema" => serde_json::json!([d]),
            _ => serde_json::json!({ "a": d }),
        });
        (schema.to_string(), doc)
    }

    #[test]
    fn nesting_is_allowed_to_the_limit_and_refused_past_it() {
        let over = Err("json schema: exceeded maximum nesting".to_string());
        // The root `$ref` is one level: MAX_DEPTH - 1 more reach the limit exactly.
        let (ok, doc) = chain("$ref", MAX_DEPTH - 1);
        assert_eq!(run(&ok, doc), Ok(()));
        let (deep, doc) = chain("$ref", MAX_DEPTH);
        assert_eq!(run(&deep, doc), over);
        // A wrapper and its `$ref` are two levels each.
        for key in [
            "properties",
            "allOf",
            "anyOf",
            "patternProperties",
            "additionalProperties",
            "items",
            "items-schema",
            "dependencies",
        ] {
            let (ok, doc) = chain(key, (MAX_DEPTH - 1) / 2);
            assert_eq!(run(&ok, doc), Ok(()), "{key}");
            let (deep, doc) = chain(key, MAX_DEPTH / 2);
            assert_eq!(run(&deep, doc), over, "{key}");
        }
    }

    #[test]
    fn refs_resolve_locally_and_a_cycle_or_missing_key_is_an_error() {
        assert_eq!(
            err(r##"{"$ref":"#"}"##, serde_json::json!(1)),
            "json schema: exceeded maximum nesting"
        );
        assert_eq!(
            err(r##"{"$ref":"#/definitions/none"}"##, serde_json::json!(1)),
            "failed to parse json schema definition: Object has no key '#/definitions/none'"
        );
        let defs = r##"{"$defs":{"n":{"type":"integer"}},"items":{"$ref":"#/$defs/n"}}"##;
        assert_eq!(run(defs, serde_json::json!([1, 2])), Ok(()));
        assert_eq!(
            err(defs, serde_json::json!([1, "x"])),
            "1 invalid type. expected: integer, given: string"
        );
    }

    #[test]
    fn type_accepts_a_list_and_integers_count_as_numbers() {
        let nullable = r#"{"type":["string","null"]}"#;
        assert_eq!(run(nullable, serde_json::json!(null)), Ok(()));
        assert_eq!(run(nullable, serde_json::json!("a")), Ok(()));
        assert_eq!(
            err(nullable, serde_json::json!(true)),
            "(root) invalid type. expected: string,null, given: boolean"
        );
        assert_eq!(run(r#"{"type":"number"}"#, serde_json::json!(3)), Ok(()));
        assert_eq!(run(r#"{"type":"integer"}"#, serde_json::json!(2.0)), Ok(()));
        assert_eq!(
            err(r#"{"type":"integer"}"#, serde_json::json!(1.5)),
            "(root) invalid type. expected: integer, given: number"
        );
        assert_eq!(
            err(r#"{"type":"array"}"#, serde_json::json!({})),
            "(root) invalid type. expected: array, given: object"
        );
        assert_eq!(
            err(r#"{"type":"object"}"#, serde_json::json!([1])),
            "(root) invalid type. expected: object, given: array"
        );
        // A type keyword that names no type constrains nothing.
        assert_eq!(run(r#"{"type":5}"#, serde_json::json!("x")), Ok(()));
        assert_eq!(run(r#"{"type":[5]}"#, serde_json::json!("x")), Ok(()));
    }

    #[test]
    fn enum_and_const_compare_whole_values() {
        let e = r#"{"enum":["a",1]}"#;
        assert_eq!(run(e, serde_json::json!(1)), Ok(()));
        assert_eq!(
            err(e, serde_json::json!("b")),
            r#"(root) (root) must be one of the following: "a", 1"#
        );
        let c = r#"{"properties":{"k":{"const":{"x":1}}}}"#;
        assert_eq!(run(c, serde_json::json!({"k": {"x": 1}})), Ok(()));
        assert_eq!(
            err(c, serde_json::json!({"k": {"x": 2}})),
            r#"k k does not match: {"x":1}"#
        );
    }

    #[test]
    fn combinators_count_the_branches_that_pass() {
        let all = r#"{"allOf":[{"type":"integer"},{"minimum":5}]}"#;
        assert_eq!(run(all, serde_json::json!(5)), Ok(()));
        assert_eq!(
            err(all, serde_json::json!(4)),
            "(root) must be greater than or equal to 5\n(root) must validate all the schemas (allof)"
        );
        let any = r#"{"anyOf":[{"type":"string"},{"minimum":5}]}"#;
        assert_eq!(run(any, serde_json::json!("x")), Ok(()));
        assert_eq!(run(any, serde_json::json!(9)), Ok(()));
        assert_eq!(
            err(any, serde_json::json!(1)),
            "(root) must validate at least one schema (anyof)"
        );
        let one = r#"{"oneOf":[{"type":"integer"},{"minimum":5}]}"#;
        assert_eq!(run(one, serde_json::json!(1)), Ok(()));
        assert_eq!(run(one, serde_json::json!(5.5)), Ok(()));
        let one_msg = "(root) must validate one and only one schema (oneof)";
        assert_eq!(err(one, serde_json::json!(7)), one_msg);
        assert_eq!(err(one, serde_json::json!(2.5)), one_msg);
        let not = r#"{"not":{"type":"string"}}"#;
        assert_eq!(run(not, serde_json::json!(1)), Ok(()));
        assert_eq!(
            err(not, serde_json::json!("s")),
            "(root) must not validate the schema (not)"
        );
    }

    #[test]
    fn object_counts_patterns_and_additional_properties() {
        let counts = r#"{"minProperties":1,"maxProperties":2}"#;
        assert_eq!(run(counts, serde_json::json!({"a": 1})), Ok(()));
        assert_eq!(run(counts, serde_json::json!({"a": 1, "b": 2})), Ok(()));
        assert_eq!(
            err(counts, serde_json::json!({})),
            "(root) must have at least 1 properties"
        );
        assert_eq!(
            err(counts, serde_json::json!({"a": 1, "b": 2, "c": 3})),
            "(root) must have at most 2 properties"
        );
        let pat = r#"{"properties":{"id":{"type":"string"}},"patternProperties":{"^n_":{"type":"integer"}},"additionalProperties":{"type":"boolean"}}"#;
        assert_eq!(
            run(pat, serde_json::json!({"id": "a", "n_x": 1, "flag": true})),
            Ok(())
        );
        assert_eq!(
            err(pat, serde_json::json!({"id": 1, "n_x": "a", "flag": 1})),
            "flag invalid type. expected: boolean, given: integer\nid invalid type. expected: string, given: integer\nn_x invalid type. expected: integer, given: string"
        );
        let closed = r#"{"patternProperties":{"^n_":{}},"additionalProperties":false}"#;
        assert_eq!(run(closed, serde_json::json!({"n_a": 1})), Ok(()));
        assert_eq!(
            err(closed, serde_json::json!({"n_a": 1, "o": 2})),
            "(root) oadditional property o is not allowed"
        );
        assert!(err(
            r#"{"patternProperties":{"(":{}}}"#,
            serde_json::json!({"a": 1})
        )
        .starts_with("failed to parse json schema definition: "));
        let nested = r#"{"properties":{"a":{"properties":{"b":{"type":"string"}}}}}"#;
        assert_eq!(
            err(nested, serde_json::json!({"a": {"b": 1}})),
            "a.b invalid type. expected: string, given: integer"
        );
    }

    #[test]
    fn array_items_tuple_bounds_and_uniqueness() {
        let tuple = r#"{"items":[{"type":"string"},{"type":"integer"}]}"#;
        assert_eq!(run(tuple, serde_json::json!(["a", 1, true])), Ok(()));
        assert_eq!(
            err(tuple, serde_json::json!([1])),
            "0 invalid type. expected: string, given: integer"
        );
        let bounds = r#"{"minItems":1,"maxItems":2}"#;
        assert_eq!(run(bounds, serde_json::json!([1])), Ok(()));
        assert_eq!(run(bounds, serde_json::json!([1, 2])), Ok(()));
        assert_eq!(
            err(bounds, serde_json::json!([])),
            "(root) array must have at least 1 items"
        );
        assert_eq!(
            err(bounds, serde_json::json!([1, 2, 3])),
            "(root) array must have at most 2 items"
        );
        let unique = r#"{"uniqueItems":true}"#;
        assert_eq!(run(unique, serde_json::json!([1, 2, 3])), Ok(()));
        assert_eq!(
            err(unique, serde_json::json!([1, 2, 1, 2])),
            "(root) array items[0,2] must be unique"
        );
        assert_eq!(
            err(unique, serde_json::json!([3, 1, 1])),
            "(root) array items[1,2] must be unique"
        );
        assert_eq!(
            run(r#"{"uniqueItems":false}"#, serde_json::json!([1, 1])),
            Ok(())
        );
        let in_obj = r#"{"properties":{"l":{"items":{"type":"string"}}}}"#;
        assert_eq!(
            err(in_obj, serde_json::json!({"l": ["a", 2]})),
            "l.1 invalid type. expected: string, given: integer"
        );
    }

    #[test]
    fn string_lengths_count_characters_and_patterns_must_match() {
        let len = r#"{"minLength":2,"maxLength":3}"#;
        assert_eq!(run(len, serde_json::json!("éé")), Ok(()));
        assert_eq!(run(len, serde_json::json!("ééé")), Ok(()));
        assert_eq!(
            err(len, serde_json::json!("é")),
            "(root) string length must be greater than or equal to 2"
        );
        assert_eq!(
            err(len, serde_json::json!("éééé")),
            "(root) string length must be less than or equal to 3"
        );
        let pat = r#"{"pattern":"^[a-z]+$"}"#;
        assert_eq!(run(pat, serde_json::json!("abc")), Ok(()));
        assert_eq!(
            err(pat, serde_json::json!("Abc")),
            "(root) does not match pattern '^[a-z]+$'"
        );
        assert!(err(r#"{"pattern":"("}"#, serde_json::json!("a"))
            .starts_with("failed to parse json schema definition: "));
    }

    #[test]
    fn number_bounds_in_draft_4_and_draft_6_spelling() {
        let incl = r#"{"minimum":1,"maximum":3}"#;
        assert_eq!(run(incl, serde_json::json!(1)), Ok(()));
        assert_eq!(run(incl, serde_json::json!(3)), Ok(()));
        assert_eq!(
            err(incl, serde_json::json!(0.5)),
            "(root) must be greater than or equal to 1"
        );
        assert_eq!(
            err(incl, serde_json::json!(3.5)),
            "(root) must be less than or equal to 3"
        );
        let d4 = r#"{"minimum":1,"exclusiveMinimum":true,"maximum":3,"exclusiveMaximum":true}"#;
        assert_eq!(run(d4, serde_json::json!(2)), Ok(()));
        assert_eq!(
            err(d4, serde_json::json!(1)),
            "(root) must be greater than 1"
        );
        assert_eq!(err(d4, serde_json::json!(3)), "(root) must be less than 3");
        assert_eq!(
            err(d4, serde_json::json!(0)),
            "(root) must be greater than 1"
        );
        let d4_off =
            r#"{"minimum":1,"exclusiveMinimum":false,"maximum":3,"exclusiveMaximum":false}"#;
        assert_eq!(run(d4_off, serde_json::json!(1)), Ok(()));
        assert_eq!(run(d4_off, serde_json::json!(3)), Ok(()));
        let d6 = r#"{"exclusiveMinimum":1,"exclusiveMaximum":3}"#;
        assert_eq!(run(d6, serde_json::json!(2)), Ok(()));
        assert_eq!(run(d6, serde_json::json!(1.5)), Ok(()));
        assert_eq!(
            err(d6, serde_json::json!(1)),
            "(root) must be greater than 1"
        );
        assert_eq!(err(d6, serde_json::json!(3)), "(root) must be less than 3");
        let mult = r#"{"multipleOf":0.1}"#;
        assert_eq!(run(mult, serde_json::json!(0.3)), Ok(()));
        assert_eq!(
            err(mult, serde_json::json!(0.35)),
            "(root) must be a multiple of 0.1"
        );
        assert_eq!(
            err(r#"{"multipleOf":2}"#, serde_json::json!(3)),
            "(root) must be a multiple of 2"
        );
        assert_eq!(run(r#"{"multipleOf":0}"#, serde_json::json!(3)), Ok(()));
        assert_eq!(run(r#"{"multipleOf":-2}"#, serde_json::json!(3)), Ok(()));
        // Bounds only constrain numbers.
        assert_eq!(run(incl, serde_json::json!("9")), Ok(()));
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
