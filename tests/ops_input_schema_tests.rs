//! T-1659: every operation's input says what each parameter is, and bounds it (AG-64, MF-40).
//!
//! The registry's input schemas are what an MCP client sees in `tools/list` and what the
//! assistant is prompted with, and they are the first thing that checks an input. A property
//! without a description is a guess for the model; an unbounded string or an open object is an
//! input nobody decided to accept.

use joinedcontext_portal::ops::registry;
use serde_json::Value;

fn walk(op: &str, at: &str, schema: &Value, problems: &mut Vec<String>) {
    let Some(object) = schema.as_object() else {
        return;
    };
    let kind = object.get("type");
    let is = |name: &str| {
        kind.is_some_and(|k| k == name || k.as_array().is_some_and(|a| a.iter().any(|t| t == name)))
    };
    if is("string")
        && !["maxLength", "pattern", "enum", "const", "format"]
            .iter()
            .any(|bound| object.contains_key(*bound))
    {
        problems.push(format!(
            "{op} {at}: a string without maxLength, pattern or enum"
        ));
    }
    // A manifest, a sample row, a parameter map: the members are the kind's to check, so the
    // object is bounded by how many it may carry instead of by their names.
    if is("object")
        && object.get("additionalProperties") != Some(&Value::Bool(false))
        && !object.contains_key("maxProperties")
    {
        problems.push(format!("{op} {at}: an object open to any property"));
    }
    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            let here = format!("{at}/{name}");
            if property
                .get("description")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                problems.push(format!("{op} {here}: no description"));
            }
            walk(op, &here, property, problems);
        }
    }
    if let Some(items) = object.get("items") {
        walk(op, &format!("{at}[]"), items, problems);
    }
    for combinator in ["oneOf", "anyOf", "allOf"] {
        for (i, branch) in object
            .get(combinator)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            walk(op, &format!("{at}<{combinator}{i}>"), branch, problems);
        }
    }
}

/// AG-64, MF-40: every property described, every string bounded, every object closed or
/// bounded by how many members it may carry.
#[test]
fn every_input_property_is_described_and_bounded() {
    let mut problems = Vec::new();
    for op in registry() {
        walk(op.name, "", &(op.input)(), &mut problems);
    }
    assert!(
        problems.is_empty(),
        "{} input schema problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}
