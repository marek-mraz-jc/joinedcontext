// SPDX-License-Identifier: EUPL-1.2
//! A write through a view keeps to the view's write guard on the local half
//! of a federated write too: with an inclusive registration covering the
//! Entity, Merge and Replace Entity through the view neither set an
//! Attribute the view does not let them write nor move the Entity out of
//! the view, and a failed `If-Match` is the 412 the local-only path answers.
#![cfg(feature = "views")]

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::Service;

const TENANT: &str = "fedguard";
const VIEW: &str = "/x/views/zone-a";
const ID: &str = "urn:ngsi-ld:Sensor:fg1";

async fn broker() -> Router {
    let mut state = AppState::new("fed-guard".into());
    antares_api::wire(&mut state).await;
    let surfaces: Vec<Box<dyn antares_api::ApiSurface>> = vec![
        Box::new(antares_api::Admin),
        antares_ops::mount(antares_views::Views),
    ];
    let state = state.with_surfaces(surfaces).expect("the surfaces mount");
    // the reload handle lives as long as the table it keeps
    std::mem::forget(
        antares_ops::views::install(&state)
            .await
            .expect("the view table"),
    );
    let app = antares_api::router(state);
    let ops: Vec<&str> = antares_ops::OperationId::ALL
        .iter()
        .map(|op| op.id())
        .collect();
    let view = json!({"id": "zone-a", "tenant": TENANT, "selector": ["Sensor"],
        "rules": [{"types": ["Sensor"], "q": "zone==\"A\"", "attributes": ["zone", "name"]}],
        "operations": ops});
    let (s, b) = send(&app, "PUT", "/q/views/zone-a", Some(&view), None).await;
    assert!(s.is_success(), "view: {s} {b}");
    let sensor = json!({"id": ID, "type": "Sensor",
        "zone": {"type": "Property", "value": "A"},
        "name": {"type": "Property", "value": "n"},
        "secret": {"type": "Property", "value": "kept"}});
    let (s, b) = send(&app, "POST", "/ngsi-ld/v1/entities", Some(&sensor), None).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    // an inclusive Context Source that never answers: only the local half matters here
    let registration = json!({"id": "urn:ngsi-ld:ContextSourceRegistration:fg",
        "type": "ContextSourceRegistration", "endpoint": "http://127.0.0.1:1",
        "mode": "inclusive", "information": [{"entities": [{"type": "Sensor"}]}]});
    let (s, b) = send(
        &app,
        "POST",
        "/ngsi-ld/v1/csourceRegistrations",
        Some(&registration),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    app
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<&Value>,
    if_match: Option<&str>,
) -> (StatusCode, Value) {
    let text = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Host", "localhost")
        .header("NGSILD-Tenant", TENANT);
    if body.is_some() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", text.len());
    }
    if let Some(tag) = if_match {
        req = req.header("If-Match", tag);
    }
    let Ok(res) = app
        .clone()
        .call(req.body(Body::from(text)).expect("request"))
        .await;
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn stored(app: &Router) -> Value {
    let (s, b) = send(
        app,
        "GET",
        &format!("/ngsi-ld/v1/entities/{ID}"),
        None,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    b
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inclusive_registration_does_not_lift_the_views_write_guard() {
    let app = broker().await;
    let through = format!("{VIEW}/ngsi-ld/v1/entities/{ID}");
    let writes = [
        (
            "PATCH",
            json!({"secret": {"type": "Property", "value": "leaked"}}),
        ),
        ("PATCH", json!({"zone": {"type": "Property", "value": "B"}})),
        (
            "PUT",
            json!({"id": ID, "type": "Sensor", "zone": {"type": "Property", "value": "A"},
                   "secret": {"type": "Property", "value": "leaked"}}),
        ),
        (
            "PUT",
            json!({"id": ID, "type": "Sensor", "zone": {"type": "Property", "value": "B"},
                   "name": {"type": "Property", "value": "n"}}),
        ),
    ];
    for (method, body) in writes {
        let (s, b) = send(&app, method, &through, Some(&body), None).await;
        assert!(
            s.is_client_error() || s == StatusCode::MULTI_STATUS,
            "{method} {body}: {s} {b}"
        );
        let now = stored(&app).await;
        assert_eq!(
            now["secret"]["value"],
            json!("kept"),
            "{method} {body}: {s} {b}"
        );
        assert_eq!(now["zone"]["value"], json!("A"), "{method} {body}: {s} {b}");
    }
    let (s, b) = send(
        &app,
        "PATCH",
        &through,
        Some(&json!({"name": {"type": "Property", "value": "m"}})),
        Some("\"0000000000000000\""),
    )
    .await;
    assert_eq!(s, StatusCode::PRECONDITION_FAILED, "{b}");
    assert_eq!(stored(&app).await["name"]["value"], json!("n"));
}
