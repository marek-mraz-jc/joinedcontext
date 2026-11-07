// SPDX-License-Identifier: EUPL-1.2
//! One broker with the internal transport over [`SELF_URL`], and the
//! requests the tests send it. Each test binary sets the subject header
//! list before the broker first reads it.

use antares_ops::policy::PolicyEngine;
use antares_ops::test_kit::TestBroker;
use antares_ops::NotificationSink;
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

/// The self URL every test registers this broker by.
pub const SELF_URL: &str = "http://self.invalid";

/// A broker over the built-in store with the internal transport, asking
/// `engine` when there is one and delivering through `sinks` too; the
/// router serves `/q` too.
pub async fn broker(
    engine: Option<Arc<dyn PolicyEngine>>,
    sinks: Vec<Box<dyn NotificationSink>>,
) -> (TestBroker, axum::Router) {
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "X-Subject");
    let selfs = antares_icsr::SelfUrls::parse(SELF_URL).unwrap_or_default();
    let broker = TestBroker::with_sinks(sinks, engine)
        .await
        .and_then(|b| b.with_forward_transport(antares_icsr::transport(selfs)))
        .unwrap_or_else(|e| unreachable!("{e}"));
    let app = broker
        .serve_mounted(Vec::new())
        .unwrap_or_else(|e| unreachable!("{e}"));
    (broker, app)
}

/// An answer: status, `NGSILD-Warning` values, body.
pub struct Answer {
    pub status: u16,
    pub warnings: Vec<String>,
    pub body: Value,
}

/// `method path` in `tenant` (none: the default Tenant) as `subject` (none:
/// anonymous), with `doc` as its JSON payload.
pub async fn send(
    app: &axum::Router,
    method: &str,
    path: &str,
    tenant: Option<&str>,
    subject: Option<&str>,
    doc: Option<Value>,
) -> Answer {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = tenant {
        req = req.header("NGSILD-Tenant", t);
    }
    if let Some(s) = subject {
        req = req.header("X-Subject", s);
    }
    let req = match doc {
        Some(d) => {
            let text = d.to_string();
            req.header("Content-Type", "application/json")
                .header("Content-Length", text.len())
                .body(Body::from(text))
        }
        None => req.body(Body::empty()),
    }
    .unwrap_or_else(|e| unreachable!("{e}"));
    let answer = app
        .clone()
        .oneshot(req)
        .await
        .unwrap_or_else(|e| unreachable!("{e}"));
    let status = answer.status().as_u16();
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
    Answer {
        status,
        warnings,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

/// Write the grant `target` holds for `source`.
pub async fn grant(app: &axum::Router, target: &str, source: &str, extra: Value) {
    let mut doc = json!({"target": target, "source": source});
    if let (Some(d), Some(e)) = (doc.as_object_mut(), extra.as_object()) {
        d.extend(e.clone());
    }
    let path = format!("/q/grants/{target}/{source}");
    let a = send(app, "PUT", &path, None, None, Some(doc)).await;
    assert!(
        matches!(a.status, 201 | 204),
        "{path}: {} {}",
        a.status,
        a.body
    );
}

/// Register, in `tenant`, this broker by its self URL as the source of
/// `Car`s in Tenant `target` (none: the default Tenant), with the members
/// of `extra`; the answer.
pub async fn register(
    app: &axum::Router,
    tenant: &str,
    id: &str,
    target: Option<&str>,
    extra: Value,
) -> Answer {
    let mut doc = json!({
        "id": format!("urn:ngsi-ld:ContextSourceRegistration:{id}"),
        "type": "ContextSourceRegistration",
        "endpoint": SELF_URL,
        "information": [{"entities": [{"type": "Car"}]}],
    });
    if let (Some(d), Some(t)) = (doc.as_object_mut(), target) {
        d.insert("tenant".into(), json!(t));
    }
    if let (Some(d), Some(e)) = (doc.as_object_mut(), extra.as_object()) {
        d.extend(e.clone());
    }
    send(
        app,
        "POST",
        "/ngsi-ld/v1/csourceRegistrations",
        Some(tenant),
        None,
        Some(doc),
    )
    .await
}

/// Create Car `n` in `tenant` with `speed` and `owner`.
pub async fn car(app: &axum::Router, tenant: &str, n: u32) {
    let doc = json!({"id": format!("urn:ngsi-ld:Car:{tenant}-{n}"), "type": "Car",
                     "speed": {"type": "Property", "value": n},
                     "owner": {"type": "Property", "value": "someone"}});
    let a = send(
        app,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(tenant),
        None,
        Some(doc),
    )
    .await;
    assert_eq!(a.status, 201, "{}", a.body);
}

/// The ids of the Entities a query answered.
pub fn ids(body: &Value) -> Vec<String> {
    body.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| e["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Query the `Car`s of `tenant` as `subject`.
pub async fn cars(app: &axum::Router, tenant: &str, subject: Option<&str>) -> Answer {
    send(
        app,
        "GET",
        "/ngsi-ld/v1/entities?type=Car",
        Some(tenant),
        subject,
        None,
    )
    .await
}

/// Whether a warning of code `code` is among `warnings`.
pub fn warned(warnings: &[String], code: &str) -> bool {
    warnings.iter().any(|w| w.starts_with(code))
}
