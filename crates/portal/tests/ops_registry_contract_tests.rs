//! One operation registry, two documents (AG-59, AG-146, T-5948): the MCP tool list and the
//! OpenAPI document are generated from the same registry entry, every entry carries a closed input
//! schema and the rule the door applies, and an entry without them is refused.

use joinedcontext_portal::mcp::tool_definition;
use joinedcontext_portal::ops;
use serde_json::{json, Value};

const OPS_PREFIX: &str = "/api/v1/projects/{project}/ops/";

fn document() -> Value {
    serde_json::from_str(&joinedcontext_portal::openapi::document_text())
        .expect("the OpenAPI document is JSON")
}

#[test]
fn the_tool_list_and_the_openapi_document_say_the_same_of_every_operation() {
    let doc = document();
    let mut differ = Vec::new();
    for op in ops::registry() {
        let tool = tool_definition(&ops::summary(op));
        let entry = &doc["paths"][format!("{OPS_PREFIX}{}", op.name)]["post"];
        let mut input = tool["inputSchema"].clone();
        // MCP carries the project in the arguments, REST in the path: the rest is one schema.
        input["properties"]
            .as_object_mut()
            .expect("properties")
            .remove("project");
        input["required"]
            .as_array_mut()
            .expect("required")
            .retain(|name| name != "project");
        // MCP drops the schema's own description: the tool's description says it (T-6058).
        let mut documented = entry["requestBody"]["content"]["application/json"]["schema"].clone();
        if let Some(schema) = documented.as_object_mut() {
            schema.remove("description");
        }
        // MCP lists `required` even when empty; JSON Schema reads an empty list as none.
        if input["required"] == json!([]) && documented.get("required").is_none() {
            input.as_object_mut().expect("object").remove("required");
        }
        let pairs = [
            ("name", tool["name"].clone(), entry["operationId"].clone()),
            ("title", tool["title"].clone(), entry["summary"].clone()),
            (
                "description",
                tool["description"].clone(),
                entry["description"].clone(),
            ),
            ("input", input, documented),
            (
                "output",
                tool["outputSchema"].clone(),
                entry["responses"]["200"]["content"]["application/json"]["schema"].clone(),
            ),
            (
                "requires",
                tool["_meta"]["joinedcontext.com/requires"].clone(),
                entry["x-jc-requires"].clone(),
            ),
        ];
        for (what, listed, documented) in pairs {
            if listed != documented || listed.is_null() {
                differ.push(format!("{} {what}: {listed} vs {documented}", op.name));
            }
        }
    }
    assert!(
        differ.is_empty(),
        "the two documents differ:\n{}",
        differ.join("\n")
    );

    let documented = doc["paths"]
        .as_object()
        .expect("paths")
        .keys()
        .filter(|path| path.starts_with(OPS_PREFIX) && !path.ends_with("{name}"))
        .count();
    assert_eq!(
        documented,
        ops::registry().len(),
        "one OpenAPI entry per operation and none for an operation that is gone"
    );
}

#[test]
fn every_tool_carries_an_object_input_schema_and_a_rule() {
    for op in ops::registry() {
        let tool = tool_definition(&ops::summary(op));
        assert_eq!(tool["inputSchema"]["type"], "object", "{}", op.name);
        assert!(tool["inputSchema"]["properties"].is_object(), "{}", op.name);
        let rule = &tool["_meta"]["joinedcontext.com/requires"];
        assert!(
            rule["verb"].as_str().is_some_and(|verb| !verb.is_empty()),
            "{} names no verb: {rule}",
            op.name
        );
        assert!(
            ["registry", "route", "organization", "project"]
                .contains(&rule["checkedBy"].as_str().unwrap_or_default()),
            "{}: {rule}",
            op.name
        );
    }
}

/// Every operation answers on the registry's door (`ops_registry_tests` calls each one there), so
/// the door's authorization row covers each of them, and the MCP door's row the tool calls.
#[test]
fn every_operation_has_a_route_with_an_authz_row() {
    let matrix = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/authz_matrix.yaml"
    ))
    .expect("the matrix is committed");
    for route in ["POST /projects/{project}/ops/{name}", "POST /mcp"] {
        assert!(
            matrix.contains(&format!("- route: {route}\n")),
            "no authz row for {route}"
        );
    }
}
