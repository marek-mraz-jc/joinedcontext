// SPDX-License-Identifier: EUPL-1.2
//! T-5912: a check a module registers with `Ops::check_writes` holds every
//! NGSI-LD write of a Tenant with a schema, under `ANTARES_SCHEMA_WRITES=enforce`
//! (read when the views loader starts); a Tenant without a schema is written
//! as before.

use antares_ops::test_kit::TestBroker;
use antares_ops::views::{SCHEMA_MAX_ENV, SCHEMA_WRITES_ENV};
use antares_ops::{BuiltForward, ForwardFuture, ForwardTransport};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::sync::{Arc, PoisonError};
use tower::ServiceExt;

/// Forwards nothing in process: every forward goes over HTTP.
struct Http;

impl ForwardTransport for Http {
    fn forward<'a>(&'a self, _: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        None
    }
}

/// Refuses an Entity holding `bad`, by its text alone.
fn no_bad(_: u64, _: &Arc<[u8]>, doc: &Value, _: bool) -> Result<(), String> {
    if doc.to_string().contains("bad") {
        Err("the attribute n breaks the data model of T: no".into())
    } else {
        Ok(())
    }
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    tenant: &str,
    doc: &Value,
) -> (StatusCode, Value) {
    let payload = doc.to_string();
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("NGSILD-Tenant", tenant)
        .header("Content-Type", "application/json")
        .header("Content-Length", payload.len())
        .body(Body::from(payload))
        .unwrap_or_else(|e| unreachable!("{e}"));
    let answer = app
        .clone()
        .oneshot(req)
        .await
        .unwrap_or_else(|e| unreachable!("{e}"));
    let status = answer.status();
    let bytes = axum::body::to_bytes(answer.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_registered_check_refuses_a_write_to_a_tenant_with_a_schema() {
    std::env::set_var(SCHEMA_MAX_ENV, "65536");
    std::env::set_var(SCHEMA_WRITES_ENV, "enforce");
    let kept = Arc::new(std::sync::Mutex::new(None));
    let keep = kept.clone();
    let b = TestBroker::new()
        .await
        .unwrap_or_else(|e| unreachable!("{e}"))
        .with_forward_transport(move |ops| {
            ops.check_writes(no_bad);
            *keep.lock().unwrap_or_else(PoisonError::into_inner) = Some(());
            Arc::new(Http)
        })
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert!(kept
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some());
    let app = b.router();
    let schema = json!({"id": "https://e.org/", "name": "s"});
    let (s, body) = send(&app, "PUT", "/q/schemas/held", "held", &schema).await;
    assert!(s.is_success(), "{s} {body}");
    let bad =
        json!({"id": "urn:ngsi-ld:T:1", "type": "T", "n": {"type": "Property", "value": "bad"}});
    let (s, body) = send(&app, "POST", "/ngsi-ld/v1/entities", "held", &bad).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["detail"], "the attribute n breaks the data model of T: no",
        "{body}"
    );
    let (s, body) = send(&app, "POST", "/ngsi-ld/v1/entities", "free", &bad).await;
    assert_eq!(s, StatusCode::CREATED, "a Tenant without a schema: {body}");
}

/// Refuses a whole Entity without `n`, as a schema with a required slot does; a fragment
/// passes.
fn needs_n(_: u64, _: &Arc<[u8]>, doc: &Value, whole: bool) -> Result<(), String> {
    if whole && !doc.to_string().contains("/n\"") {
        Err("the Entity lacks the required attribute n".into())
    } else {
        Ok(())
    }
}

/// F-R2ANT1-009: an upsert with options=update creates a member whose Entity is absent, and
/// a created Entity is held to the whole-Entity rules, not the fragment ones.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_upsert_creating_an_entity_is_held_to_the_whole_entity_rules() {
    std::env::set_var(SCHEMA_MAX_ENV, "65536");
    std::env::set_var(SCHEMA_WRITES_ENV, "enforce");
    let b = TestBroker::new()
        .await
        .unwrap_or_else(|e| unreachable!("{e}"))
        .with_forward_transport(|ops| {
            ops.check_writes(needs_n);
            Arc::new(Http)
        })
        .unwrap_or_else(|e| unreachable!("{e}"));
    let app = b.router();
    let schema = json!({"id": "https://e.org/", "name": "s"});
    let (s, body) = send(&app, "PUT", "/q/schemas/whole", "whole", &schema).await;
    assert!(s.is_success(), "{s} {body}");
    let partial =
        json!([{"id": "urn:ngsi-ld:T:2", "type": "T", "m": {"type": "Property", "value": 1}}]);
    let (s, body) = send(
        &app,
        "POST",
        "/ngsi-ld/v1/entityOperations/upsert?options=update",
        "whole",
        &partial,
    )
    .await;
    assert!(
        matches!(s, StatusCode::MULTI_STATUS | StatusCode::BAD_REQUEST),
        "{s} {body}"
    );
    assert_eq!(
        body.pointer("/errors/0/error/detail"),
        Some(&json!("the Entity lacks the required attribute n")),
        "{body}"
    );
    let (s, body) = send(
        &app,
        "POST",
        "/ngsi-ld/v1/entityOperations/upsert?options=update",
        "whole",
        &json!([{"id": "urn:ngsi-ld:T:2", "type": "T", "n": {"type": "Property", "value": 1}}]),
    )
    .await;
    assert!(s.is_success(), "a whole member is created: {s} {body}");
    // a stored Entity is updated by a fragment
    let (s, body) = send(
        &app,
        "POST",
        "/ngsi-ld/v1/entityOperations/upsert?options=update",
        "whole",
        &partial,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{body}");
}
