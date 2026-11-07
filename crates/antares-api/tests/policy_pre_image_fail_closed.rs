// SPDX-License-Identifier: EUPL-1.2
//! A pre-image the store cannot read fails the write (ADR-0020 fail-closed,
//! F-ANT2-009). Before, the failed read became an absent Entity and an
//! engine keyed on the stored types decided over nothing: a rule that
//! refuses writes to a `Secret` Entity let them through.
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use crate::common;

use antares_api::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_api::AppState;
use antares_model::{NgsiError, TenantId};
use antares_store::{filter, Kind, MutateFn, TemporalDriver};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::Double;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const ID: &str = "urn:ngsi-ld:Secret:1";

/// Refuses any write whose stored pre-image is a `Secret`.
struct NoSecrets;

impl PolicyEngine for NoSecrets {
    fn name(&self) -> &str {
        "no-secrets"
    }
    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let secret = op
            .stored
            .is_some_and(|s| s.types.iter().any(|t| t.ends_with("Secret")));
        Box::pin(std::future::ready(if secret {
            Decision::Deny("secret".into())
        } else {
            Decision::Allow
        }))
    }
    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn send(st: &AppState, method: &str, uri: &str, body: &str) -> StatusCode {
    antares_api::router(st.clone())
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
        .unwrap()
        .status()
}

async fn seeded() -> AppState {
    let st = AppState::new("me".into());
    let e = json!({"id": ID, "type": "Secret", "pin": {"type": "Property", "value": 1}});
    assert_eq!(
        send(&st, "POST", "/ngsi-ld/v1/entities", &e.to_string()).await,
        StatusCode::CREATED
    );
    st
}

#[tokio::test]
async fn an_unreadable_pre_image_fails_the_write_instead_of_allowing_it() {
    let st = seeded().await;
    let patch = json!({"pin": {"type": "Property", "value": 2}}).to_string();
    let uri = format!("/ngsi-ld/v1/entities/{ID}/attrs");
    // readable: the engine sees the Secret and refuses
    let guarded = st.clone().with_policy(Arc::new(NoSecrets));
    assert_eq!(
        send(&guarded, "PATCH", &uri, &patch).await,
        StatusCode::FORBIDDEN
    );
    // unreadable: the request fails, it is not decided over an absent Entity
    let mut broken = guarded.clone();
    broken.store = Arc::new(Double::refusing_doc(st.store.clone(), Kind::Entity, ID));
    assert_eq!(
        send(&broken, "PATCH", &uri, &patch).await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let doc = st
        .store
        .get(&Default::default(), Kind::Entity, ID)
        .await
        .unwrap()
        .unwrap();
    assert!(
        doc.to_string().contains("\"value\":1"),
        "the write never ran: {doc}"
    );
}

/// The batch delete reads every pre-image in one call and fails closed the
/// same way (F-ANT2-025).
#[tokio::test]
async fn a_batch_delete_with_an_unreadable_pre_image_fails_whole() {
    let st = seeded().await;
    let mut broken = st.clone().with_policy(Arc::new(NoSecrets));
    broken.store = Arc::new(Double::refusing_doc(st.store.clone(), Kind::Entity, ID));
    let body = json!([ID]).to_string();
    assert_eq!(
        send(
            &broken,
            "POST",
            "/ngsi-ld/v1/entityOperations/delete",
            &body
        )
        .await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert!(st
        .store
        .get(&Default::default(), Kind::Entity, ID)
        .await
        .unwrap()
        .is_some());
}

/// A batch delete waits for a batch slot like the write batches (F-ANT2-026).
#[tokio::test]
async fn a_batch_delete_waits_for_a_batch_slot() {
    let st = seeded().await;
    let mut held = Vec::new();
    for _ in 0..*antares_api::bounds::BATCH_INFLIGHT {
        held.push(antares_api::bounds::admit_batch(0).await.unwrap());
    }
    let body = json!([ID]).to_string();
    let call = {
        let st = st.clone();
        tokio::spawn(async move {
            send(&st, "POST", "/ngsi-ld/v1/entityOperations/delete", &body).await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!call.is_finished(), "ran without a slot");
    drop(held);
    assert_eq!(call.await.unwrap(), StatusCode::NO_CONTENT);
}

/// The batch upsert and update read their pre-images in one `get_entities`
/// and fail closed the same way (F-ANT2-009).
#[tokio::test]
async fn a_batch_write_with_an_unreadable_pre_image_fails_whole() {
    let e = json!([{"id": ID, "type": "Secret", "pin": {"type": "Property", "value": 2}}]);
    for op in ["upsert", "update"] {
        let st = seeded().await;
        let mut broken = st.clone().with_policy(Arc::new(NoSecrets));
        broken.store = Arc::new(Double::refusing_doc(st.store.clone(), Kind::Entity, ID));
        let uri = format!("/ngsi-ld/v1/entityOperations/{op}");
        assert_eq!(
            send(&broken, "POST", &uri, &e.to_string()).await,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{op}"
        );
        let doc = st
            .store
            .get(&Default::default(), Kind::Entity, ID)
            .await
            .unwrap()
            .unwrap();
        assert!(doc.to_string().contains("\"value\":1"), "{op} wrote: {doc}");
    }
}

/// A temporal store that cannot read one evolution; everything else passes.
struct RefusingTemporal(Arc<dyn TemporalDriver>);

#[async_trait::async_trait]
impl TemporalDriver for RefusingTemporal {
    async fn temporal_append(
        &self,
        t: &TenantId,
        id: &str,
        shell: &Value,
        additions: &Value,
    ) -> Result<(), NgsiError> {
        self.0.temporal_append(t, id, shell, additions).await
    }
    async fn query_temporal(
        &self,
        t: &TenantId,
        f: &filter::TemporalFilter<'_>,
    ) -> Result<filter::TemporalOutcome, NgsiError> {
        self.0.query_temporal(t, f).await
    }
    async fn get_temporal(
        &self,
        t: &TenantId,
        id: &str,
        f: &filter::TemporalFilter<'_>,
    ) -> Result<Option<Value>, NgsiError> {
        self.0.get_temporal(t, id, f).await
    }
    async fn get(&self, _t: &TenantId, id: &str) -> Result<Option<Value>, NgsiError> {
        Err(NgsiError::InternalError(format!("{id} unreadable")))
    }
    async fn create(&self, t: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError> {
        self.0.create(t, id, doc).await
    }
    async fn upsert(&self, t: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError> {
        self.0.upsert(t, id, doc).await
    }
    async fn delete(&self, t: &TenantId, id: &str) -> Result<bool, NgsiError> {
        self.0.delete(t, id).await
    }
    async fn list(&self, t: &TenantId) -> Result<Vec<Value>, NgsiError> {
        self.0.list(t).await
    }
    async fn mutate_boxed<'a>(
        &self,
        t: &TenantId,
        id: &str,
        f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        self.0.mutate_boxed(t, id, f).await
    }
}

/// A temporal write whose evolution cannot be read fails; it is not
/// decided over the absent current state (F-ANT2-009, temporal_pre_image).
#[tokio::test]
async fn a_temporal_write_with_an_unreadable_evolution_fails() {
    const T: &str = "urn:ngsi-ld:Secret:t";
    let st = AppState::new("me".into());
    let e = json!({"id": T, "type": "Secret", "pin": [
        {"type": "Property", "value": 1, "observedAt": "2026-01-01T00:00:00Z"}
    ]});
    assert_eq!(
        send(&st, "POST", "/ngsi-ld/v1/temporal/entities", &e.to_string()).await,
        StatusCode::CREATED
    );
    let uri = format!("/ngsi-ld/v1/temporal/entities/{T}");
    let guarded = st.clone().with_policy(Arc::new(NoSecrets));
    assert_eq!(
        send(&guarded, "DELETE", &uri, "").await,
        StatusCode::FORBIDDEN
    );
    let mut broken = guarded.clone();
    broken.temporal = Arc::new(RefusingTemporal(st.temporal.clone()));
    assert_eq!(
        send(&broken, "DELETE", &uri, "").await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert!(st
        .temporal
        .get(&Default::default(), T)
        .await
        .unwrap()
        .is_some());
}
