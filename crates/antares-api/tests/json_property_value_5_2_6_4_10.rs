// SPDX-License-Identifier: EUPL-1.2
//! JsonProperty json (TS 104 175 5.2.6.4.10, T-5580): Table 5.2.6.4.10-1
//! types json "See NGSI-LD Value definition in clause 3.1", which is a "JSON
//! value (i.e. a string, a number, true or false, an object, an array)".
//! Every kind is created and read back verbatim; JSON null and the NGSI-LD
//! Null (8.2.3) stay BadRequestData in a create.

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn send(st: &AppState, method: &str, uri: &str, body: Option<&str>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(p) => b
            .header("Content-Type", "application/json")
            .header("Content-Length", p.len())
            .body(Body::from(p.to_owned()))
            .expect("request"),
        None => b.body(Body::empty()).expect("request"),
    };
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

/// Coraine array_reduction_entity_data step 01 sends json ["a"].
#[tokio::test(flavor = "multi_thread")]
async fn every_json_value_kind_is_created_and_read_back_verbatim() {
    let st = AppState::new("me".into());
    let kinds = [
        json!(["a"]),
        json!("x"),
        json!(1),
        json!(true),
        json!([1, {"a": "b"}]),
        json!({"@context": "kept", "id": "raw"}),
    ];
    for (n, j) in kinds.iter().enumerate() {
        let id = format!("urn:ngsi-ld:T:json-{n}");
        let e = json!({"id": id, "type": "T", "raw": {"type": "JsonProperty", "json": j}});
        let (code, body) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(&e.to_string())).await;
        assert_eq!(code, StatusCode::CREATED, "json {j}: {body}");
        let (code, body) = send(&st, "GET", &format!("/ngsi-ld/v1/entities/{id}"), None).await;
        assert_eq!(code, StatusCode::OK, "{body}");
        assert_eq!(body["raw"]["json"], *j, "json {j} read back: {body}");
        let uri = format!("/ngsi-ld/v1/entities/{id}?format=keyValues");
        let (code, body) = send(&st, "GET", &uri, None).await;
        assert_eq!(code, StatusCode::OK, "{body}");
        assert_eq!(
            body["raw"],
            json!({"json": j}),
            "keyValues of json {j}: {body}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn null_json_is_refused_in_a_create() {
    let st = AppState::new("me".into());
    for j in [Value::Null, json!("urn:ngsi-ld:null")] {
        let e = json!({"id": "urn:ngsi-ld:T:json-null", "type": "T",
            "raw": {"type": "JsonProperty", "json": j}});
        let (code, body) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(&e.to_string())).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "json {j}: {body}");
        assert!(
            body["type"]
                .as_str()
                .unwrap_or_default()
                .ends_with("BadRequestData"),
            "{body}"
        );
    }
}
