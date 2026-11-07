// SPDX-License-Identifier: EUPL-1.2
//! G9 and the 10.3 mapping (surfaces-threat-model): what a surface gets back
//! from an operation that failed inside the broker carries no inner detail,
//! and a request that is not the operation it names is refused.

use antares_ops::test_kit::TestBroker;
use antares_ops::{
    Caller, OperationId, Ops, OpsRequest, Refusal, Surface, CONTEXT_DETAIL, TIMEOUT_DETAIL,
};
use axum::body::Body;
use axum::extract::Query;
use axum::http::{HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

/// Every event's fields, as `name=value` text: what the log would carry.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

struct Line<'a>(&'a mut String);

impl tracing::field::Visit for Line<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, "{}={value:?} ", field.name());
    }
}

impl tracing::Subscriber for Log {
    fn enabled(&self, _m: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _s: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _s: &tracing::span::Id, _v: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _s: &tracing::span::Id, _f: &tracing::span::Id) {}
    fn event(&self, e: &tracing::Event<'_>) {
        let mut line = String::new();
        e.record(&mut Line(&mut line));
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(line);
    }
    fn enter(&self, _s: &tracing::span::Id) {}
    fn exit(&self, _s: &tracing::span::Id) {}
}

/// `/x/probe?path=...&context=...` runs `QueryEntities` on `path`, with a
/// `Link` to `context` when one is given, and renders a refusal as the
/// NGSI-LD view would.
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
        Router::new().route(
            "/",
            get(move |caller: Caller, Query(q): Query<HashMap<String, String>>| async move {
                let path = q.get("path").cloned().unwrap_or_default();
                let mut req = OpsRequest::new(Method::GET, path);
                if let Some(ctx) = q.get("context") {
                    let link = format!(
                        "<{ctx}>; rel=\"http://www.w3.org/ns/json-ld#context\"; type=\"application/ld+json\""
                    );
                    match HeaderValue::from_str(&link) {
                        Ok(v) => req = req.header(HeaderName::from_static("link"), v),
                        Err(_) => return Refusal::Malformed("context").ngsi_ld(),
                    }
                }
                let op = if q.get("op").is_some_and(|o| o == "retrieve") {
                    OperationId::RetrieveEntity
                } else {
                    OperationId::QueryEntities
                };
                match ops.run(op, &caller, req).await {
                    Ok(r) => r,
                    Err(refusal) => refusal.ngsi_ld(),
                }
            }),
        )
    }
}

async fn get_body(app: &Router, uri: &str) -> (StatusCode, String, axum::http::HeaderMap) {
    let Ok(resp) = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap_or_default())
        .await;
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        status,
        String::from_utf8_lossy(&bytes).into_owned(),
        headers,
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

/// A `@context` nobody serves: the NGSI-LD answer is a 504 naming it
/// (`LdContextNotAvailable`); the surface answers the same status and
/// problem type (TS 104 176 6.3.2, T-3947) with a fixed detail, never the
/// URL or the fetch error. Current-thread, so the recording subscriber sees
/// every event of the run.
#[tokio::test(flavor = "current_thread")]
async fn an_inner_5xx_detail_never_reaches_the_surface() {
    let log = Log::default();
    let _guard = tracing::subscriber::set_default(log.clone());
    let app = TestBroker::new()
        .await
        .expect("broker")
        .serve(Probe)
        .expect("mounts");
    open(&app).await;
    let ctx = "http://127.0.0.1:9/nowhere-7f3a.jsonld";

    // the same request straight to the NGSI-LD API: a 5xx that names the context
    let link = format!(
        "<{ctx}>; rel=\"http://www.w3.org/ns/json-ld#context\"; type=\"application/ld+json\""
    );
    let direct = TestBroker::new()
        .await
        .expect("broker")
        .serve(Probe)
        .expect("mounts")
        .oneshot(
            Request::get("/ngsi-ld/v1/entities?type=Car")
                .header("Link", link)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("answered");
    assert!(
        direct.status().is_server_error(),
        "the broker fails inside: {}",
        direct.status()
    );
    let inner = direct.into_body().collect().await.expect("body").to_bytes();
    assert!(
        String::from_utf8_lossy(&inner).contains("nowhere-7f3a"),
        "the inner detail names it"
    );

    let (s, body, _) = get_body(
        &app,
        &format!("/x/probe?path=/ngsi-ld/v1/entities%3Ftype%3DCar&context={ctx}"),
    )
    .await;
    assert_eq!(s, StatusCode::GATEWAY_TIMEOUT, "{body}");
    assert!(
        body.contains("\"title\":\"LdContextNotAvailable\""),
        "{body}"
    );
    assert!(body.contains(CONTEXT_DETAIL), "{body}");
    assert!(
        !body.contains("nowhere-7f3a") && !body.contains("127.0.0.1"),
        "no inner detail: {body}"
    );
}

/// A surface reaches the NGSI-LD API only through the operation it names:
/// the admin routes, another surface, a traversal or another operation's
/// path are refused before the broker sees them.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_that_is_not_its_operation_is_refused() {
    let app = TestBroker::new()
        .await
        .expect("broker")
        .serve(Probe)
        .expect("mounts");
    open(&app).await;
    for path in [
        "/q/health",
        "/x/probe",
        "/ngsi-ld/v1/entities/../../q/health",
        "/ngsi-ld/v1/subscriptions",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:1",
    ] {
        let (s, body, headers) = get_body(&app, &format!("/x/probe?path={path}")).await;
        assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED, "{path}: {body}");
        assert!(headers.contains_key("allow"), "{path}");
        assert!(!body.contains(path), "the refusal echoes nothing: {body}");
    }
    let (s, body, _) = get_body(&app, "/x/probe?path=/ngsi-ld/v1/entities%3Ftype%3DCar").await;
    assert_eq!(s, StatusCode::OK, "{body}");
}

/// A target the operation's binding accepts but that is no URI (a space in
/// the id) is malformed, before the broker sees it.
#[tokio::test(flavor = "multi_thread")]
async fn a_target_that_is_no_uri_is_malformed() {
    let app = TestBroker::new()
        .await
        .expect("broker")
        .serve(Probe)
        .expect("mounts");
    open(&app).await;
    let (s, body, _) = get_body(&app, "/x/probe?op=retrieve&path=/ngsi-ld/v1/entities/a%20b").await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(!body.contains("a b"), "no echo: {body}");
}

/// `/q/health` lists the mounted surface by its name, its prefix and its
/// own version info.
#[tokio::test(flavor = "multi_thread")]
async fn the_health_report_names_the_surface() {
    let app = TestBroker::new()
        .await
        .expect("broker")
        .serve(Probe)
        .expect("mounts");
    let (s, body, _) = get_body(&app, "/q/health").await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let health: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(
        health["surfaces"]["probe"],
        serde_json::json!({
            "state": "closed",
            "reason": "no view is loaded",
            "info": null,
            "prefix": "/x/probe"
        }),
        "{body}"
    );
}

#[tokio::test]
async fn every_refusal_renders_with_its_status_and_no_echo() {
    for (r, status) in [
        (Refusal::NotFound, 404),
        (Refusal::Closed, 404),
        (
            Refusal::NotAllowed {
                allow: vec![Method::GET],
            },
            405,
        ),
        (Refusal::NotSupported, 422),
        (Refusal::Denied("no".into()), 403),
        (Refusal::Malformed("bad"), 400),
        (Refusal::Limit("too many"), 413),
        (
            Refusal::Inner {
                request_id: "ops-1".into(),
            },
            500,
        ),
        (Refusal::ContextNotAvailable, 504),
        (Refusal::StatementTimeout, 500),
        (
            Refusal::Overloaded {
                retry_after: Some(HeaderValue::from_static("6")),
            },
            503,
        ),
    ] {
        let rendered = r.ngsi_ld();
        assert_eq!(rendered.status(), status, "{r:?}");
        assert_eq!(r.status(), status, "{r:?}");
    }
    // T-5616: the gateway reads this detail to answer 504 query-timeout
    let timeout = Refusal::StatementTimeout.ngsi_ld();
    let body = timeout
        .into_body()
        .collect()
        .await
        .map(|b| String::from_utf8_lossy(&b.to_bytes()).into_owned())
        .unwrap_or_default();
    assert!(
        body.contains(&format!("\"detail\":\"{TIMEOUT_DETAIL}\"")),
        "{body}"
    );
    let not_allowed = Refusal::NotAllowed {
        allow: vec![Method::GET, Method::HEAD],
    }
    .ngsi_ld();
    assert_eq!(not_allowed.headers()["allow"], "GET, HEAD");
    let overloaded = Refusal::Overloaded {
        retry_after: Some(HeaderValue::from_static("6")),
    }
    .ngsi_ld();
    assert_eq!(overloaded.headers()["retry-after"], "6");
    assert_eq!(
        Refusal::Overloaded { retry_after: None }.ngsi_ld().status(),
        503
    );
}
