// SPDX-License-Identifier: EUPL-1.2
//! `Ops::get_as` and `ViewOps::watched_types` (T-5911): a push channel re-reads a resource
//! through the broker's own router as the caller, with the caller's `Host`, and names the
//! types a Subscription through a view watches.

use antares_ops::test_kit::TestBroker;
use antares_ops::{Caller, Ops, Surface};
use axum::body::Body;
use axum::extract::{Path, Query};
use axum::http::Request;
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use tower::ServiceExt;

/// `/x/reread/get?path=…&max=…` answers `Ops::get_as` of `path`; `/x/reread/{view}/types`
/// `ViewOps::watched_types`.
struct Reread;

impl Surface for Reread {
    fn name(&self) -> &'static str {
        "reread"
    }
    fn prefix(&self) -> &'static str {
        "/x/reread"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn needs_view(&self) -> bool {
        false
    }
    fn router(&self, ops: Ops) -> Router {
        let types = ops.clone();
        Router::new()
            .route(
                "/get",
                get(
                    move |caller: Caller, Query(q): Query<HashMap<String, String>>| async move {
                        let path = q.get("path").cloned().unwrap_or_default();
                        let max = q.get("max").and_then(|m| m.parse().ok()).unwrap_or(1 << 20);
                        let (status, body) = ops.get_as(&caller, &path, max).await;
                        axum::Json(json!({
                            "status": status,
                            "body": String::from_utf8_lossy(&body),
                        }))
                    },
                ),
            )
            .route(
                "/{view}/types",
                get(move |Path(id): Path<String>| async move {
                    match types.view(&id) {
                        Some(v) => axum::Json(json!(v.watched_types().await)),
                        None => axum::Json(Value::Null),
                    }
                }),
            )
    }
}

async fn call(app: &Router, method: &str, uri: &str, host: &str, body: Option<Value>) -> Value {
    let text = body.map(|b| b.to_string()).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("NGSILD-Tenant", "city");
    if !host.is_empty() {
        req = req.header("Host", host);
    }
    if !text.is_empty() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", text.len());
    }
    let req = req.body(Body::from(text)).unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert!(status.is_success(), "{method} {uri}: {status} {body}");
    body
}

#[tokio::test]
async fn a_reread_answers_as_http_with_the_callers_host() -> Result<(), String> {
    let broker = TestBroker::new().await?;
    let app = broker.serve(Reread)?;
    let car = json!({"id": "urn:ngsi-ld:Car:1", "type": "Car", "speed": {"type": "Property", "value": 1}});
    call(&app, "POST", "/ngsi-ld/v1/entities", "h", Some(car)).await;
    let read = call(
        &app,
        "GET",
        "/x/reread/get?path=/ngsi-ld/v1/entities/urn:ngsi-ld:Car:1",
        "h",
        None,
    )
    .await;
    // the read runs in the caller's Tenant, as the request it re-reads for
    assert_eq!(read["status"], json!(200), "{read}");
    assert!(
        read["body"]
            .as_str()
            .is_some_and(|b| b.contains("urn:ngsi-ld:Car:1")),
        "{read}"
    );
    let health = call(
        &app,
        "GET",
        "/x/reread/get?path=/q/health",
        "example.org:8080",
        None,
    )
    .await;
    assert_eq!(health["status"], json!(200), "{health}");
    // past `max` the body is empty, and a path no request can carry answers 400
    let small = call(&app, "GET", "/x/reread/get?path=/q/health&max=1", "", None).await;
    assert_eq!(small, json!({"status": 200, "body": ""}));
    let bad = call(&app, "GET", "/x/reread/get?path=%20bad%20path", "h", None).await;
    assert_eq!(bad, json!({"status": 400, "body": ""}));
    Ok(())
}

#[tokio::test]
async fn a_view_watches_its_shown_types_or_the_stored_ones() -> Result<(), String> {
    let broker = TestBroker::new().await?;
    let app = broker.serve(Reread)?;
    let car = json!({"id": "urn:ngsi-ld:Car:1", "type": "Car", "speed": {"type": "Property", "value": 1}});
    call(&app, "POST", "/ngsi-ld/v1/entities", "h", Some(car)).await;
    for (id, selector, rules) in [
        (
            "named",
            json!(["Car", "Stop"]),
            json!([{"types": ["Car", "Stop"], "attributes": ["speed"]}]),
        ),
        (
            "every",
            json!(["*"]),
            json!([{"types": ["*"], "attributes": ["*"]}]),
        ),
    ] {
        let view = json!({"id": id, "tenant": "city", "selector": selector, "rules": rules,
                          "operations": ["query_entities"]});
        call(&app, "PUT", &format!("/q/views/{id}"), "h", Some(view)).await;
    }
    let named = call(&app, "GET", "/x/reread/named/types", "h", None).await;
    assert_eq!(named, json!(["Car", "Stop"]));
    let every = call(&app, "GET", "/x/reread/every/types", "h", None).await;
    let every: Vec<&str> = every
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    assert!(every.iter().any(|t| t.ends_with("Car")), "{every:?}");
    let none = call(&app, "GET", "/x/reread/gone/types", "h", None).await;
    assert_eq!(none, Value::Null);
    Ok(())
}
