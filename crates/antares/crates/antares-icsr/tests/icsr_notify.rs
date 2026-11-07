// SPDX-License-Identifier: EUPL-1.2
//! ADR-0025 rule 7, threats C1 and C7: the notifications of a 5.8.1.4
//! Subscription copy made in process reach the receiving Tenant in process,
//! only while the copy's Tenant grants it, and carry no subject. Its own
//! binary: it sets the public URL the copies notify under before the broker
//! first reads it.

mod common;

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_ops::test_kit::TestBroker;
use antares_ops::{DeliveryFuture, NgsiError, NotificationSink, Outbound};
use common::{broker, car, cars, grant, ids, register, send, warned, SELF_URL};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// Takes `urn:kit:` endpoints and keeps what it was handed.
#[derive(Clone, Default)]
struct Kit(Arc<Mutex<Vec<Outbound>>>);

impl Kit {
    fn got(&self) -> Vec<Outbound> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

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
    fn deliver<'a>(&'a self, _uri: &'a str, out: &'a Outbound, _t: Duration) -> DeliveryFuture<'a> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(out.clone());
        Box::pin(std::future::ready(Ok(())))
    }
}

/// Allows everything and records the subject of every decision and every
/// notification it is shown, by Tenant.
#[derive(Default)]
struct Engine(Mutex<Vec<(String, String)>>);

impl Engine {
    fn note(&self, s: &Subject) {
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
    }
    fn seen(&self) -> Vec<(String, String)> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl PolicyEngine for Engine {
    fn name(&self) -> &'static str {
        "icsr-notify-test"
    }
    fn decide<'a>(&'a self, s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        self.note(s);
        Box::pin(std::future::ready(Decision::Allow))
    }
    fn pre_notify(&self, s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        self.note(s);
        NotifyDecision::Deliver
    }
}

/// The binary's log: one global subscriber (a thread's own misses the
/// callsites other tests registered first).
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

/// A broker whose copies notify under [`SELF_URL`], delivering to `kit`,
/// with b's grant for a and a's registration of b's `Car`s stored.
async fn seeded(kit: &Kit, engine: Option<Arc<dyn PolicyEngine>>) -> (TestBroker, axum::Router) {
    std::env::set_var("ANTARES_PUBLIC_URL", SELF_URL);
    logged();
    let (b, app) = broker(engine, vec![Box::new(kit.clone())]).await;
    grant(&app, "b", "a", json!({})).await;
    let stored = register(&app, "a", "r1", Some("b"), json!({})).await;
    assert_eq!(stored.status, 201, "{}", stored.body);
    (b, app)
}

/// Subscribe to `Car`s in a as `subject`, notifying the kit.
async fn subscribe(app: &axum::Router, subject: Option<&str>) {
    let sub = json!({"id": "urn:ngsi-ld:Subscription:s1", "type": "Subscription",
                     "entities": [{"type": "Car"}],
                     "notification": {"endpoint": {"uri": "urn:kit:a"}}});
    let made = send(
        app,
        "POST",
        "/ngsi-ld/v1/subscriptions",
        Some("a"),
        subject,
        Some(sub),
    )
    .await;
    assert_eq!(made.status, 201, "{}", made.body);
    // 5.8.1.4: the copy is made in b after the answer
    for _ in 0..50 {
        if !copy_status(app).await.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    unreachable!("no copy of a's Subscription was made in b");
}

/// Whether `done` holds within five seconds.
async fn eventually(mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..50 {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    done()
}

/// Set the speed of b's Car 1 to `value` as `subject`: the copy notifies
/// on changed Attributes.
async fn speed(app: &axum::Router, subject: Option<&str>, value: u32) {
    let set = send(
        app,
        "PATCH",
        "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:b-1/attrs",
        Some("b"),
        subject,
        Some(json!({"speed": {"type": "Property", "value": value}})),
    )
    .await;
    assert_eq!(set.status, 204, "{}", set.body);
}

/// Whether the kit was handed b's Car 1 at speed `value`.
fn delivered(kit: &Kit, value: u32) -> bool {
    kit.got().iter().any(|o| {
        o.body
            .get("data")
            .and_then(Value::as_array)
            .is_some_and(|d| {
                d.iter().any(|e| {
                    e.get("id") == Some(&json!("urn:ngsi-ld:Car:b-1"))
                        && e.pointer("/speed/value") == Some(&json!(value))
                })
            })
    })
}

/// The notification status of each of b's copies of a's Subscription
/// (empty before the first attempt).
async fn copy_status(app: &axum::Router) -> Vec<String> {
    let listed = send(
        app,
        "GET",
        "/ngsi-ld/v1/subscriptions",
        Some("b"),
        None,
        None,
    )
    .await;
    listed
        .body
        .as_array()
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

/// C7: a change in b reaches a's subscriber through the copy made in
/// process; once b's grant is removed the forwards stop, the next change
/// reaches nothing and the copy's attempt is a failed one; a grant written
/// again opens it.
#[tokio::test]
async fn icsr_a_revoked_grant_stops_notifications() {
    let kit = Kit::default();
    let (_b, app) = seeded(&kit, None).await;
    car(&app, "b", 1).await;
    subscribe(&app, None).await;
    speed(&app, None, 101).await;
    assert!(
        eventually(|| delivered(&kit, 101)).await,
        "b's change reaches a"
    );

    assert_eq!(
        ids(&cars(&app, "a", None).await.body),
        ["urn:ngsi-ld:Car:b-1"]
    );

    let removed = send(&app, "DELETE", "/q/grants/b/a", None, None, None).await;
    assert_eq!(removed.status, 204, "{}", removed.body);
    let read = cars(&app, "a", None).await;
    assert!(ids(&read.body).is_empty(), "{}", read.body);
    assert!(warned(&read.warnings, "299"), "{:?}", read.warnings);
    speed(&app, None, 102).await;
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
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !delivered(&kit, 102),
        "nothing of b reaches a without a grant"
    );
    assert!(
        logged().contains("an internal notification without a grant for it is not delivered"),
        "b logs the refusal"
    );

    grant(&app, "b", "a", json!({})).await;
    speed(&app, None, 103).await;
    assert!(
        eventually(|| delivered(&kit, 103)).await,
        "a grant written again opens it"
    );
}

/// C1: bob's change in b reaches alice's Subscription in a, and bob's
/// identity stays in b: no decision in a and no notification is taken
/// about him, and the notification a's subscriber gets does not name him.
#[tokio::test]
async fn icsr_a_notification_carries_no_subject() {
    let kit = Kit::default();
    let engine = Arc::new(Engine::default());
    let (_b, app) = seeded(&kit, Some(engine.clone() as Arc<dyn PolicyEngine>)).await;
    car(&app, "b", 1).await;
    subscribe(&app, Some("alice")).await;
    speed(&app, Some("bob"), 101).await;
    assert!(
        eventually(|| delivered(&kit, 101)).await,
        "b's change reaches a"
    );

    // C9: b's log names the copy and both Tenants, never a subject.
    let log = logged();
    let line = log
        .lines()
        .find(|l| {
            l.contains(": internal notification ")
                && l.contains("urn:ngsi-ld:Subscription:distsub:")
        })
        .unwrap_or_default();
    assert!(
        line.contains(r#"source_tenant="a""#) && line.contains(r#"target_tenant="b""#),
        "{log}"
    );
    assert!(!log.contains("bob") && !log.contains("alice"), "{log}");

    let seen = engine.seen();
    let bob: Vec<_> = seen.iter().filter(|(_, who)| who == "bob").collect();
    assert_eq!(
        bob,
        [&("b".to_owned(), "bob".to_owned())],
        "bob is judged once, for his own write in b: {seen:?}"
    );
    for out in kit.got() {
        assert!(
            !format!("{:?} {}", out.receiver_info, out.body).contains("bob"),
            "{:?}",
            out.receiver_info
        );
    }
}
