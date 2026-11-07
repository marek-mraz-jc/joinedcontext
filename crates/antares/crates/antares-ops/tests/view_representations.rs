// SPDX-License-Identifier: EUPL-1.2
//! What a surface reads of the request and the view for a domain standard (T-4901): the view's
//! own `representations` member by surface name, and whether the request carries the broker's
//! admin bearer token (false on a broker that has none), and the view compilation it reads
//! through (T-5909).

use antares_ops::test_kit::{AdminAuth, TestBroker};
use antares_ops::{Ops, Surface};
use axum::body::Body;
use axum::extract::{Path, Request};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::{Mutex, PoisonError, Weak};
use tower::ServiceExt;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// `/x/probe/{view}` answers `[admin?, the view's ical representation]`; `/x/probe/gen/{view}`
/// whether the view's compilation is the one the previous call kept.
struct Probe;

type Generation = Weak<dyn std::any::Any + Send + Sync>;

static KEPT: Mutex<Option<Generation>> = Mutex::new(None);

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
        let gen_ops = ops.clone();
        Router::new()
            .route(
                "/{view}",
                get(move |Path(id): Path<String>, req: Request| async move {
                    let admin = ops.admits_admin(req.headers());
                    let ical = ops
                        .view(&id)
                        .and_then(|v| v.representation("ical").cloned());
                    axum::Json(json!([admin, ical]))
                }),
            )
            .route(
                "/gen/{view}",
                get(move |Path(id): Path<String>| async move {
                    let now = gen_ops.view(&id).map(|v| v.generation());
                    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
                    let same = kept.as_ref().zip(now.as_ref()).map(|(a, b)| a.ptr_eq(b));
                    *kept = now;
                    axum::Json(json!(same))
                }),
            )
    }
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: &str,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    if !body.is_empty() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len());
    }
    let Ok(resp) = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap_or_default())
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

fn view(id: &str, representations: Option<Value>) -> String {
    let mut v = json!({"id": id, "tenant": "city", "selector": ["Event"],
        "operations": ["query_entities"],
        "rules": [{"types": ["Event"], "attributes": ["name"]}]});
    if let (Some(r), Some(doc)) = (representations, v.as_object_mut()) {
        doc.insert("representations".into(), r);
    }
    v.to_string()
}

#[tokio::test]
async fn a_surface_reads_its_representation_and_the_admin_token() -> Result<(), String> {
    let broker = TestBroker::new()
        .await?
        .with_admin_auth(AdminAuth::new(TOKEN, true)?);
    let app = broker.serve(Probe)?;
    let ical = json!([{"name": "events", "sources": [{"preset": "schema-org"}]}]);
    for (id, r) in [("mapped", Some(json!({"ical": ical}))), ("plain", None)] {
        let (status, _) = send(
            &app,
            "PUT",
            &format!("/q/views/{id}"),
            &view(id, r),
            Some(TOKEN),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{id}");
    }
    let (_, seen) = send(&app, "GET", "/x/probe/mapped", "", Some(TOKEN)).await;
    assert_eq!(seen, json!([true, ical]));
    let (_, seen) = send(
        &app,
        "GET",
        "/x/probe/plain",
        "",
        Some("x".repeat(32).as_str()),
    )
    .await;
    assert_eq!(seen, json!([false, null]));
    let (_, seen) = send(&app, "GET", "/x/probe/plain", "", None).await;
    assert_eq!(seen, json!([false, null]));
    Ok(())
}

#[tokio::test]
async fn a_broker_without_an_admin_token_admits_nobody_to_a_surfaces_internal_route(
) -> Result<(), String> {
    let broker = TestBroker::new().await?;
    let app = broker.serve(Probe)?;
    let (status, _) = send(&app, "PUT", "/q/views/plain", &view("plain", None), None).await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, seen) = send(&app, "GET", "/x/probe/plain", "", Some(TOKEN)).await;
    assert_eq!(seen, json!([false, null]));
    Ok(())
}

#[tokio::test]
async fn a_view_compilation_is_the_same_until_the_view_changes() -> Result<(), String> {
    let broker = TestBroker::new().await?;
    let app = broker.serve(Probe)?;
    let (status, _) = send(&app, "PUT", "/q/views/gen", &view("gen", None), None).await;
    assert_eq!(status, StatusCode::CREATED);
    send(&app, "GET", "/x/probe/gen/gen", "", None).await;
    let (_, same) = send(&app, "GET", "/x/probe/gen/gen", "", None).await;
    assert_eq!(same, json!(true), "two reads of one compilation");
    let changed = view("gen", Some(json!({"ical": []})));
    let (status, _) = send(&app, "PUT", "/q/views/gen", &changed, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, same) = send(&app, "GET", "/x/probe/gen/gen", "", None).await;
    assert_eq!(same, json!(false), "a changed view is a new compilation");
    Ok(())
}
