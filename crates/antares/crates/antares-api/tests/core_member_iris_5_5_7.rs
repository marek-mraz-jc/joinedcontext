// SPDX-License-Identifier: EUPL-1.2
//! 5.5.7: "The term to URI expansion or compaction shall be performed using a
//! @context as described by the JSON-LD specification" (TS 104 175 8.2.4 the
//! same). JSON-LD expansion maps `observedAt` and
//! `https://uri.etsi.org/ngsi-ld/observedAt` to one IRI, so both spellings
//! are one document. The full IRI of a core member was matched by its short
//! key only and stored as a sub-Property (T-5466, Coraine
//! expanded_core_terms_input).

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const CORE: &str = "https://uri.etsi.org/ngsi-ld/";

async fn call(st: &AppState, method: &str, path: &str, body: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(format!("/ngsi-ld/v1/{path}"))
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body.to_owned()))
        .expect("request");
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

async fn round_trip(st: &AppState, n: usize, attr: Value) -> Value {
    let id = format!("urn:ngsi-ld:T:c{n}");
    let body = json!({"id": id, "type": "T", "P": attr}).to_string();
    let (status, resp) = call(st, "POST", "entities", &body).await;
    assert_eq!(status, StatusCode::CREATED, "{attr}: {resp}");
    let (status, doc) = call(st, "GET", &format!("entities/{id}"), "").await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    doc["P"].clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_core_member_spelled_as_its_iri_is_the_member() {
    let st = AppState::new("test".into());
    let iri = |t: &str| format!("{CORE}{t}");
    let cases = [
        (
            json!({"type": "Property", "value": 1, iri("observedAt"): "2026-01-01T00:00:00Z"}),
            json!({"type": "Property", "value": 1, "observedAt": "2026-01-01T00:00:00Z"}),
        ),
        (
            json!({"type": "Property", "value": 1, iri("unitCode"): "CEL"}),
            json!({"type": "Property", "value": 1, "unitCode": "CEL"}),
        ),
        (
            json!({"type": "Property", "value": 1, iri("datasetId"): "urn:ds1"}),
            json!({"type": "Property", "value": 1, "datasetId": "urn:ds1"}),
        ),
        (
            json!({"type": iri("Property"), "value": 1}),
            json!({"type": "Property", "value": 1}),
        ),
        (
            json!({"type": "Property", iri("hasValue"): 42}),
            json!({"type": "Property", "value": 42}),
        ),
        (
            json!({"type": "Property", "value": 1,
                "sub": {"type": "Property", "value": 2, iri("observedAt"): "2026-01-01T00:00:00Z"}}),
            json!({"type": "Property", "value": 1,
                "sub": {"type": "Property", "value": 2, "observedAt": "2026-01-01T00:00:00Z"}}),
        ),
    ];
    for (n, (sent, want)) in cases.into_iter().enumerate() {
        assert_eq!(round_trip(&st, n, sent.clone()).await, want, "sent {sent}");
    }
    let rel = json!({"type": "Relationship", iri("hasObject"): "urn:ngsi-ld:E:1"});
    assert_eq!(
        round_trip(&st, 99, rel).await,
        json!({"type": "Relationship", "object": "urn:ngsi-ld:E:1"})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_given_as_term_and_iri_is_refused() {
    let st = AppState::new("test".into());
    let body = json!({"id": "urn:ngsi-ld:T:dup", "type": "T",
        "P": {"type": "Property", "value": 1, "https://uri.etsi.org/ngsi-ld/hasValue": 2}})
    .to_string();
    let (status, resp) = call(&st, "POST", "entities", &body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{resp}");
}
