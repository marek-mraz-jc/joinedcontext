// SPDX-License-Identifier: EUPL-1.2
//! 5.6.7 with a registration narrowed to an Attribute: a batch member of the
//! registered type that carries none of the registered Attributes is no part
//! of what that source holds, so nothing is forwarded to it, and the member
//! is created here as any unregistered one.
#![cfg(feature = "test-kit")]

use antares_api::{AppState, BuiltForward, ForwardFuture, ForwardTransport};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// The peer behind the registration: counts the forwards it is handed.
#[derive(Default)]
struct Peer(Mutex<usize>);

impl ForwardTransport for Peer {
    fn forward<'a>(&'a self, _req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        *self.0.lock().expect("lock") += 1;
        Some(Box::pin(std::future::ready((204, Value::Null, Vec::new()))))
    }
}

async fn send(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => {
            let payload = v.to_string();
            b.header("Content-Type", "application/json")
                .header("Content-Length", payload.len())
                .body(Body::from(payload))
        }
        None => b.body(Body::empty()),
    }
    .expect("request");
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let code = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_without_a_registered_attribute_is_not_forwarded() {
    let peer = Arc::new(Peer::default());
    let mut st = AppState::new("me".into());
    antares_api::wire(&mut st).await;
    let st = st
        .with_forward_transport(Arc::clone(&peer) as Arc<dyn ForwardTransport>)
        .expect("a transport on a fresh state");
    let reg = json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:scope",
        "type": "ContextSourceRegistration",
        "mode": "inclusive",
        "information": [{"entities": [{"type": "Remote"}], "propertyNames": ["speed"]}],
        "operations": ["createBatch"],
        "endpoint": "http://peer.invalid",
    });
    let (code, body) = send(&st, "POST", "/ngsi-ld/v1/csourceRegistrations", Some(reg)).await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
    let (code, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/create",
        Some(json!([
            {"id": "urn:ngsi-ld:Remote:r", "type": "Remote",
             "brand": {"type": "Property", "value": "x"}},
            {"id": "urn:ngsi-ld:Other:o", "type": "Other",
             "speed": {"type": "Property", "value": 1}},
        ])),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
    assert_eq!(*peer.0.lock().expect("lock"), 0, "nothing was forwarded");
    for id in ["urn:ngsi-ld:Remote:r", "urn:ngsi-ld:Other:o"] {
        let (code, body) = send(
            &st,
            "GET",
            &format!("/ngsi-ld/v1/entities/{id}?local=true"),
            None,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{id} is stored here: {body}");
    }
}

/// With an exclusive registration in play, a member that is no JSON object
/// is still no local part of anything: it fails alone as an invalid member,
/// and the proxied member goes to the source.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_that_is_no_object_fails_alone_beside_an_exclusive_registration() {
    let peer = Arc::new(Peer::default());
    let mut st = AppState::new("me".into());
    antares_api::wire(&mut st).await;
    let st = st
        .with_forward_transport(Arc::clone(&peer) as Arc<dyn ForwardTransport>)
        .expect("a transport on a fresh state");
    let reg = json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:exclusive",
        "type": "ContextSourceRegistration",
        "mode": "exclusive",
        "information": [{"entities": [{"id": "urn:ngsi-ld:Remote:r", "type": "Remote"}], "propertyNames": ["speed"]}],
        "operations": ["createBatch"],
        "endpoint": "http://peer.invalid",
    });
    let (code, body) = send(&st, "POST", "/ngsi-ld/v1/csourceRegistrations", Some(reg)).await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
    let (code, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/create",
        Some(json!([
            7,
            {"id": "urn:ngsi-ld:Remote:r", "type": "Remote",
             "speed": {"type": "Property", "value": 1}},
        ])),
    )
    .await;
    assert_eq!(code, StatusCode::MULTI_STATUS, "{body}");
    let bad = "https://uri.etsi.org/ngsi-ld/errors/BadRequestData";
    assert!(
        body["errors"]
            .as_array()
            .is_some_and(|e| e.iter().any(|x| x["error"]["type"] == bad)),
        "{body}"
    );
    assert_eq!(*peer.0.lock().expect("lock"), 1, "the proxied member went");
}

/// A delete batch holding no string id sends nothing to a registration that
/// would take any id, and the member fails as an invalid id.
#[tokio::test(flavor = "multi_thread")]
async fn a_delete_batch_without_a_string_id_forwards_nothing() {
    let peer = Arc::new(Peer::default());
    let mut st = AppState::new("me".into());
    antares_api::wire(&mut st).await;
    let st = st
        .with_forward_transport(Arc::clone(&peer) as Arc<dyn ForwardTransport>)
        .expect("a transport on a fresh state");
    let reg = json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:delete",
        "type": "ContextSourceRegistration",
        "mode": "inclusive",
        "information": [{"entities": [{"type": "Remote"}]}],
        "operations": ["deleteBatch"],
        "endpoint": "http://peer.invalid",
    });
    let (code, body) = send(&st, "POST", "/ngsi-ld/v1/csourceRegistrations", Some(reg)).await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
    let (code, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/delete",
        Some(json!([5])),
    )
    .await;
    assert_ne!(code, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(*peer.0.lock().expect("lock"), 0, "nothing was forwarded");
}
