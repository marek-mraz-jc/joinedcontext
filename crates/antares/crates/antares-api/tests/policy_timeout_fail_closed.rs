// SPDX-License-Identifier: EUPL-1.2
//! An engine that does not answer in time still refuses (T-6614, R1-019):
//! the answer is 503 with Retry-After instead of 403, and nothing else
//! changes. The write is not applied and the read carries no data.
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use antares_api::policy::{
    DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_api::AppState;
use antares_store::Kind;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const ID: &str = "urn:ngsi-ld:Vault:t6614";

/// Never answers: every decision runs into the seam's timeout.
struct Never;

impl PolicyEngine for Never {
    fn name(&self) -> &str {
        "never"
    }
    fn decide<'a>(&'a self, _s: &'a Subject, _o: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::pending())
    }
    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn send(st: &AppState, method: &str, uri: &str, body: &str) -> (StatusCode, String, String) {
    let res = antares_api::router(st.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("Content-Type", "application/json")
                .header("Content-Length", body.len())
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let retry = res
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, retry, String::from_utf8_lossy(&bytes).into_owned())
}

fn entity(n: i64) -> String {
    json!({"id": ID, "type": "Vault", "pin": {"type": "Property", "value": n}}).to_string()
}

async fn stored(st: &AppState) -> Option<Value> {
    st.store
        .get(&Default::default(), Kind::Entity, ID)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_timed_out_create_answers_503_and_writes_nothing() {
    let st = AppState::new("me".into()).with_policy(Arc::new(Never));
    let (status, retry, _) = send(&st, "POST", "/ngsi-ld/v1/entities", &entity(1)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(retry, "1");
    assert!(stored(&st).await.is_none(), "the create was applied");
}

#[tokio::test]
async fn a_timed_out_read_or_update_answers_503_and_shows_or_changes_nothing() {
    let open = AppState::new("me".into());
    let (status, _, _) = send(&open, "POST", "/ngsi-ld/v1/entities", &entity(7341)).await;
    assert_eq!(status, StatusCode::CREATED);
    let st = open.clone().with_policy(Arc::new(Never));
    for uri in [
        format!("/ngsi-ld/v1/entities/{ID}"),
        "/ngsi-ld/v1/entities?type=Vault".to_owned(),
    ] {
        let (status, retry, body) = send(&st, "GET", &uri, "").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}");
        assert_eq!(retry, "1", "{uri}");
        assert!(!body.contains(ID) && !body.contains("7341"), "{uri}: {body}");
    }
    let patch = json!({"pin": {"type": "Property", "value": 2}}).to_string();
    let (status, _, _) = send(&st, "PATCH", &format!("/ngsi-ld/v1/entities/{ID}/attrs"), &patch).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let doc = stored(&st).await.unwrap();
    assert!(doc.to_string().contains("7341"), "the update was applied: {doc}");
}

/// The batch path decides every Entity in one call (`decide_each`): a
/// timeout there refuses each of them.
#[tokio::test]
async fn a_timed_out_batch_upsert_writes_nothing() {
    let st = AppState::new("me".into()).with_policy(Arc::new(Never));
    let batch = format!("[{}]", entity(1));
    let (status, _, body) = send(&st, "POST", "/ngsi-ld/v1/entityOperations/upsert", &batch).await;
    assert!(
        !matches!(status, StatusCode::CREATED | StatusCode::NO_CONTENT),
        "{status}: {body}"
    );
    assert!(stored(&st).await.is_none(), "the upsert was applied: {body}");
}
