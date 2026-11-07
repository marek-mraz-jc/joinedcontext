// SPDX-License-Identifier: EUPL-1.2
//! ADR-0023 rule 10: a view written on one pod opens, changes and closes on
//! every other pod over the same store within the reload interval, without
//! a restart. Its own binary: it sets the interval before any table loads.

use antares_ops::test_kit::TestBroker;
use antares_ops::{Caller, OperationId, Ops, OpsRequest, Surface};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::time::Duration;
use tower::ServiceExt;

struct Probe;

impl Surface for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }
    fn prefix(&self) -> &'static str {
        "/x/probe"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, ops: Ops) -> Router {
        Router::new().route(
            "/types",
            get(move |caller: Caller| async move {
                let req = OpsRequest::new(Method::GET, "/ngsi-ld/v1/types");
                match ops
                    .run(OperationId::RetrieveEntityTypes, &caller, req)
                    .await
                {
                    Ok(r) => r,
                    Err(refusal) => refusal.ngsi_ld(),
                }
            }),
        )
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    let bytes = body.map(Value::to_string).unwrap_or_default();
    let mut req = Request::builder().method(method).uri(uri);
    if body.is_some() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", bytes.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(bytes)).unwrap_or_default())
        .await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Polls `app` until the probe answers `want`, for at most two seconds (20
/// reload intervals).
async fn until(app: &Router, want: StatusCode) -> bool {
    for _ in 0..100 {
        if send(app, Method::GET, "/x/probe/types", None).await.0 == want {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn views_reload_without_restart() {
    std::env::set_var(antares_ops::views::RELOAD_ENV, "100");
    let a = TestBroker::new().await.expect("pod a");
    let b = a.sibling().await.expect("pod b");
    let (app_a, app_b) = (
        a.serve(Probe).expect("mounts"),
        b.serve(Probe).expect("mounts"),
    );
    assert_eq!(
        send(&app_b, Method::GET, "/x/probe/types", None).await.0,
        StatusCode::NOT_FOUND
    );

    let mut v = json!({
        "id": "cars", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["retrieve_entity_types"]
    });
    let (s, _) = send(&app_a, Method::PUT, "/q/views/cars", Some(&v)).await;
    assert_eq!(s, StatusCode::CREATED);
    assert!(
        until(&app_b, StatusCode::OK).await,
        "pod b opened on pod a's write"
    );

    v["operations"] = json!(["retrieve_entity_types", "query_entities"]);
    let (s, _) = send(&app_a, Method::PUT, "/q/views/cars", Some(&v)).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "a replace");
    let mut seen = Value::Null;
    for _ in 0..100 {
        seen = send(&app_b, Method::GET, "/q/health", None)
            .await
            .1
            .pointer("/views/cars")
            .cloned()
            .unwrap_or_default();
        if seen.get("operations") == Some(&json!(2)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(seen["operations"], 2, "pod b took the change: {seen}");

    let (s, _) = send(&app_a, Method::DELETE, "/q/views/cars", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(
        until(&app_b, StatusCode::NOT_FOUND).await,
        "pod b closed on pod a's delete"
    );
}

/// `/q/health` `viewsRevision` of `app`.
async fn revision(app: &Router) -> Value {
    send(app, Method::GET, "/q/health", None)
        .await
        .1
        .get("viewsRevision")
        .cloned()
        .unwrap_or_default()
}

/// Polls `app` until its revision is `want`, for at most two seconds.
async fn until_revision(app: &Router, want: &Value) -> bool {
    for _ in 0..100 {
        if revision(app).await == *want {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// Threat V26: two pods over one store report one revision once both have
/// reloaded, a write changes it on the writer at once and on the other pod
/// after its reload, and a reload that finds nothing new keeps it.
#[tokio::test(flavor = "multi_thread")]
async fn health_reports_the_view_table_revision() {
    std::env::set_var(antares_ops::views::RELOAD_ENV, "100");
    let a = TestBroker::new().await.expect("pod a");
    let b = a.sibling().await.expect("pod b");
    let (app_a, app_b) = (
        a.serve(Probe).expect("mounts"),
        b.serve(Probe).expect("mounts"),
    );
    let empty = revision(&app_a).await;
    assert!(
        empty.as_str().is_some_and(|r| r.len() == 16),
        "sixteen hex digits: {empty}"
    );
    assert!(
        until_revision(&app_b, &empty).await,
        "one store, one revision"
    );

    let v = json!({
        "id": "cars", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["retrieve_entity_types"]
    });
    let (s, _) = send(&app_a, Method::PUT, "/q/views/cars", Some(&v)).await;
    assert_eq!(s, StatusCode::CREATED);
    let written = revision(&app_a).await;
    assert_ne!(written, empty, "the writer reports its write at once");
    assert!(
        until_revision(&app_b, &written).await,
        "pod b reports pod a's write after its reload"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(revision(&app_a).await, written, "an unchanged reload");
    assert_eq!(revision(&app_b).await, written);
}
