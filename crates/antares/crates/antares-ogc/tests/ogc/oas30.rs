// SPDX-License-Identifier: EUPL-1.2
//! Part 1 `OpenAPI` 3.0 (mapping row C2).

use crate::common::{fixture, get};
use axum::http::StatusCode;
use serde_json::Value;

#[tokio::test(flavor = "multi_thread")]
async fn definition_lists_only_shown_collections_and_offered_methods() -> Result<(), String> {
    let f = fixture().await?;
    let a = get(&f.app, "/x/views/city/ogc/features/api").await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(
        a.headers
            .get("content-type")
            .map(axum::http::HeaderValue::as_bytes),
        Some(&b"application/vnd.oai.openapi+json;version=3.0"[..])
    );
    assert_eq!(a.body.get("openapi").and_then(Value::as_str), Some("3.0.3"));
    let paths = a
        .body
        .get("paths")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    assert!(
        paths.contains_key("/collections/Car/items")
            && paths.contains_key("/collections/Stop/items/{featureId}")
    );
    assert!(
        !a.body.to_string().contains("Depot"),
        "a hidden type is not named"
    );
    for (path, item) in &paths {
        let methods: Vec<&String> = item
            .as_object()
            .map(|m| m.keys().collect())
            .unwrap_or_default();
        assert_eq!(methods, ["get"], "{path}: GET only, as served");
    }
    Ok(())
}
