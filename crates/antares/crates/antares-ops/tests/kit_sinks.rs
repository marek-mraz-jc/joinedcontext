// SPDX-License-Identifier: EUPL-1.2
//! What a surface with its own notification channel builds on (ADR-0026):
//! the test kit registers its sink and its policy engine beside the
//! broker's, mounts its surfaces, and a handler reads the caller's subject
//! headers and a view's Tenant. One test in its own binary, because it sets
//! the subject header list in the environment before the broker first
//! reads it.

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use antares_ops::{Caller, DeliveryFuture, NgsiError, NotificationSink, Ops, Outbound, Surface};
use axum::body::Body;
use axum::extract::Path;
use axum::http::{Method, Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

/// Allows everything and counts what it was asked.
#[derive(Default)]
struct Counting(AtomicUsize);

impl PolicyEngine for Counting {
    fn name(&self) -> &'static str {
        "counting"
    }
    fn decide<'a>(&'a self, _s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Box::pin(std::future::ready(Decision::Allow))
    }
    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// Takes `urn:kit:` endpoints and counts its deliveries.
struct Kit(Arc<AtomicUsize>);

impl NotificationSink for Kit {
    fn schemes(&self) -> &'static [&'static str] {
        &["urn"]
    }
    fn parse_endpoint(&self, uri: &str, _info: &[(&str, &str)]) -> Result<(), NgsiError> {
        if uri.starts_with("urn:kit:") {
            Ok(())
        } else {
            Err(NgsiError::BadRequestData("not a kit endpoint".into()))
        }
    }
    fn network(&self) -> bool {
        false
    }
    fn deliver<'a>(
        &'a self,
        _uri: &'a str,
        _out: &'a Outbound,
        _t: Duration,
    ) -> DeliveryFuture<'a> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Box::pin(std::future::ready(Ok(())))
    }
}

/// Answers the caller's subject headers and a view's Tenant.
struct Who;

impl Surface for Who {
    fn name(&self) -> &'static str {
        "who"
    }
    fn prefix(&self) -> &'static str {
        "/x/who"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        Router::new()
            .route(
                "/me",
                get(|caller: Caller| async move {
                    let subject: Vec<(String, String)> = caller
                        .subject()
                        .into_iter()
                        .map(|(k, v)| (k, String::from_utf8_lossy(&v).into_owned()))
                        .collect();
                    Json(json!(subject))
                }),
            )
            .route(
                "/views/{view}",
                get(move |Path(view): Path<String>| async move {
                    Json(json!(ops.view(&view).map(|v| v.tenant().to_owned())))
                }),
            )
    }
}

async fn call(app: &Router, req: Request<Body>) -> Result<(StatusCode, Value), String> {
    let res = app.clone().oneshot(req).await.map_err(|e| e.to_string())?;
    let status = res.status();
    let bytes = res
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes();
    Ok((
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    ))
}

fn json_req(method: Method, uri: &str, body: &Value) -> Result<Request<Body>, String> {
    let bytes = body.to_string();
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("content-length", bytes.len())
        .body(Body::from(bytes))
        .map_err(|e| e.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_kit_wires_a_surfaces_sink_engine_and_subject() {
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "x-subject");
    let delivered = Arc::new(AtomicUsize::new(0));
    let engine = Arc::new(Counting::default());
    let broker =
        TestBroker::with_sinks(vec![Box::new(Kit(delivered.clone()))], Some(engine.clone()))
            .await
            .expect("broker");
    let app = broker
        .serve_mounted(vec![antares_ops::mount(Who)])
        .expect("mounts");

    let view = json!({"id": "cars", "tenant": "city", "selector": ["Car"],
                      "rules": [{"types": ["Car"], "attributes": ["speed"]}],
                      "operations": ["query_entities"]});
    let (s, body) = call(
        &app,
        json_req(Method::PUT, "/q/views/cars", &view).expect("request"),
    )
    .await
    .expect("answered");
    assert_eq!(s, StatusCode::CREATED, "{body}");

    let me = Request::get("/x/who/me")
        .header("x-subject", "ada")
        .header("x-subject", "eve")
        .header("x-other", "no")
        .body(Body::empty())
        .expect("request");
    let (s, body) = call(&app, me).await.expect("answered");
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, json!([["x-subject", "ada"], ["x-subject", "eve"]]));
    for (view, tenant) in [("cars", json!("city")), ("nope", Value::Null)] {
        let req = Request::get(format!("/x/who/views/{view}"))
            .body(Body::empty())
            .expect("request");
        assert_eq!(
            call(&app, req).await.expect("answered"),
            (StatusCode::OK, tenant)
        );
    }

    // the sink takes its endpoints and is handed the notifications, after
    // the engine decided
    let sub = json!({"type": "Subscription", "entities": [{"type": "Car"}],
                     "notification": {"endpoint": {"uri": "urn:kit:1"}}});
    let (s, body) = call(
        &app,
        json_req(Method::POST, "/ngsi-ld/v1/subscriptions", &sub).expect("request"),
    )
    .await
    .expect("answered");
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let car = json!({"id": "urn:ngsi-ld:Car:k", "type": "Car", "speed": {"type": "Property", "value": 1}});
    let (s, body) = call(
        &app,
        json_req(Method::POST, "/ngsi-ld/v1/entities", &car).expect("request"),
    )
    .await
    .expect("answered");
    assert_eq!(s, StatusCode::CREATED, "{body}");
    for _ in 0..500 {
        if delivered.load(Ordering::Relaxed) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(delivered.load(Ordering::Relaxed), 1);
    assert!(engine.0.load(Ordering::Relaxed) > 0);
}
