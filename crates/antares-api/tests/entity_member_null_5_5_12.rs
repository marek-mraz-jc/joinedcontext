// SPDX-License-Identifier: EUPL-1.2
//! 5.5.12: a Fragment member whose value is the NGSI-LD Null removes the
//! target member. Update Attributes treats `scope` as merge does; Append is
//! neither an update nor a merge, so 5.5.4 refuses the Null there;
//! `type` is mandatory (4.5.1), so the Null is never a type (Coraine
//! entity_member_delete_and_null steps 10-13).

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
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
    let resp = antares_api::router(st.clone())
        .oneshot(req.body(body).expect("req"))
        .await
        .expect("resp");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn entity(st: &AppState) -> Value {
    let (status, e) = send(st, "GET", "/ngsi-ld/v1/entities/urn:E1", None).await;
    assert_eq!(status, StatusCode::OK, "{e}");
    e
}

#[tokio::test(flavor = "multi_thread")]
async fn the_null_removes_a_scope_on_every_attribute_write_and_is_never_a_type() {
    let mut st = AppState::new("test-entity-member-null".into());
    antares_api::wire(&mut st).await;
    let created = json!({"id": "urn:E1", "type": "T", "scope": "/a",
        "p": {"type": "Property", "value": 1}});
    let (status, err) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(created)).await;
    assert_eq!(status, StatusCode::CREATED, "{err}");

    let null_scope = json!({"scope": "urn:ngsi-ld:null"});
    let (status, err) = send(
        &st,
        "PATCH",
        "/ngsi-ld/v1/entities/urn:E1/attrs",
        Some(null_scope.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{err}");
    assert!(entity(&st).await.get("scope").is_none());

    // 5.5.4: outside update and merge Fragments the Null is BadRequestData
    let (status, err) = send(
        &st,
        "PATCH",
        "/ngsi-ld/v1/entities/urn:E1",
        Some(json!({"scope": "/b"})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{err}");
    let (status, err) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entities/urn:E1/attrs",
        Some(null_scope),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(entity(&st).await["scope"], "/b");

    for (method, uri) in [
        ("PATCH", "/ngsi-ld/v1/entities/urn:E1"),
        ("PATCH", "/ngsi-ld/v1/entities/urn:E1/attrs"),
    ] {
        let (status, err) = send(&st, method, uri, Some(json!({"type": "urn:ngsi-ld:null"}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {uri}: {err}");
    }
    assert_eq!(entity(&st).await["type"], "T");
}
