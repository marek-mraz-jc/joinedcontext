// SPDX-License-Identifier: EUPL-1.2
//! T-3702 (ADR-0020 amendment): the engine learns the view a request came
//! through from the operation layer, and from nothing a client sends. A
//! request through `ViewOps` carries the view; the same operation through
//! `Ops` carries none; `offered` asks with the view as well.

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use antares_ops::{Caller, OperationId, Ops, OpsRequest, Surface};
use axum::body::Body;
use axum::extract::Path;
use axum::http::{Method, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// Records the view of every subject it is asked about.
#[derive(Default)]
struct Witness(Mutex<Vec<Option<String>>>);

impl Witness {
    fn take(&self) -> Vec<Option<String>> {
        std::mem::take(
            &mut *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

impl PolicyEngine for Witness {
    fn name(&self) -> &'static str {
        "witness"
    }
    fn decide<'a>(&'a self, s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(s.view().map(str::to_owned));
        Box::pin(std::future::ready(Decision::Allow))
    }
    fn pre_notify(
        &self,
        _s: &Subject,
        _sub: &serde_json::Value,
        _n: &mut serde_json::Value,
    ) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// Runs one read through a view, the same read through no view, and asks
/// what a view offers.
struct Probe;

impl Surface for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }
    fn prefix(&self) -> &'static str {
        "/x/probe"
    }
    fn version_info(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        let plain = ops.clone();
        let offered = ops.clone();
        let read = || {
            OpsRequest::new(
                Method::GET,
                "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:1".to_owned(),
            )
        };
        Router::new()
            .route(
                "/view/{view}",
                get(move |caller: Caller, Path(id): Path<String>| async move {
                    let Some(view) = ops.view(&id) else {
                        return StatusCode::NOT_FOUND.into_response();
                    };
                    match view.run(OperationId::RetrieveEntity, &caller, read()).await {
                        Ok(r) => r,
                        Err(refusal) => refusal.ngsi_ld(),
                    }
                }),
            )
            .route(
                "/plain",
                get(move |caller: Caller| async move {
                    match plain
                        .run(OperationId::RetrieveEntity, &caller, read())
                        .await
                    {
                        Ok(r) => r,
                        Err(refusal) => refusal.ngsi_ld(),
                    }
                }),
            )
            .route(
                "/offered/{view}",
                get(move |caller: Caller, Path(id): Path<String>| async move {
                    match offered.view(&id) {
                        Some(v) => {
                            axum::Json(v.offered(&caller, &[OperationId::RetrieveEntity]).await)
                                .into_response()
                        }
                        None => StatusCode::NOT_FOUND.into_response(),
                    }
                }),
            )
    }
}

use axum::response::IntoResponse as _;

async fn get_status(app: &Router, uri: &str) -> StatusCode {
    let Ok(resp) = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap_or_default())
        .await;
    resp.status()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_engine_hears_the_view_from_the_operation_layer_alone() {
    let witness = Arc::new(Witness::default());
    let broker = TestBroker::new()
        .await
        .expect("broker")
        .with_policy(witness.clone());
    let app = broker.serve(Probe).expect("mounts");
    let doc = r#"{"id":"cars","tenant":"good","selector":["Car"],
        "rules":[{"types":["Car"],"attributes":["colour"]}],
        "operations":["retrieve_entity","query_entities"]}"#;
    let Ok(resp) = app
        .clone()
        .oneshot(
            Request::put("/q/views/cars")
                .header("Content-Type", "application/json")
                .header("Content-Length", doc.len())
                .body(Body::from(doc))
                .unwrap_or_default(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "the view loads");
    for tenant in ["good", ""] {
        let car =
            r#"{"id":"urn:ngsi-ld:Car:1","type":"Car","colour":{"type":"Property","value":"red"}}"#;
        let mut create = Request::post("/ngsi-ld/v1/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", car.len());
        if !tenant.is_empty() {
            create = create.header("NGSILD-Tenant", tenant);
        }
        let Ok(resp) = app
            .clone()
            .oneshot(create.body(Body::from(car)).unwrap_or_default())
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED, "seeded in {tenant:?}");
    }
    witness.take();

    let status = get_status(&app, "/x/probe/view/cars").await;
    let through = witness.take();
    assert!(!through.is_empty(), "the engine was asked ({status})");
    assert!(
        through.iter().all(|v| v.as_deref() == Some("cars")),
        "{through:?}"
    );

    let status = get_status(&app, "/x/probe/plain").await;
    let plain = witness.take();
    assert!(!plain.is_empty(), "the engine was asked ({status})");
    assert!(plain.iter().all(Option::is_none), "{plain:?}");

    assert_eq!(
        get_status(&app, "/x/probe/offered/cars").await,
        StatusCode::OK
    );
    let offered = witness.take();
    assert!(!offered.is_empty(), "the engine was asked");
    assert!(
        offered.iter().all(|v| v.as_deref() == Some("cars")),
        "{offered:?}"
    );
}
