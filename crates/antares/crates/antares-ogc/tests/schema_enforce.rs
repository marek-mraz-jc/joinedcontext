// SPDX-License-Identifier: EUPL-1.2
//! Part 4 writes under `ANTARES_SCHEMA_WRITES=enforce`: the data model's refusal of an
//! attribute the collection shows is answered as that refusal (its detail), so the client
//! learns which property to fix. Its own binary: the mode is read from the process
//! environment when the views loader starts, and it must not reach the other OGC tests.

use antares_ops::test_kit::TestBroker;
use antares_ops::views::{SCHEMA_MAX_ENV, SCHEMA_WRITES_ENV};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

const SCHEMA: &str = r#"{
  "id": "https://example.org/fleet", "name": "fleet",
  "prefixes": {"dc": {"prefix_prefix": "dc", "prefix_reference": "https://uri.etsi.org/ngsi-ld/default-context/"},
               "ngsi": {"prefix_prefix": "ngsi", "prefix_reference": "https://uri.etsi.org/ngsi-ld/"}},
  "default_prefix": "dc",
  "classes": {"Car": {"slots": ["speed", "colour", "location"]}},
  "slots": {
    "speed": {"range": "float"},
    "colour": {"range": "Colour"},
    "location": {"slot_uri": "ngsi:location", "range": "string",
                 "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}}
  },
  "enums": {"Colour": {"permissible_values": {"red": {}, "blue": {}}}}
}"#;

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
    media: &str,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Type", media)
        .header("Content-Length", body.len())
        .body(Body::from(body.to_owned()))
        .unwrap_or_default();
    let Ok(answer) = app.clone().oneshot(req).await;
    let status = answer.status();
    let bytes = axum::body::to_bytes(answer.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_refusal_of_a_shown_property_is_answered_with_its_detail() -> Result<(), String> {
    std::env::set_var(SCHEMA_MAX_ENV, "65536");
    std::env::set_var(SCHEMA_WRITES_ENV, "enforce");
    let broker = TestBroker::new().await?;
    let app = broker.serve(antares_ogc::Ogc)?;
    let view = json!({
        "id": "fleet", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed", "colour", "location"]}],
        "operations": ["query_entities", "create_entity"]
    });
    let (s, body) = send(
        &app,
        "PUT",
        "/q/views/fleet",
        &view.to_string(),
        "application/json",
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let (s, body) = send(&app, "PUT", "/q/schemas/city", SCHEMA, "application/json").await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let items = "/x/views/fleet/ogc/features/collections/Car/items";
    let feature = |colour: &str| {
        json!({"type": "Feature", "geometry": {"type": "Point", "coordinates": [17.0, 48.0]},
               "properties": {"speed": 3.5, "colour": colour}})
        .to_string()
    };
    let (s, body) = send(
        &app,
        "POST",
        items,
        &feature("green"),
        "application/geo+json",
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    let detail = body
        .get("detail")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        detail.contains("colour") && detail.contains(linkml_model::check::MARK),
        "the refusal names the property: {body}"
    );
    // the model admits a permitted value: the same write is created
    let (s, body) = send(&app, "POST", items, &feature("red"), "application/geo+json").await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    Ok(())
}
