// SPDX-License-Identifier: EUPL-1.2
//! The message schemas of `docs/api/asyncapi-ws.yaml`, the published
//! contract (ADR-0026 rule 11), read by the tests that speak the channel:
//! a message the broker sends and the document does not describe fails
//! here, and CI validates the same file with `@asyncapi/cli` (T-3518).
//!
//! The validator knows the JSON Schema keywords the document uses and
//! refuses any other, so a keyword added to the document fails a test
//! before it is silently ignored. `format`, `description`, `title` and
//! `summary` are annotations, as in JSON Schema 2020-12.

use serde_json::Value;
use std::sync::LazyLock;

static DOC: LazyLock<Result<Value, String>> = LazyLock::new(|| {
    serde_saphyr::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/api/asyncapi-ws.yaml"
    )))
    .map_err(|e| format!("asyncapi-ws.yaml does not parse: {e}"))
});

/// The document, parsed.
///
/// # Errors
/// The YAML does not parse.
pub fn doc() -> Result<&'static Value, String> {
    DOC.as_ref().map_err(Clone::clone)
}

/// Check `message` against the payload of `components.messages.<name>`.
///
/// # Errors
/// Each place the message breaks the schema, or the message is unknown.
pub fn check(name: &str, message: &Value) -> Result<(), String> {
    let doc = doc()?;
    let payload = doc
        .pointer(&format!("/components/messages/{name}/payload"))
        .ok_or_else(|| format!("asyncapi-ws.yaml describes no message {name}"))?;
    let mut errors = Vec::new();
    validate(doc, payload, message, "", &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{name} breaks asyncapi-ws.yaml: {errors:?} in {message}"
        ))
    }
}

/// Check a server message against the schema its `x-op` names.
///
/// # Errors
/// As [`check`]; also a message without a known `x-op`.
pub fn check_server(message: &Value) -> Result<(), String> {
    let op = message
        .pointer("/metadata/x-op")
        .and_then(Value::as_str)
        .and_then(|op| op.strip_prefix("x-"))
        .ok_or_else(|| format!("no x-op: {message}"))?;
    check(op, message)
}

fn validate(doc: &Value, schema: &Value, v: &Value, at: &str, errors: &mut Vec<String>) {
    let Some(schema) = schema.as_object() else {
        errors.push(format!("{at}: schema is not an object"));
        return;
    };
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        match r.strip_prefix('#').and_then(|p| doc.pointer(p)) {
            Some(target) => validate(doc, target, v, at, errors),
            None => errors.push(format!("{at}: unresolved $ref {r}")),
        }
        return;
    }
    for (keyword, rule) in schema {
        check_keyword(doc, schema, keyword, rule, v, at, errors);
    }
}

/// One keyword of `schema` on `v`.
fn check_keyword(
    doc: &Value,
    schema: &serde_json::Map<String, Value>,
    keyword: &str,
    rule: &Value,
    v: &Value,
    at: &str,
    errors: &mut Vec<String>,
) {
    let fail = |what: String| format!("{at}: {what}");
    match keyword {
        "format" | "description" | "title" | "summary" => {}
        "type" => {
            let ok = match rule.as_str() {
                Some("object") => v.is_object(),
                Some("array") => v.is_array(),
                Some("string") => v.is_string(),
                Some("integer") => v.is_i64() || v.is_u64(),
                Some("number") => v.is_number(),
                Some("boolean") => v.is_boolean(),
                _ => false,
            };
            if !ok {
                errors.push(fail(format!("not of type {rule}")));
            }
        }
        "const" => {
            if v != rule {
                errors.push(fail(format!("is not {rule}")));
            }
        }
        "enum" => {
            if !rule.as_array().is_some_and(|e| e.contains(v)) {
                errors.push(fail(format!("is not one of {rule}")));
            }
        }
        "required" => {
            for name in rule
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if v.get(name).is_none() {
                    errors.push(fail(format!("misses {name}")));
                }
            }
        }
        "properties" | "additionalProperties" => {
            members(doc, schema, keyword, rule, v, at, errors);
        }
        "items" => {
            for (i, item) in v.as_array().into_iter().flatten().enumerate() {
                validate(doc, rule, item, &format!("{at}/{i}"), errors);
            }
        }
        "oneOf" => {
            let passing = rule
                .as_array()
                .into_iter()
                .flatten()
                .filter(|sub| {
                    let mut e = Vec::new();
                    validate(doc, sub, v, at, &mut e);
                    e.is_empty()
                })
                .count();
            if passing != 1 {
                errors.push(fail(format!("matches {passing} of oneOf, not 1")));
            }
        }
        "pattern" => {
            let (Some(p), Some(s)) = (rule.as_str(), v.as_str()) else {
                return;
            };
            match regex::Regex::new(p) {
                Ok(re) if re.is_match(s) => {}
                Ok(_) => errors.push(fail(format!("{s:?} does not match {p}"))),
                Err(e) => errors.push(fail(format!("pattern {p}: {e}"))),
            }
        }
        "minimum" | "maximum" | "minLength" | "maxLength" => {
            if !within(keyword, rule, v) {
                errors.push(fail(format!("breaks {keyword} {rule}")));
            }
        }
        other => errors.push(fail(format!("keyword {other} is not supported here"))),
    }
}

/// `properties` or `additionalProperties` of `schema` on the members of `v`.
fn members(
    doc: &Value,
    schema: &serde_json::Map<String, Value>,
    keyword: &str,
    rule: &Value,
    v: &Value,
    at: &str,
    errors: &mut Vec<String>,
) {
    let Some(obj) = v.as_object() else {
        return;
    };
    let known = |name: &str| {
        schema
            .get("properties")
            .is_some_and(|p| p.get(name).is_some())
    };
    for (name, member) in obj {
        let sub = if keyword == "properties" {
            rule.get(name)
        } else if known(name) {
            None
        } else {
            Some(rule)
        };
        match sub {
            None | Some(Value::Bool(true)) => {}
            Some(Value::Bool(false)) => errors.push(format!("{at}: has {name}")),
            Some(sub) => validate(doc, sub, member, &format!("{at}/{name}"), errors),
        }
    }
}

/// Whether `v` keeps the bound `keyword` sets (a length counts characters).
fn within(keyword: &str, rule: &Value, v: &Value) -> bool {
    let bound = rule.as_f64().unwrap_or(f64::NAN);
    let got = match (keyword, v) {
        ("minLength" | "maxLength", Value::String(s)) => Some(f64::from(
            u32::try_from(s.chars().count()).unwrap_or(u32::MAX),
        )),
        ("minimum" | "maximum", Value::Number(n)) => n.as_f64(),
        _ => None,
    };
    got.is_none_or(|got| {
        if keyword.starts_with("min") {
            got >= bound
        } else {
            got <= bound
        }
    })
}

/// `schema`, or the target of its local `$ref`.
#[must_use]
pub fn resolve<'a>(doc: &'a Value, schema: &'a Value) -> &'a Value {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| r.strip_prefix('#'))
        .and_then(|p| doc.pointer(p))
        .map_or(schema, |target| resolve(doc, target))
}
