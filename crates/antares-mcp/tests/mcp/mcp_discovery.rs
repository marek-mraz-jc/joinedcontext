// SPDX-License-Identifier: EUPL-1.2
//! Discovery and the tool list (MCP D-3, D-4; MC1, MC2).

use crate::common::{call, fixture, fixture_with, rpc, tool_names};
use crate::engines::NoTypes;
use serde_json::{json, Value};
use std::sync::Arc;

/// MC2: every name an input schema offers is one the view shows.
#[tokio::test(flavor = "multi_thread")]
async fn input_schemas_list_only_visible_names() -> Result<(), String> {
    let f = fixture().await?;
    let list = rpc(&f.app, "cars", "tools/list", json!({})).await;
    let text = list.result().to_string();
    for hidden in ["Bike", "secret"] {
        assert!(!text.contains(hidden), "{hidden} in {text}");
    }
    let query = list
        .result()
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|t| t.get("name") == Some(&json!("query_entities")))
        .cloned()
        .unwrap_or_default();
    let schema = query.get("inputSchema").cloned().unwrap_or_default();
    assert_eq!(
        schema.pointer("/properties/type/items/enum"),
        Some(&json!(["Car"]))
    );
    assert_eq!(
        schema.pointer("/properties/attrs/items/enum"),
        Some(&json!(["colour", "note", "speed"]))
    );
    assert_eq!(schema.get("additionalProperties"), Some(&json!(false)));
    assert!(
        query.pointer("/annotations/readOnlyHint") == Some(&json!(true)),
        "{query}"
    );
    Ok(())
}

/// The list is the view's allowlist filtered by the engine (D-3).
#[tokio::test(flavor = "multi_thread")]
async fn the_tool_list_is_the_allowlist_filtered_by_the_engine() -> Result<(), String> {
    let f = fixture().await?;
    // `summarize` (T-4680) and `area_overview` (T-5279) read through the same
    // operation, so the same view offers them.
    assert_eq!(
        tool_names(&f.app, "few").await,
        vec!["area_overview", "query_entities", "summarize"]
    );
    let all = tool_names(&f.app, "cars").await;
    assert_eq!(
        all.len(),
        16,
        "every read tool but the Subscription ones, and describe_type: {all:?}"
    );
    let f = fixture_with(Some(Arc::new(NoTypes))).await?;
    let less = tool_names(&f.app, "cars").await;
    for gone in ["list_types", "list_type_details"] {
        assert!(
            !less.contains(&gone.to_owned()),
            "clause 5.7.5 serves both: {less:?}"
        );
    }
    assert_eq!(less.len(), 14, "{less:?}");
    Ok(())
}

/// MC1: a tool, resource or name the view or the engine hides answers
/// with the same bytes as one that does not exist.
#[tokio::test(flavor = "multi_thread")]
async fn a_hidden_and_an_absent_name_answer_the_same_bytes_on_every_method() -> Result<(), String> {
    let f = fixture().await?;
    let absent = call(&f.app, "few", "no_such_tool", json!({})).await;
    assert_eq!(
        absent.error().get("code"),
        Some(&json!(-32602)),
        "{}",
        absent.text
    );
    let hidden = call(
        &f.app,
        "few",
        "get_entity",
        json!({"id": "urn:ngsi-ld:Car:1"}),
    )
    .await;
    assert_eq!((hidden.status, &hidden.text), (absent.status, &absent.text));

    let f2 = fixture_with(Some(Arc::new(NoTypes))).await?;
    let engine_hidden = call(&f2.app, "cars", "list_types", json!({})).await;
    let absent2 = call(&f2.app, "cars", "no_such_tool", json!({})).await;
    assert_eq!(
        (engine_hidden.status, &engine_hidden.text),
        (absent2.status, &absent2.text)
    );

    let bike = call(&f.app, "cars", "get_type", json!({"type": "Bike"})).await;
    let nothing = call(&f.app, "cars", "get_type", json!({"type": "Nothing"})).await;
    assert_eq!(bike.text, nothing.text);
    assert!(
        bike.is_error() && !bike.text.contains("Bike"),
        "{}",
        bike.text
    );

    let read = |uri: &'static str| {
        let app = f.app.clone();
        async move { rpc(&app, "cars", "resources/read", json!({"uri": uri})).await }
    };
    let out_of_view = read("ngsi-ld://cars/entities/urn:ngsi-ld:Car:3").await;
    let other_type = read("ngsi-ld://cars/entities/urn:ngsi-ld:Bike:b").await;
    let missing = read("ngsi-ld://cars/entities/urn:ngsi-ld:Car:404").await;
    let hidden_type = read("ngsi-ld://cars/types/Bike").await;
    assert_eq!(
        missing.error().get("code"),
        Some(&json!(-32602)),
        "{}",
        missing.text
    );
    for a in [&out_of_view, &other_type, &hidden_type] {
        assert_eq!((a.status, &a.text), (missing.status, &missing.text));
    }
    Ok(())
}
