// SPDX-License-Identifier: EUPL-1.2
//! The loader's log (ADR-0023 rules 10 and 11): an operator reads each note
//! of a view that loaded and why a stored view is not served, once per
//! change. Its own binary: tracing caches a callsite's interest, and another
//! test hitting the same callsite while this one's subscriber registers
//! would hide the event from it.

use antares_ops::test_kit::TestBroker;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

struct Nothing;

impl antares_ops::Surface for Nothing {
    fn name(&self) -> &'static str {
        "nothing"
    }
    fn prefix(&self) -> &'static str {
        "/x/nothing"
    }
    fn version_info(&self) -> Value {
        Value::Null
    }
    fn router(&self, _ops: antares_ops::Ops) -> Router {
        Router::new()
    }
}

async fn put(app: &Router, id: &str, v: &Value) -> StatusCode {
    let bytes = v.to_string();
    let req = Request::put(format!("/q/views/{id}"))
        .header("Content-Type", "application/json")
        .header("Content-Length", bytes.len())
        .body(Body::from(bytes))
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    resp.status()
}

fn view(id: &str) -> Value {
    json!({
        "id": id, "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": ["query_entities"]
    })
}

/// Every event's fields, as `name=value` text.
#[derive(Clone, Default)]
struct Log(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

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

/// An operator reads in the log what the loader found: each note of a view
/// that loaded, and why a stored view is not served, once per change.
#[tokio::test(flavor = "current_thread")]
async fn the_loader_logs_its_notes_and_its_refusals_once() {
    let log = Log::default();
    let _guard = tracing::subscriber::set_default(log.clone());
    let b = TestBroker::new().await.expect("broker");
    let app = b.serve(Nothing).expect("mounts");
    let mut v = view("zone");
    v["rules"] = json!([{"types": ["Car"], "q": "status==\"parked\"", "attributes": ["speed"]}]);
    assert_eq!(put(&app, "zone", &v).await, StatusCode::CREATED);
    let mut bad = view("bad");
    bad["q"] = json!("speed>");
    b.store_view_doc("bad", bad).await.expect("stored");
    b.store_view_doc("bad", json!({"id": "bad", "q": "speed>"}))
        .await
        .expect("stored");

    let lines = log.0.lock().expect("log").clone();
    let notes = lines.iter().filter(|l| l.contains("view=\"zone\"")).count();
    assert_eq!(
        notes, 1,
        "one note, logged once over three reloads: {lines:?}"
    );
    let refusals = lines.iter().filter(|l| l.contains("view=\"bad\"")).count();
    assert_eq!(refusals, 2, "each changed document once: {lines:?}");
}
