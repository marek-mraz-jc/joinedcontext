// SPDX-License-Identifier: EUPL-1.2
//! ADR-0025 rules 2 and 4, ADR-0027 section 9: a forward the registered
//! transport serves goes through `Ops::forward_in_process`, lands in the
//! registration's own Tenant (never the triggering request's), and is
//! decided about the caller who asked. One binary, because it sets the
//! subject header list in the environment before the broker first reads it.

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::{BuiltForward, ForwardFuture, ForwardTransport, Ops};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, PoisonError};
use tower::ServiceExt;

const SELF_URL: &str = "http://self.invalid";

/// Serves every forward in process, under `SELF_URL`.
struct Loopback(Ops);

impl ForwardTransport for Loopback {
    fn forward<'a>(&'a self, req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        Some(Box::pin(self.0.forward_in_process(req, SELF_URL)))
    }
}

/// Records the Tenant and the subject of every decision.
#[derive(Default)]
struct Witness(Mutex<Vec<(String, String)>>);

impl PolicyEngine for Witness {
    fn name(&self) -> &'static str {
        "witness"
    }
    fn decide<'a>(&'a self, s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let who = s
            .headers
            .iter()
            .find(|(k, _)| k == "x-subject")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((s.tenant.as_str().to_owned(), who));
        Box::pin(std::future::ready(Decision::Allow))
    }
    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn broker(witness: Arc<Witness>) -> axum::Router {
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "X-Subject");
    let mut state = antares_api::AppState::new("antares-test".into());
    antares_api::wire(&mut state).await;
    let state = state.with_policy(witness);
    let state = antares_ops::with_forward_transport(state, |ops| Arc::new(Loopback(ops)))
        .unwrap_or_else(|e| unreachable!("{e}"));
    antares_api::router(state)
}

async fn send(
    app: &axum::Router,
    method: &str,
    path: &str,
    tenant: Option<&str>,
    doc: Option<Value>,
) -> (StatusCode, Vec<String>, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(format!("/ngsi-ld/v1/{path}"))
        .header("X-Subject", "alice");
    if let Some(t) = tenant {
        req = req.header("NGSILD-Tenant", t);
    }
    let req = match doc {
        Some(d) => {
            let payload = d.to_string();
            req.header("Content-Type", "application/json")
                .header("Content-Length", payload.len())
                .body(Body::from(payload))
        }
        None => req.body(Body::empty()),
    }
    .unwrap_or_else(|e| unreachable!("{e}"));
    let answer = app
        .clone()
        .oneshot(req)
        .await
        .unwrap_or_else(|e| unreachable!("{e}"));
    let status = answer.status();
    let warnings = answer
        .headers()
        .get_all("ngsild-warning")
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_owned))
        .collect();
    let bytes = answer
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    let doc = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, warnings, doc)
}

async fn register(app: &axum::Router, tenant: Option<&str>, id: &str, reg: Value) {
    let mut doc = json!({
        "id": format!("urn:ngsi-ld:ContextSourceRegistration:{id}"),
        "type": "ContextSourceRegistration",
        "information": [{"entities": [{"type": "Vehicle"}]}],
    });
    if let (Some(d), Some(r)) = (doc.as_object_mut(), reg.as_object()) {
        d.extend(r.clone());
    }
    let (status, _, body) = send(app, "POST", "csourceRegistrations", tenant, Some(doc)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

fn ids(doc: &Value) -> Vec<&str> {
    doc.as_array()
        .map(|a| a.iter().filter_map(|e| e["id"].as_str()).collect())
        .unwrap_or_default()
}

/// A registration without `tenant` targets the default Tenant (4.14) when
/// the triggering request named Tenant `a`, and the inner request is
/// decided about the same caller (rule 4): the subject crossed in process.
#[tokio::test]
async fn a_read_through_the_transport_lands_in_the_registrations_tenant_as_the_caller() {
    let witness = Arc::new(Witness::default());
    let app = broker(witness.clone()).await;
    let vehicle = json!({"id": "urn:ngsi-ld:Vehicle:1", "type": "Vehicle", "speed": {"type": "Property", "value": 9}});
    let (status, _, body) = send(&app, "POST", "entities", None, Some(vehicle)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    register(
        &app,
        Some("a"),
        "self",
        json!({"endpoint": SELF_URL, "operations": ["queryEntity"]}),
    )
    .await;
    witness
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();

    let (status, warnings, body) =
        send(&app, "GET", "entities?type=Vehicle", Some("a"), None).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), ["urn:ngsi-ld:Vehicle:1"], "{warnings:?}");
    let asked = witness
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(asked.contains(&("a".into(), "alice".into())), "{asked:?}");
    assert!(
        asked.contains(&("default".into(), "alice".into())),
        "{asked:?}"
    );
}

/// A write carries its payload into the target Tenant.
#[tokio::test]
async fn a_write_through_the_transport_carries_its_payload() {
    let app = broker(Arc::default()).await;
    register(
        &app,
        Some("a"),
        "self-write",
        json!({"endpoint": SELF_URL, "mode": "redirect", "operations": ["createEntity"]}),
    )
    .await;
    let vehicle = json!({"id": "urn:ngsi-ld:Vehicle:2", "type": "Vehicle", "speed": {"type": "Property", "value": 4}});

    let (status, _, body) = send(&app, "POST", "entities", Some("a"), Some(vehicle)).await;

    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, _, body) = send(&app, "GET", "entities/urn:ngsi-ld:Vehicle:2", None, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["speed"]["value"], 4, "{body}");
    let (status, _, _) = send(
        &app,
        "GET",
        "entities/urn:ngsi-ld:Vehicle:2?local=true",
        Some("a"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the write landed in the registration's Tenant only"
    );
}

/// A forward outside the self URL is not made in process (502), and the
/// target's own warnings about it travel back to the first caller.
#[tokio::test]
async fn an_endpoint_outside_the_self_url_is_refused_and_its_warning_travels_back() {
    let app = broker(Arc::default()).await;
    let vehicle = json!({"id": "urn:ngsi-ld:Vehicle:3", "type": "Vehicle"});
    let (status, _, body) = send(&app, "POST", "entities", None, Some(vehicle)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    register(
        &app,
        None,
        "elsewhere",
        json!({"endpoint": "http://other.invalid", "operations": ["queryEntity"]}),
    )
    .await;
    register(
        &app,
        Some("a"),
        "self",
        json!({"endpoint": SELF_URL, "operations": ["queryEntity"]}),
    )
    .await;

    let (status, warnings, body) =
        send(&app, "GET", "entities?type=Vehicle", Some("a"), None).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), ["urn:ngsi-ld:Vehicle:3"], "{warnings:?}");
    // Vehicle:3 came through the self forward, so that forward answered
    // 2xx: the one 299 is the default Tenant's, about the 502 of its own
    // forward, relayed (6.3.17).
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings.iter().all(|w| w.starts_with("299 ")),
        "{warnings:?}"
    );
}
