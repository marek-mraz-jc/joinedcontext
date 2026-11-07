// SPDX-License-Identifier: EUPL-1.2
//! ADR-0025 rules 2 and 4, threats C1, C2, C3, C9 and C10: the target
//! Tenant's engine decides about the caller who asked, never about someone
//! a registration names; a refusal reaches the source as a 299 without its
//! text; the target's log names the registration and the source, never the
//! subject.

mod common;

use antares_ops::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use common::{broker, car, cars, grant, ids, register, send, warned};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, PoisonError};

/// Allows the subjects of `allowed` (Tenant, subject) and records every
/// decision; refuses the rest with a reason that must not travel.
#[derive(Default)]
struct Engine {
    allowed: Vec<(&'static str, &'static str)>,
    seen: Mutex<Vec<(String, String)>>,
}

const SECRET: &str = "tenant b keeps its reasons";

impl PolicyEngine for Engine {
    fn name(&self) -> &'static str {
        "icsr-test"
    }
    fn decide<'a>(&'a self, s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let who = s
            .headers
            .iter()
            .find(|(k, _)| k == "x-subject")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let tenant = s.tenant.as_str().to_owned();
        let ok = self.allowed.iter().any(|(t, w)| *t == tenant && *w == who);
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((tenant, who));
        Box::pin(std::future::ready(if ok {
            Decision::Allow
        } else {
            Decision::Deny(SECRET.into())
        }))
    }
    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

fn engine(allowed: Vec<(&'static str, &'static str)>) -> Arc<Engine> {
    Arc::new(Engine {
        allowed,
        seen: Mutex::default(),
    })
}

/// The decisions taken in Tenant `tenant`, by subject.
fn seen_in(e: &Engine, tenant: &str) -> Vec<String> {
    e.seen
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|(t, _)| t == tenant)
        .map(|(_, w)| w.clone())
        .collect()
}

/// Seeds `b` and `a`, the grant and the registration, as `admin` (allowed
/// everywhere).
async fn seeded(e: &Arc<Engine>) -> axum::Router {
    let (_b, app) = broker(Some(e.clone() as Arc<dyn PolicyEngine>), Vec::new()).await;
    for tenant in ["a", "b"] {
        let doc = json!({"id": format!("urn:ngsi-ld:Car:{tenant}-1"), "type": "Car",
                         "speed": {"type": "Property", "value": 1}});
        let made = send(
            &app,
            "POST",
            "/ngsi-ld/v1/entities",
            Some(tenant),
            Some("admin"),
            Some(doc),
        )
        .await;
        assert_eq!(made.status, 201, "{}", made.body);
    }
    grant(&app, "b", "a", json!({})).await;
    let reg = json!({"id": "urn:ngsi-ld:ContextSourceRegistration:r1", "type": "ContextSourceRegistration",
                     "endpoint": common::SELF_URL, "tenant": "b",
                     "information": [{"entities": [{"type": "Car"}]}],
                     "contextSourceInfo": [{"key": "X-Subject", "value": "mallory"}]});
    let stored = send(
        &app,
        "POST",
        "/ngsi-ld/v1/csourceRegistrations",
        Some("a"),
        Some("admin"),
        Some(reg),
    )
    .await;
    assert_eq!(stored.status, 201, "{}", stored.body);
    app
}

/// C2: Tenant b's engine is asked, under b, about alice, who asked a.
#[tokio::test]
async fn icsr_the_target_engine_is_asked_about_the_caller() {
    let e = engine(vec![
        ("a", "admin"),
        ("b", "admin"),
        ("a", "alice"),
        ("b", "alice"),
    ]);
    let app = seeded(&e).await;
    e.seen
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    let read = cars(&app, "a", Some("alice")).await;
    assert_eq!(read.status, 200, "{}", read.body);
    let mut found = ids(&read.body);
    found.sort();
    assert_eq!(found, ["urn:ngsi-ld:Car:a-1", "urn:ngsi-ld:Car:b-1"]);
    assert!(read.warnings.is_empty(), "{:?}", read.warnings);
    let in_b = seen_in(&e, "b");
    assert!(
        !in_b.is_empty() && in_b.iter().all(|w| w == "alice"),
        "{in_b:?}"
    );
}

/// C1, C2: the grant is there, b's engine refuses alice in b: she reads
/// a's Cars and none of b's, with a 299.
#[tokio::test]
async fn icsr_cross_tenant_read_without_an_engine_grant_is_refused() {
    let e = engine(vec![("a", "admin"), ("b", "admin"), ("a", "alice")]);
    let app = seeded(&e).await;
    let read = cars(&app, "a", Some("alice")).await;
    assert_eq!(read.status, 200, "{}", read.body);
    assert_eq!(ids(&read.body), ["urn:ngsi-ld:Car:a-1"]);
    assert!(warned(&read.warnings, "299"), "{:?}", read.warnings);
}

/// C3: the subject b's engine judges is the caller's, never a
/// `contextSourceInfo` value of the registration, and an anonymous caller
/// is judged as nobody.
#[tokio::test]
async fn icsr_the_subject_is_the_callers_and_never_comes_from_the_registration() {
    let e = engine(vec![
        ("a", "admin"),
        ("b", "admin"),
        ("a", ""),
        ("a", "alice"),
        ("b", "alice"),
    ]);
    let app = seeded(&e).await;
    e.seen
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    assert_eq!(cars(&app, "a", Some("alice")).await.status, 200);
    let anonymous = cars(&app, "a", None).await;
    assert_eq!(
        ids(&anonymous.body),
        ["urn:ngsi-ld:Car:a-1"],
        "{}",
        anonymous.body
    );
    let in_b = seen_in(&e, "b");
    assert_eq!(in_b, ["alice", ""], "mallory is never the subject");
}

/// C10: the source learns that b refused, never why.
#[tokio::test]
async fn icsr_a_target_error_reaches_the_source_without_its_detail() {
    let e = engine(vec![("a", "admin"), ("b", "admin"), ("a", "alice")]);
    let app = seeded(&e).await;
    let read = cars(&app, "a", Some("alice")).await;
    assert!(warned(&read.warnings, "299"), "{:?}", read.warnings);
    let all = format!("{} {:?}", read.body, read.warnings);
    assert!(!all.contains(SECRET), "{all}");
}

/// A registration without `tenant` reads the default Tenant (7.8.1), not
/// the Tenant the request ran in.
#[tokio::test]
async fn icsr_without_tenant_targets_the_default_tenant() {
    let (_b, app) = broker(None, Vec::new()).await;
    car(&app, "default", 1).await;
    car(&app, "a", 1).await;
    grant(&app, "default", "a", json!({})).await;
    let stored = register(&app, "a", "d", None, json!({})).await;
    assert_eq!(stored.status, 201, "{}", stored.body);
    let mut found = ids(&cars(&app, "a", None).await.body);
    found.sort();
    assert_eq!(found, ["urn:ngsi-ld:Car:a-1", "urn:ngsi-ld:Car:default-1"]);
}

/// A `tracing` writer into a shared buffer.
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

/// C9: b's log names the registration and the source Tenant of every
/// forward it served, and never the subject. The subscriber is the binary's
/// global one (a thread's own misses the callsites other tests registered
/// first), so every forward line of every test is checked.
#[tokio::test]
async fn icsr_a_forward_is_logged_in_the_target_with_the_registration_id() {
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
    let e = engine(vec![
        ("a", "admin"),
        ("b", "admin"),
        ("a", "alice"),
        ("b", "alice"),
    ]);
    let app = seeded(&e).await;
    assert_eq!(cars(&app, "a", Some("alice")).await.status, 200);
    let text =
        String::from_utf8_lossy(&log.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("internal context source forward"))
        .collect();
    assert!(
        lines.iter().any(|l| {
            l.contains(r#"registration="urn:ngsi-ld:ContextSourceRegistration:r1""#)
                && l.contains(r#"source_tenant="a""#)
                && l.contains(r#"target_tenant="b""#)
        }),
        "{text}"
    );
    for line in &lines {
        assert!(
            !line.contains("alice") && !line.contains("mallory"),
            "{line}"
        );
    }
}
