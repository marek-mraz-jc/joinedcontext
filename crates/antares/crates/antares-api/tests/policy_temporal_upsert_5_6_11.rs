// SPDX-License-Identifier: EUPL-1.2
//! T-4855: Upsert Temporal (5.6.11) asks the policy engine with what
//! Create Entity (5.6.1) gives it: the Entity's types, its Attributes and
//! the expanded body. Asked with the id alone, an engine could never admit
//! it under a grant scoped to a type, and could not see an access-control
//! member written into the history.

use antares_api::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const CTX: &str = "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld";
const DEFAULT: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

/// Each 5.6.11 question: its types, Attributes, whether a body came and
/// whether a stored evolution came.
type Asked = Arc<Mutex<Vec<(Vec<String>, Vec<String>, bool, bool)>>>;

struct Recorder(Asked);

impl PolicyEngine for Recorder {
    fn name(&self) -> &str {
        "recorder"
    }
    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        if op.clause == "5.6.11" {
            self.0.lock().expect("lock").push((
                op.types.to_vec(),
                op.attrs.to_vec(),
                op.body.is_some(),
                op.stored.is_some(),
            ));
        }
        Box::pin(std::future::ready(Decision::Allow))
    }
    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

#[tokio::test]
async fn the_engine_sees_the_types_attributes_and_body_of_a_temporal_upsert() {
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    let st = AppState::new("t".into()).with_policy(Arc::new(Recorder(asked.clone())));
    let body = json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle", "@context": CTX,
                      "speed": [{"type": "Property", "value": 1,
                                 "observedAt": "2026-01-01T00:00:00Z"}]})
    .to_string();
    let resp = antares_api::router(st)
        .oneshot(
            Request::post("/ngsi-ld/v1/temporal/entities")
                .header("Content-Type", "application/ld+json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let asked = asked.lock().expect("lock").clone();
    assert_eq!(
        asked,
        vec![(
            vec![format!("{DEFAULT}Vehicle")],
            vec![format!("{DEFAULT}speed")],
            true,
            false
        )]
    );
}

async fn upsert(st: &AppState, ty: &str) -> StatusCode {
    let body = json!({"id": "urn:ngsi-ld:Vehicle:1", "type": ty, "@context": CTX,
                      "speed": [{"type": "Property", "value": 1,
                                 "observedAt": "2026-01-01T00:00:00Z"}]})
    .to_string();
    antares_api::router(st.clone())
        .oneshot(
            Request::post("/ngsi-ld/v1/temporal/entities")
                .header("Content-Type", "application/ld+json")
                .header("Content-Length", body.len())
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("response")
        .status()
}

/// An upsert adds to the stored evolution, so the engine judges it as Batch
/// Upsert is judged: with the stored evolution and its types beside the
/// body's, never the body's types alone.
#[tokio::test]
async fn the_engine_sees_the_stored_evolution_a_temporal_upsert_adds_to() {
    let asked: Asked = Arc::new(Mutex::new(Vec::new()));
    let st = AppState::new("t".into()).with_policy(Arc::new(Recorder(asked.clone())));
    assert_eq!(upsert(&st, "Vehicle").await, StatusCode::CREATED);
    assert_eq!(upsert(&st, "Car").await, StatusCode::NO_CONTENT);
    let asked = asked.lock().expect("lock").clone();
    assert_eq!(
        asked.get(1),
        Some(&(
            vec![format!("{DEFAULT}Vehicle"), format!("{DEFAULT}Car")],
            vec![format!("{DEFAULT}speed")],
            true,
            true
        ))
    );
}
