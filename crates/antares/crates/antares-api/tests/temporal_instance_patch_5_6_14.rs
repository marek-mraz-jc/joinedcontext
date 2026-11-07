// SPDX-License-Identifier: EUPL-1.2
//! 5.6.14.4: "Replace the target Attribute instance identified by the
//! instanceId with the Attribute instance in the EntityTemporal Fragment."
//! The Fragment names the Attribute (Coraine troe_timescale_temporal_instance);
//! a bare instance (ETSI 016_01) is that instance itself.

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn send(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(b) => {
            let s = b.to_string();
            req = req
                .header("Content-Type", "application/json")
                .header("Content-Length", s.len());
            Body::from(s)
        }
        None => Body::empty(),
    };
    let res = antares_api::router(st.clone())
        .oneshot(req.body(body).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The instance observed at `at`, as the history shows it.
async fn instance(st: &AppState, at: &str) -> Value {
    let (status, body) = send(st, "GET", "/ngsi-ld/v1/temporal/entities/urn:V1", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["speed"]
        .as_array()
        .and_then(|a| a.iter().find(|i| i["observedAt"] == at))
        .cloned()
        .unwrap_or_else(|| panic!("no instance at {at}: {body}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_instance_is_replaced_from_a_named_or_a_bare_fragment() {
    let mut st = AppState::new("temporal-instance-patch".into());
    antares_api::wire(&mut st).await;
    let (h9, h10) = ("2026-04-28T09:00:00Z", "2026-04-28T10:00:00Z");
    let created = json!({"id": "urn:V1", "type": "Vehicle", "speed": [
        {"type": "Property", "value": 10, "observedAt": h9},
        {"type": "Property", "value": 20, "observedAt": h10}]});
    let (status, err) = send(&st, "POST", "/ngsi-ld/v1/temporal/entities", Some(created)).await;
    assert_eq!(status, StatusCode::CREATED, "{err}");
    let iid = instance(&st, h10).await["instanceId"]
        .as_str()
        .expect("id")
        .to_owned();
    let uri = format!("/ngsi-ld/v1/temporal/entities/urn:V1/attrs/speed/{iid}");

    let named = json!({"speed": [{"type": "Property", "value": 25, "observedAt": h10}]});
    let (status, err) = send(&st, "PATCH", &uri, Some(named)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{err}");
    assert_eq!(instance(&st, h10).await["value"], 25);

    let bare = json!({"type": "Property", "value": 26, "observedAt": h10});
    let (status, err) = send(&st, "PATCH", &uri, Some(bare)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{err}");
    assert_eq!(instance(&st, h10).await["value"], 26);

    let two = json!({"speed": [{"type": "Property", "value": 1, "observedAt": h10},
        {"type": "Property", "value": 2, "observedAt": h10}]});
    let (status, err) = send(&st, "PATCH", &uri, Some(two)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(instance(&st, h9).await["value"], 10);
}
