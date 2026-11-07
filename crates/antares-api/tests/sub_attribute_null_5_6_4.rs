// SPDX-License-Identifier: EUPL-1.2
//! TS 104 175 10.2.5.4 (Partial Attribute update) and 10.2.9.4 (Merge Entity):
//! "NGSI-LD Nulls should be supported by this operation", and a member of the
//! fragment whose value is an NGSI-LD Null removes that member from the
//! target (5.5.12). Found by the forge suite in T-3677 (012_10_01, 012_10_03,
//! 056_07_02): `observedAt: "urn:ngsi-ld:null"` was refused as an invalid
//! DateTime, and a sub-Relationship whose `object` is the Null was stored as a
//! Relationship pointing at `urn:ngsi-ld:null` instead of being removed.

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const ID: &str = "urn:ngsi-ld:Building:n1";

async fn call(st: &AppState, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let req = Request::builder()
        .method(method)
        .uri(format!("/ngsi-ld/v1/{path}"))
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .expect("request");
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// A Building whose `airQualityLevel` carries every member the cases remove.
async fn building() -> AppState {
    let st = AppState::new("test".into());
    let (status, body) = call(
        &st,
        "POST",
        "entities",
        Some(json!({
            "id": ID,
            "type": "Building",
            "airQualityLevel": {
                "type": "Property",
                "value": 3,
                "observedAt": "2026-10-13T10:00:00Z",
                "unitCode": "C62",
                "hasWeatherObserved": {
                    "type": "Relationship",
                    "object": "urn:ngsi-ld:WeatherObserved:w1"
                }
            }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    st
}

async fn level(st: &AppState) -> Value {
    let (status, body) = call(st, "GET", &format!("entities/{ID}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["airQualityLevel"].clone()
}

#[tokio::test]
async fn a_partial_update_removes_observed_at_set_to_the_null() {
    let st = building().await;
    let (status, body) = call(
        &st,
        "PATCH",
        &format!("entities/{ID}/attrs/airQualityLevel"),
        Some(json!({"type": "Property", "value": 4, "observedAt": "urn:ngsi-ld:null"})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let level = level(&st).await;
    assert_eq!(level["value"], 4, "{level}");
    assert!(level.get("observedAt").is_none(), "{level}");
    assert_eq!(
        level["unitCode"], "C62",
        "only the nulled member leaves: {level}"
    );
}

#[tokio::test]
async fn a_partial_update_removes_a_sub_relationship_whose_object_is_the_null() {
    let st = building().await;
    let (status, body) = call(
        &st,
        "PATCH",
        &format!("entities/{ID}/attrs/airQualityLevel"),
        Some(json!({"type": "Property", "value": 4,
            "hasWeatherObserved": {"type": "Relationship", "object": "urn:ngsi-ld:null"}})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let level = level(&st).await;
    assert!(level.get("hasWeatherObserved").is_none(), "{level}");
    assert_eq!(level["observedAt"], "2026-10-13T10:00:00Z", "{level}");
}

#[tokio::test]
async fn a_merge_removes_a_sub_relationship_and_observed_at_set_to_the_null() {
    let st = building().await;
    let (status, body) = call(
        &st,
        "PATCH",
        &format!("entities/{ID}"),
        Some(json!({"airQualityLevel": {"type": "Property", "value": 4,
            "observedAt": "urn:ngsi-ld:null",
            "hasWeatherObserved": {"type": "Relationship", "object": "urn:ngsi-ld:null"}}})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let level = level(&st).await;
    assert_eq!(level["value"], 4, "{level}");
    assert!(level.get("hasWeatherObserved").is_none(), "{level}");
    assert!(level.get("observedAt").is_none(), "{level}");
    assert_eq!(level["unitCode"], "C62", "{level}");
}

#[tokio::test]
async fn an_observed_at_that_is_neither_a_datetime_nor_the_null_is_still_refused() {
    let st = building().await;
    for bad in [json!("yesterday"), json!(7), json!("urn:ngsi-ld:nul")] {
        let (status, body) = call(
            &st,
            "PATCH",
            &format!("entities/{ID}/attrs/airQualityLevel"),
            Some(json!({"type": "Property", "value": 4, "observedAt": bad})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    assert_eq!(level(&st).await["observedAt"], "2026-10-13T10:00:00Z");
}
