// SPDX-License-Identifier: EUPL-1.2
//! 4.8 / TS 104 176 6.3.11: createdAt and modifiedAt appear only with
//! options=sysAttrs, and a sub-Attribute's are no exception in the temporal
//! representation (Coraine troe_timescale_routes_single_entity).

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn send(st: &AppState, req: Request<Body>) -> (StatusCode, Value) {
    let res = antares_api::router(st.clone())
        .oneshot(req)
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

#[tokio::test(flavor = "multi_thread")]
async fn a_temporal_sub_attribute_shows_its_timestamps_only_with_sys_attrs() {
    let mut st = AppState::new("temporal-sub-sys".into());
    antares_api::wire(&mut st).await; // temporal auto-recording
    let body = json!({"id": "urn:E:01", "type": "T",
        "p": {"type": "Property", "value": 1, "s": {"type": "Property", "value": "S"}}})
    .to_string();
    let (status, b) = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{b}");

    let read = |options: &'static str| {
        let st = st.clone();
        async move {
            let (status, body) = send(
                &st,
                Request::builder()
                    .uri(format!(
                        "/ngsi-ld/v1/temporal/entities/urn:E:01?timerel=after&timeAt=2000-01-01T00:00:00Z&timeproperty=modifiedAt{options}"
                    ))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            body["p"][0]["s"].clone()
        }
    };
    let plain = read("").await;
    assert_eq!(plain["value"], "S", "{plain}");
    assert!(plain.get("createdAt").is_none(), "{plain}");
    assert!(plain.get("modifiedAt").is_none(), "{plain}");
}
