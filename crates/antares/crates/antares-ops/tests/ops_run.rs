// SPDX-License-Identifier: EUPL-1.2
//! G2 (surfaces-threat-model 2.5): the tenant, the snapshot and the policy
//! subject an operation runs as are the outer request's. A surface that sets
//! any of them on the request it builds has it dropped. One test in its own
//! binary, because it sets the subject header list in the environment before
//! the broker first reads it.

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use antares_ops::{Caller, OperationId, Ops, OpsRequest, Surface};
use axum::body::Body;
use axum::extract::Path;
use axum::http::{HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// Records the subject header every decision was asked with.
#[derive(Default)]
struct Witness(Mutex<Vec<String>>);

impl PolicyEngine for Witness {
    fn name(&self) -> &'static str {
        "witness"
    }
    fn decide<'a>(&'a self, s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let seen: Vec<String> = s
            .headers
            .iter()
            .filter(|(k, _)| k == "x-subject")
            .map(|(_, v)| v.clone())
            .collect();
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(seen);
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

/// A surface that tries to choose the tenant, the snapshot and the subject.
struct Forger;

impl Surface for Forger {
    fn name(&self) -> &'static str {
        "forger"
    }
    fn prefix(&self) -> &'static str {
        "/x/forger"
    }
    fn version_info(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        Router::new().route(
            "/{id}",
            get(move |caller: Caller, Path(id): Path<String>| async move {
                let forged = |name: &'static str, value: &'static str| {
                    (
                        HeaderName::from_static(name),
                        HeaderValue::from_static(value),
                    )
                };
                let mut req = OpsRequest::new(Method::GET, format!("/ngsi-ld/v1/entities/{id}"));
                for (k, v) in [
                    forged("ngsild-tenant", "evil"),
                    forged("ngsild-snapshot", "urn:ngsi-ld:Snapshot:none"),
                    forged("x-subject", "mallory"),
                ] {
                    req = req.header(k, v);
                }
                match ops.run(OperationId::RetrieveEntity, &caller, req).await {
                    Ok(r) => r,
                    Err(refusal) => refusal.ngsi_ld(),
                }
            }),
        )
    }
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
    let Ok(resp) = app.clone().oneshot(req).await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// Rule M4 keeps every surface closed until a view loads: one over Cars.
async fn open(app: &Router) {
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
}

fn create(tenant: &str, colour: &str) -> Result<Request<Body>, axum::http::Error> {
    let body = format!(
        r#"{{"id":"urn:ngsi-ld:Car:1","type":"Car","colour":{{"type":"Property","value":"{colour}"}}}}"#
    );
    Request::post("/ngsi-ld/v1/entities")
        .header("NGSILD-Tenant", tenant)
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .header("x-subject", "alice")
        .body(Body::from(body))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_surface_cannot_set_tenant_snapshot_or_subject() {
    // Before anything reads the list: the broker carries `x-subject` to the engine.
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "x-subject");
    let witness = Arc::new(Witness::default());
    let broker = TestBroker::new()
        .await
        .expect("broker")
        .with_policy(witness.clone());
    let app = broker.serve(Forger).expect("mounts");
    open(&app).await;

    for (tenant, colour) in [("good", "green"), ("evil", "black")] {
        let (s, b) = send(&app, create(tenant, colour).expect("request")).await;
        assert_eq!(s, StatusCode::CREATED, "{tenant}: {b}");
    }
    witness.0.lock().expect("lock").clear();

    let read = |tenant: Option<&str>| {
        let mut r = Request::get("/x/forger/urn:ngsi-ld:Car:1").header("x-subject", "alice");
        if let Some(t) = tenant {
            r = r.header("NGSILD-Tenant", t);
        }
        r.body(Body::empty()).expect("request")
    };
    let (s, b) = send(&app, read(Some("good"))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["colour"]["value"], "green", "the outer tenant wins: {b}");

    let (s, b) = send(&app, read(None)).await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "no outer tenant is the default tenant, never the surface's: {b}"
    );

    let asked = witness.0.lock().expect("lock").clone();
    assert!(!asked.is_empty(), "the engine was asked");
    assert!(
        asked.iter().all(|v| v == "alice"),
        "the outer subject wins: {asked:?}"
    );
}
