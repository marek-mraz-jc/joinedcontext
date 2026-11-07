// SPDX-License-Identifier: EUPL-1.2
//! Every tool is one gated operation (MCP rules 2, 4 and 13; MC3, MC15,
//! MC16). MC19, the log line, is `tests/mcp_log.rs`: a binary of its own,
//! since tracing's callsite cache is per process.

use crate::common::{call, fixture, fixture_with, rpc, tool_names};
use crate::engines::{Counting, NoNamedTypes};
use serde_json::{json, Value};
use std::sync::Arc;

/// Arguments that make `tool` a valid call on view `cars`.
fn minimal(tool: &str) -> Value {
    let id = "urn:ngsi-ld:Car:1";
    let time = json!({"id": id, "timerel": "before", "timeAt": "2999-01-01T00:00:00Z"});
    match tool {
        "batch_query_temporal" | "query_temporal" => {
            json!({"type": ["Car"], "timerel": "before", "timeAt": "2999-01-01T00:00:00Z"})
        }
        "get_attribute" => json!({"attrId": "speed"}),
        "get_attribute_value" => json!({"id": id, "attrId": "speed"}),
        "get_entity" => json!({"id": id}),
        "describe_type" | "get_type" => json!({"type": "Car"}),
        "batch_query" | "query_entities" | "summarize" => json!({"type": ["Car"]}),
        // one type read through the gate (T-5279)
        "area_overview" => {
            json!({"type": ["Car"], "geometry": "Point", "coordinates": [0.0, 0.0]})
        }
        "retrieve_temporal" => time,
        _ => json!({}),
    }
}

/// MC3: a call is the operation's own gate, asked once, and the one probe
/// that keeps a hidden tool unknown (D-3); a resource read is the same.
/// Decided (worker-4, owner may override): "exactly once" in MC3 is the
/// gate of the operation; the probe is the second decision, an Operation
/// that names nothing, because without it a tool the engine hides would
/// answer 403 where an unknown one answers -32602 (MC1).
#[tokio::test(flavor = "multi_thread")]
async fn every_tool_and_resource_asks_the_policy_engine_exactly_once() -> Result<(), String> {
    let engine = Arc::new(Counting::default());
    let f = fixture_with(Some(engine.clone())).await?;
    let tools = tool_names(&f.app, "cars").await;
    assert_eq!(tools.len(), 16, "{tools:?}");
    for tool in &tools {
        let before = engine.count();
        let a = call(&f.app, "cars", tool, minimal(tool)).await;
        assert!(!a.is_error(), "{tool}: {}", a.text);
        assert_eq!(engine.count() - before, 2, "{tool}: one probe, one gate");
    }
    let before = engine.count();
    let a = rpc(
        &f.app,
        "cars",
        "resources/read",
        json!({"uri": "ngsi-ld://cars/entities/urn:ngsi-ld:Car:1"}),
    )
    .await;
    assert!(a.result().get("contents").is_some(), "{}", a.text);
    assert_eq!(
        engine.count() - before,
        2,
        "resources/read: one probe, one gate"
    );
    Ok(())
}

/// MC3: an operation the gate denies is a tool error, never an empty list.
#[tokio::test(flavor = "multi_thread")]
async fn a_denied_operation_is_a_tool_error_and_never_an_empty_result() -> Result<(), String> {
    let f = fixture_with(Some(Arc::new(NoNamedTypes))).await?;
    assert!(tool_names(&f.app, "cars")
        .await
        .contains(&"query_entities".to_owned()));
    let a = call(&f.app, "cars", "query_entities", json!({"type": ["Car"]})).await;
    assert!(a.is_error(), "{}", a.text);
    assert!(a.text.contains("403"), "{}", a.text);
    assert!(a.structured().get("entities").is_none(), "{}", a.text);
    assert!(
        !a.text.contains("speed<80"),
        "the engine's reason stays inside: {}",
        a.text
    );
    Ok(())
}

/// MC15: no argument names a tenant, a space or a subject, and trying one
/// reaches no other tenant.
#[tokio::test(flavor = "multi_thread")]
async fn a_tenant_or_subject_in_the_arguments_is_refused_and_changes_nothing() -> Result<(), String>
{
    let f = fixture().await?;
    for extra in ["tenant", "NGSILD-Tenant", "space", "subject", "headers"] {
        let mut args = json!({"type": ["Car"]});
        if let Some(o) = args.as_object_mut() {
            o.insert(extra.into(), json!("other"));
        }
        let a = call(&f.app, "cars", "query_entities", args).await;
        assert!(a.is_error(), "{extra}: {}", a.text);
        assert!(a.text.contains("not an argument"), "{extra}: {}", a.text);
        assert!(!a.text.contains("Car:9"), "{extra}: {}", a.text);
    }
    let a = rpc(
        &f.app,
        "cars",
        "tools/call",
        json!({"name": "query_entities", "arguments": {"type": ["Car"]}}),
    )
    .await;
    let ids: Vec<&str> = a
        .structured()
        .get("entities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| e.get("id").and_then(Value::as_str))
        .collect();
    assert_eq!(ids, vec!["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:2"]);
    let header = crate::common::rpc_with(
        &f.app,
        "cars",
        "tools/call",
        json!({"name": "query_entities", "arguments": {"type": ["Car"]}}),
        &[("NGSILD-Tenant", "other")],
    )
    .await;
    assert!(!header.text.contains("Car:9"), "{}", header.text);
    Ok(())
}

/// MC16, renamed from `join_is_not_an_argument_until_the_target_is_checked`:
/// T-3470 checks each join target against the view, so `join` is an
/// argument now, and it never reaches an Entity out of view.
#[tokio::test(flavor = "multi_thread")]
async fn join_is_an_argument_now_that_the_target_is_checked() -> Result<(), String> {
    let f = fixture().await?;
    let to_in_view = call(
        &f.app,
        "cars",
        "get_entity",
        json!({"id": "urn:ngsi-ld:Car:2", "join": "inline"}),
    )
    .await;
    assert!(!to_in_view.is_error(), "{}", to_in_view.text);
    assert!(
        to_in_view.structured().pointer("/entity/note/entity/id")
            == Some(&json!("urn:ngsi-ld:Car:1")),
        "{}",
        to_in_view.text
    );
    let to_out_of_view = call(
        &f.app,
        "cars",
        "get_entity",
        json!({"id": "urn:ngsi-ld:Car:1", "join": "inline"}),
    )
    .await;
    assert!(!to_out_of_view.is_error(), "{}", to_out_of_view.text);
    assert!(
        to_out_of_view
            .structured()
            .pointer("/entity/note/entity")
            .is_none(),
        "{}",
        to_out_of_view.text
    );
    assert!(
        !to_out_of_view.text.contains("\"value\":90"),
        "{}",
        to_out_of_view.text
    );
    Ok(())
}
