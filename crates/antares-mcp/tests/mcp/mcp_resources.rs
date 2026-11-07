// SPDX-License-Identifier: EUPL-1.2
//! Resources (MCP section 4; MC10).

use crate::common::{fixture, rpc};
use serde_json::{json, Value};

/// The view's types are resources, an Entity is a template.
#[tokio::test(flavor = "multi_thread")]
async fn the_views_types_and_entities_are_resources() -> Result<(), String> {
    let f = fixture().await?;
    let list = rpc(&f.app, "cars", "resources/list", json!({})).await;
    let uris: Vec<&str> = list
        .result()
        .get("resources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| r.get("uri").and_then(Value::as_str))
        .collect();
    assert_eq!(uris, vec!["ngsi-ld://cars/types/Car"]);
    let templates = rpc(&f.app, "cars", "resources/templates/list", json!({})).await;
    assert_eq!(
        templates
            .result()
            .pointer("/resourceTemplates/0/uriTemplate"),
        Some(&json!("ngsi-ld://cars/entities/{id}"))
    );
    let car = rpc(
        &f.app,
        "cars",
        "resources/read",
        json!({"uri": "ngsi-ld://cars/entities/urn:ngsi-ld:Car:1"}),
    )
    .await;
    let text = car
        .result()
        .pointer("/contents/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // the JSON sits inside the data fence under the preface (F-ANT1-004)
    let lines: Vec<&str> = text.lines().collect();
    let inner = lines
        .get(2..lines.len().saturating_sub(1))
        .unwrap_or_default();
    let doc: Value = serde_json::from_str(&inner.join("\n")).map_err(|e| e.to_string())?;
    assert_eq!(doc.get("id"), Some(&json!("urn:ngsi-ld:Car:1")));
    assert!(doc.get("secret").is_none(), "{doc}");
    let escaped = rpc(
        &f.app,
        "cars",
        "resources/read",
        json!({"uri": "ngsi-ld://cars/entities/urn%3Angsi-ld%3ACar%3A1"}),
    )
    .await;
    assert_eq!(
        escaped.result().pointer("/contents/0/text"),
        car.result().pointer("/contents/0/text")
    );
    let ty = rpc(
        &f.app,
        "cars",
        "resources/read",
        json!({"uri": "ngsi-ld://cars/types/Car"}),
    )
    .await;
    assert!(
        ty.result().pointer("/contents/0/text").is_some(),
        "{}",
        ty.text
    );
    let few = rpc(&f.app, "few", "resources/list", json!({})).await;
    assert_eq!(
        few.result().get("resources"),
        Some(&json!([])),
        "{}",
        few.text
    );
    let few = rpc(&f.app, "few", "resources/templates/list", json!({})).await;
    assert_eq!(
        few.result().get("resourceTemplates"),
        Some(&json!([])),
        "{}",
        few.text
    );
    let not_served = rpc(
        &f.app,
        "few",
        "resources/read",
        json!({"uri": "ngsi-ld://few/types/Car"}),
    )
    .await;
    let unknown = rpc(
        &f.app,
        "few",
        "resources/read",
        json!({"uri": "ngsi-ld://few/types/Nothing"}),
    )
    .await;
    assert_eq!(
        not_served.error().get("code"),
        Some(&json!(-32602)),
        "{}",
        not_served.text
    );
    assert_eq!(not_served.text, unknown.text);
    Ok(())
}

/// MC10: another view's URI, another tenant's id or a malformed URI answer
/// like an Entity that does not exist.
#[tokio::test(flavor = "multi_thread")]
async fn a_resource_of_another_view_answers_like_an_unknown_one() -> Result<(), String> {
    let f = fixture().await?;
    let read = |uri: &'static str| {
        let app = f.app.clone();
        async move { rpc(&app, "cars", "resources/read", json!({"uri": uri})).await }
    };
    let unknown = read("ngsi-ld://cars/entities/urn:ngsi-ld:Car:404").await;
    assert_eq!(
        unknown.error().get("code"),
        Some(&json!(-32602)),
        "{}",
        unknown.text
    );
    for uri in [
        "ngsi-ld://elsewhere/entities/urn:ngsi-ld:Car:9",
        "ngsi-ld://cars/entities/urn:ngsi-ld:Car:9",
        "ngsi-ld://carsx/entities/urn:ngsi-ld:Car:1",
        "ngsi-ld://cars/entities/urn:ngsi-ld:Car:1%zz",
        "ngsi-ld://cars/other/x",
        "https://cars/entities/urn:ngsi-ld:Car:1",
    ] {
        let a = read(uri).await;
        assert_eq!(
            (a.status, &a.text),
            (unknown.status, &unknown.text),
            "{uri}"
        );
    }
    Ok(())
}
