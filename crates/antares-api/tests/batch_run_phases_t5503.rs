// SPDX-License-Identifier: EUPL-1.2
//! T-5503: Batch Entity Create split into phases answers as it did in one
//! scope. A mixed array (a local create, an id already taken, a member the
//! engine refuses, two members a registration takes) keeps the 5.6.7.5 207
//! outcome order: S lists the local creates, then the remote ones; E lists
//! the refusal (ADR-0020, decided before any write or forward), then the
//! local errors, then the remote ones.
#![cfg(feature = "test-kit")]
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use antares_api::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_api::{AppState, BuiltForward, ForwardFuture, ForwardTransport};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

/// Refuses every write naming the type `Secret`.
struct NoSecrets;

impl PolicyEngine for NoSecrets {
    fn name(&self) -> &str {
        "no-secrets"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let secret = format!("{DC}Secret");
        Box::pin(std::future::ready(if op.types.contains(&secret) {
            Decision::Deny("a Secret".into())
        } else {
            Decision::Allow
        }))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// The peer behind the registration: records each forwarded body and
/// answers the createBatch with `remote:r` created and `remote:bad` refused.
#[derive(Default)]
struct Peer(Mutex<Vec<Vec<u8>>>);

impl ForwardTransport for Peer {
    fn forward<'a>(&'a self, req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        self.0
            .lock()
            .unwrap()
            .push(req.body().unwrap_or_default().to_vec());
        let answer = json!({
            "success": ["urn:ngsi-ld:Remote:r"],
            "errors": [{"entityId": "urn:ngsi-ld:Remote:bad",
                        "error": {"status": 409, "detail": "taken"}}],
        });
        Some(Box::pin(std::future::ready((207, answer, Vec::new()))))
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
    .unwrap();
    let resp = antares_api::router(st.clone()).oneshot(req).await.unwrap();
    let code = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_run_phases_keep_the_outcome_order() {
    let peer = Arc::new(Peer::default());
    let mut st = AppState::new("me".into()).with_policy(Arc::new(NoSecrets));
    antares_api::wire(&mut st).await;
    let st = st
        .with_forward_transport(Arc::clone(&peer) as Arc<dyn ForwardTransport>)
        .unwrap();
    let reg = json!({
        "id": "urn:ngsi-ld:ContextSourceRegistration:t5503",
        "type": "ContextSourceRegistration",
        "mode": "redirect",
        "information": [{"entities": [{"type": "Remote"}]}],
        "operations": ["createBatch"],
        "endpoint": "http://peer.invalid",
    });
    let (code, body) = send(&st, "POST", "/ngsi-ld/v1/csourceRegistrations", Some(reg)).await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
    let taken = json!({"id": "urn:ngsi-ld:Vehicle:taken", "type": "Vehicle",
                       "n": {"type": "Property", "value": 0}});
    let (code, body) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(taken.clone())).await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
    let (code, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/create",
        Some(json!([
            {"id": "urn:ngsi-ld:Remote:r", "type": "Remote", "n": {"type": "Property", "value": 1}},
            {"id": "urn:ngsi-ld:Secret:x", "type": "Secret", "n": {"type": "Property", "value": 2}},
            {"id": "urn:ngsi-ld:Vehicle:a", "type": "Vehicle", "n": {"type": "Property", "value": 3}},
            taken,
            {"id": "urn:ngsi-ld:Remote:bad", "type": "Remote", "n": {"type": "Property", "value": 4}},
        ])),
    )
    .await;
    assert_eq!(code, StatusCode::MULTI_STATUS, "{body}");
    assert_eq!(
        body["success"],
        json!(["urn:ngsi-ld:Vehicle:a", "urn:ngsi-ld:Remote:r"]),
        "{body}"
    );
    let errors: Vec<(&str, u64)> = body["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["entityId"].as_str().unwrap(),
                e["error"]["status"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        errors,
        vec![
            ("urn:ngsi-ld:Secret:x", 403),
            ("urn:ngsi-ld:Vehicle:taken", 409),
            ("urn:ngsi-ld:Remote:bad", 409),
        ],
        "{body}"
    );
    // the refused member is neither written nor forwarded
    let (code, _) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Secret:x",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
    let forwarded = peer.0.lock().unwrap().clone();
    assert_eq!(forwarded.len(), 1, "one createBatch forward");
    let sent = String::from_utf8(forwarded[0].clone()).unwrap();
    assert!(!sent.contains("Secret"), "{sent}");
    assert!(
        sent.contains("urn:ngsi-ld:Remote:r") && !sent.contains("Vehicle"),
        "{sent}"
    );
    // a redirected member is not stored here
    let (code, _) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Remote:r?local=true",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND);
}

/// F-ANT2-021: Batch Update is Append Attributes per Entity (5.6.9, 5.6.3):
/// the instance the update names replaces the stored one, so a `unitCode`
/// the update leaves out is gone, not merged over.
#[tokio::test]
async fn a_batch_update_replaces_the_instance_it_names() {
    let st = AppState::new("antares-batch-update".into());
    let id = "urn:ngsi-ld:Room:u1";
    let (code, _) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(json!({"id": id, "type": "Room",
            "temp": {"type": "Property", "value": 1, "unitCode": "CEL"}})),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let (code, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/update",
        Some(json!([{"id": id, "type": "Room", "temp": {"type": "Property", "value": 2}}])),
    )
    .await;
    assert_eq!(code, StatusCode::NO_CONTENT, "{body}");
    let (_, e) = send(&st, "GET", &format!("/ngsi-ld/v1/entities/{id}"), None).await;
    assert_eq!(e.pointer("/temp/value"), Some(&json!(2)));
    assert_eq!(e.pointer("/temp/unitCode"), None, "{e}");
}

/// F-ANT2-022: an instance a noOverwrite Batch Update appends is stamped as
/// every other write stamps one.
#[tokio::test]
async fn a_no_overwrite_batch_update_stamps_what_it_appends() {
    let st = AppState::new("antares-batch-append".into());
    let id = "urn:ngsi-ld:Room:u2";
    let (code, _) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(json!({"id": id, "type": "Room", "temp": {"type": "Property", "value": 1}})),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED);
    let (code, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/update?options=noOverwrite",
        Some(json!([{"id": id, "type": "Room", "hum": {"type": "Property", "value": 40}}])),
    )
    .await;
    assert_eq!(code, StatusCode::NO_CONTENT, "{body}");
    let (_, e) = send(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/entities/{id}?options=sysAttrs"),
        None,
    )
    .await;
    assert!(e.pointer("/hum/createdAt").is_some(), "{e}");
    assert!(e.pointer("/hum/modifiedAt").is_some(), "{e}");
}
