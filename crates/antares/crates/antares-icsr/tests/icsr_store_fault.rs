// SPDX-License-Identifier: EUPL-1.2
//! ADR-0025 rule 5: a grant the store cannot read grants nothing. While the
//! grant store fails, a registration naming this broker is not admitted, a
//! forward is refused (code 299 to the merge), a Subscription copy's
//! notification is not delivered, and the admin routes answer a 500 that
//! names no store detail. Its own binary: it sets the public URL the copies
//! notify under before the broker first reads it.

#[path = "../../antares-ops/tests/support/fault_store.rs"]
mod fault_store;

use antares_ops::test_kit::TestBroker;
use antares_ops::{DeliveryFuture, NgsiError, NotificationSink, Outbound};
use antares_store::Kind;
use axum::body::Body;
use axum::http::Request;
use fault_store::FaultStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tower::ServiceExt;

const SELF_URL: &str = "http://self.invalid";

/// Takes `urn:kit:` endpoints and keeps what it was handed.
#[derive(Clone, Default)]
struct Kit(Arc<Mutex<Vec<Outbound>>>);

impl NotificationSink for Kit {
    fn schemes(&self) -> &'static [&'static str] {
        &["urn"]
    }
    fn parse_endpoint(&self, _uri: &str, _info: &[(&str, &str)]) -> Result<(), NgsiError> {
        Ok(())
    }
    fn network(&self) -> bool {
        false
    }
    fn deliver<'a>(&'a self, _uri: &'a str, out: &'a Outbound, _t: Duration) -> DeliveryFuture<'a> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(out.clone());
        Box::pin(std::future::ready(Ok(())))
    }
}

/// The binary's log, one global subscriber.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// What has been logged so far.
fn logged() -> String {
    static LOG: std::sync::OnceLock<Log> = std::sync::OnceLock::new();
    let log = LOG.get_or_init(|| {
        let log = Log::default();
        let writer = log.clone();
        let _ = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .try_init();
        log
    });
    String::from_utf8_lossy(&log.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
}

/// `method path` in `tenant` with `doc`: status, `NGSILD-Warning`s, body.
async fn send(
    app: &axum::Router,
    method: &str,
    path: &str,
    tenant: Option<&str>,
    doc: Option<Value>,
) -> (u16, Vec<String>, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = tenant {
        req = req.header("NGSILD-Tenant", t);
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
    (
        status,
        warnings,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Registration `id` in a of b's `Car`s at this broker.
fn registration(id: &str) -> Value {
    json!({
        "id": format!("urn:ngsi-ld:ContextSourceRegistration:{id}"),
        "type": "ContextSourceRegistration",
        "endpoint": SELF_URL,
        "tenant": "b",
        "information": [{"entities": [{"type": "Car"}]}],
    })
}

/// The notification status of b's copies of a's Subscription.
async fn copy_status(app: &axum::Router) -> Vec<String> {
    let (_, _, body) = send(app, "GET", "/ngsi-ld/v1/subscriptions", Some("b"), None).await;
    body.as_array()
        .map(|a| {
            a.iter()
                .map(|s| {
                    s.pointer("/notification/status")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A broker over the fault store with b's grant for a, a's registration
/// of b's `Car`s, b's Car 1, and a's Subscription copied into b.
async fn seeded() -> (TestBroker, axum::Router, Arc<FaultStore>) {
    std::env::set_var("ANTARES_PUBLIC_URL", SELF_URL);
    logged();
    let (state, fault) = FaultStore::state("antares-test");
    let state = state.with_sink(Box::new(Kit::default()));
    let selfs = antares_icsr::SelfUrls::parse(SELF_URL).unwrap_or_default();
    let broker: TestBroker = TestBroker::over(state)
        .await
        .and_then(|b| b.with_forward_transport(antares_icsr::transport(selfs)))
        .unwrap_or_else(|e| unreachable!("{e}"));
    let app = broker
        .serve_mounted(Vec::new())
        .unwrap_or_else(|e| unreachable!("{e}"));

    let put = send(
        &app,
        "PUT",
        "/q/grants/b/a",
        None,
        Some(json!({"target": "b", "source": "a"})),
    )
    .await;
    assert_eq!(put.0, 201, "{put:?}");
    let made = send(
        &app,
        "POST",
        "/ngsi-ld/v1/csourceRegistrations",
        Some("a"),
        Some(registration("r1")),
    )
    .await;
    assert_eq!(made.0, 201, "{}", made.2);
    let car = json!({"id": "urn:ngsi-ld:Car:b-1", "type": "Car",
                     "speed": {"type": "Property", "value": 1}});
    let created = send(&app, "POST", "/ngsi-ld/v1/entities", Some("b"), Some(car)).await;
    assert_eq!(created.0, 201, "{}", created.2);
    let sub = json!({"id": "urn:ngsi-ld:Subscription:s1", "type": "Subscription",
                     "entities": [{"type": "Car"}],
                     "notification": {"endpoint": {"uri": "urn:kit:a"}}});
    let subscribed = send(
        &app,
        "POST",
        "/ngsi-ld/v1/subscriptions",
        Some("a"),
        Some(sub),
    )
    .await;
    assert_eq!(subscribed.0, 201, "{}", subscribed.2);
    let mut copied = false;
    for _ in 0..50 {
        if !copy_status(&app).await.is_empty() {
            copied = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(copied, "a copy of a's Subscription is made in b");

    (broker, app, fault)
}

#[tokio::test]
async fn icsr_a_grant_the_store_cannot_read_grants_nothing() {
    let (_broker, app, fault) = seeded().await;
    // From here every read of a stored grant fails.
    fault.fail(|method, tenant, kind| {
        method == "get" && kind == Some(Kind::View) && tenant == Some("antares-grants")
    });

    // the admin route: a 500 naming no store detail
    let (status, _, body) = send(&app, "GET", "/q/grants/b/a", None, None).await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(
        body.get("detail"),
        Some(&json!("the grant store failed")),
        "{body}"
    );
    assert!(!body.to_string().contains("injected"), "{body}");

    // admission: a registration naming this broker is refused
    let (status, _, body) = send(
        &app,
        "POST",
        "/ngsi-ld/v1/csourceRegistrations",
        Some("a"),
        Some(registration("r2")),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    assert!(
        body.to_string()
            .contains("the export grant could not be read"),
        "{body}"
    );

    // a forward: refused, so a's query sees none of b's Cars and a 299
    let (status, warnings, body) = send(
        &app,
        "GET",
        "/ngsi-ld/v1/entities?type=Car",
        Some("a"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, json!([]), "nothing of b without a readable grant");
    assert!(
        warnings.iter().any(|w| w.starts_with("299")),
        "{warnings:?}"
    );

    // a copy's notification: not delivered, the attempt is a failed one
    let patch = json!({"speed": {"type": "Property", "value": 2}});
    let (status, _, body) = send(
        &app,
        "PATCH",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:b-1/attrs",
        Some("b"),
        Some(patch),
    )
    .await;
    assert_eq!(status, 204, "{body}");
    let mut failed = false;
    for _ in 0..50 {
        if copy_status(&app).await.iter().any(|s| s == "failed") {
            failed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        failed,
        "the copy's attempt failed: {:?}",
        copy_status(&app).await
    );
    let log = logged();
    assert!(
        log.matches("export grants: the store failed").count() >= 3,
        "the route, the admission, the forward and the notification log it: {log}"
    );
}
