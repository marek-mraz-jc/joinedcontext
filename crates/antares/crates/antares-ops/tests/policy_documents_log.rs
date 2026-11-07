// SPDX-License-Identifier: EUPL-1.2
//! A rule document says who may read what: neither it nor the engine's
//! reason for refusing it reaches the log (ADR-0020 amendment), while the
//! refusal itself is logged by tenant and revision. Its own binary, for the
//! reason `views_log.rs` gives.

use antares_api::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

/// Takes every document but one that says `"refuse": true`, and quotes the
/// document in its reason.
struct Quoting;

impl PolicyEngine for Quoting {
    fn name(&self) -> &'static str {
        "quoting"
    }
    fn decide<'a>(&'a self, _s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(Decision::Allow))
    }
    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
    fn loads_documents(&self) -> bool {
        true
    }
    fn load(&self, _tenant: &str, document: &Value) -> Result<(), String> {
        if document.get("refuse") == Some(&json!(true)) {
            Err(format!("refused: {document}"))
        } else if document.is_null() {
            Err("nothing to drop".into())
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Default)]
struct Log(Arc<std::sync::Mutex<Vec<String>>>);

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

async fn send(app: &Router, method: Method, uri: &str, body: Option<&Value>) -> StatusCode {
    let bytes = body.map(Value::to_string).unwrap_or_default();
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Type", "application/json")
        .header("Content-Length", bytes.len())
        .body(Body::from(bytes))
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    resp.status()
}

#[tokio::test(flavor = "current_thread")]
async fn no_document_and_no_reason_reaches_the_log() {
    let log = Log::default();
    let _guard = tracing::subscriber::set_default(log.clone());
    let mut state = AppState::new("antares-test".into());
    antares_api::wire(&mut state).await;
    let state = state.with_policy(Arc::new(Quoting));
    let reload = antares_ops::views::install(&state)
        .await
        .expect("installed");
    let app = antares_api::router(
        state
            .clone()
            .with_surfaces(vec![Box::new(antares_api::Admin)])
            .expect("admin"),
    );
    let taken = json!({"secret": "sk-taken-document"});
    let refused = json!({"refuse": true, "secret": "sk-refused-document"});
    assert_eq!(
        send(&app, Method::PUT, "/q/policy/city", Some(&taken)).await,
        StatusCode::CREATED
    );
    assert_eq!(
        send(&app, Method::PUT, "/q/policy/city", Some(&refused)).await,
        StatusCode::BAD_REQUEST
    );
    state
        .put_policy_doc(
            "city",
            json!({"tenant": "city", "revision": 9, "document": refused}),
        )
        .await
        .expect("stored");
    reload.now().await.expect("reload");
    assert_eq!(
        send(&app, Method::DELETE, "/q/policy/city", None).await,
        StatusCode::NO_CONTENT
    );

    let lines = log.0.lock().expect("log").clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("tenant=\"city\"") && l.contains("revision=9")),
        "the refusal is logged by tenant and revision: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("refused a drop")),
        "a failed drop is logged: {lines:?}"
    );
    for line in &lines {
        assert!(
            !line.contains("sk-") && !line.contains("refused: "),
            "a document or a reason in the log: {line}"
        );
    }
}
