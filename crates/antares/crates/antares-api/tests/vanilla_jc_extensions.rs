// SPDX-License-Identifier: EUPL-1.2
//! The platform's NGSI-LD extensions are a build choice (T-4854): vanilla
//! Antares reads no `JC-*` header and refuses `versions=all` as an unknown
//! query parameter; the `jc` build keeps `versions` and refuses it to anyone
//! the gateway did not grant `readVersions` (the granted path, with the
//! provenance recorded, is tests/temporal_versions_2_4.rs on Postgres).

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const GATEWAY: &str = "context-gateway.dev.serviceaccount.identity.linkerd.cluster.local";
const ID: &str = "urn:ngsi-ld:Sensor:t4854";

/// A broker that would believe `GATEWAY`, if the build reads its headers.
async fn state() -> AppState {
    let mut st = AppState::new("vanilla".into());
    antares_api::wire(&mut st).await;
    st.provenance_from = Some(GATEWAY.into());
    st
}

async fn send(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("l5d-client-id", GATEWAY)
        .header("JC-Provenance-Channel", "pipeline")
        .header("JC-Provenance-Actor", "pipeline-parking")
        .header("JC-Reason", "forged")
        .header("JC-Read-Versions", "true");
    let body = match body {
        Some(v) => {
            let s = v.to_string();
            b = b
                .header("Content-Type", "application/json")
                .header("Content-Length", s.len());
            Body::from(s)
        }
        None => Body::empty(),
    };
    let res = antares_api::router(st.clone())
        .oneshot(b.body(body).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let bytes = http_body_util::BodyExt::collect(res.into_body())
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn with_entity() -> AppState {
    let st = state().await;
    let (status, _) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(json!({"id": ID, "type": "Sensor",
                    "t": {"type": "Property", "value": 1, "observedAt": "2026-10-20T10:00:00Z"}})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    st
}

#[cfg(not(feature = "jc"))]
#[tokio::test]
async fn vanilla_ignores_jc_headers_and_refuses_versions() {
    let st = with_entity().await;
    let plain = format!("/ngsi-ld/v1/temporal/entities/{ID}");
    let (status, body) = send(&st, "GET", &plain, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(&st, "GET", &format!("{plain}?versions=all"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("versions"), "{body}");
}

#[cfg(feature = "jc")]
#[tokio::test]
async fn the_jc_build_keeps_versions_behind_the_gateways_grant() {
    let st = with_entity().await;
    let uri = format!("/ngsi-ld/v1/temporal/entities/{ID}?versions=sometimes");
    let (status, body) = send(&st, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "only `all`: {body}");
    // a broker that names no gateway grants nobody, whatever the headers say
    let mut st = st;
    st.provenance_from = None;
    let uri = format!("/ngsi-ld/v1/temporal/entities/{ID}?versions=all");
    let (status, body) = send(&st, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}
