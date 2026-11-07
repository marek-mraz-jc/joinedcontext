// SPDX-License-Identifier: EUPL-1.2
//! A write condition on the temporal writes 5.6.12 to 5.6.15 (T-3668): the
//! engine answers `Decision::AllowIf(filter)`, and the temporal store
//! applies the write only if the evolution's latest instances match the
//! filter, judged inside its own read-modify-write. An evolution it does not
//! select answers as an absent one; a value the Entity held once does not
//! select it; 5.6.16 still answers a condition as a deny.
#![cfg(feature = "test-kit")]

use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Subject,
    CONDITION_NOT_FORWARDED, WRITE_BY_ID,
};
use antares_api::AppState;
use antares_model::{NgsiError, TenantId};
use antares_store::{filter, MutateFn, TemporalDriver};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context/";
const IN_A: &str = "urn:ngsi-ld:Vehicle:in-a";
const IN_B: &str = "urn:ngsi-ld:Vehicle:in-b";
const ABSENT: &str = "urn:ngsi-ld:Vehicle:absent";

/// Answers every write by id with `AllowIf(zone=="A")`, else `Allow`.
struct Conditioned;

impl PolicyEngine for Conditioned {
    fn name(&self) -> &str {
        "conditioned"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(if WRITE_BY_ID.contains(&op.clause) {
            Decision::AllowIf(zone_a())
        } else {
            Decision::Allow
        }))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// Answers `AllowIf(zone=="A")` on `self.0` and `Allow` everywhere else.
struct On(&'static str);

impl PolicyEngine for On {
    fn name(&self) -> &str {
        "on"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(if op.clause == self.0 {
            Decision::AllowIf(zone_a())
        } else {
            Decision::Allow
        }))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

fn zone_a() -> Filter {
    Filter {
        q: Some(Box::new(antares_ql::parse_q("zone==\"A\"").expect("q"))),
        ..Filter::default()
    }
}

fn zone(value: &str, at: &str) -> Value {
    json!({"type": "Property", "value": value, "observedAt": at})
}

/// IN_A was in zone B and is now in A; IN_B was in A and is now in B. The
/// instances are stored out of time order, so the latest is not the last.
fn evolution(id: &str, now: &str, then: &str) -> Value {
    json!({"id": id, "type": "Vehicle",
           "zone": [zone(now, "2026-01-01T00:00:02Z"), zone(then, "2026-01-01T00:00:01Z")],
           "speed": [{"type": "Property", "value": 1, "observedAt": "2026-01-01T00:00:01Z"}]})
}

async fn state(engine: Arc<dyn PolicyEngine>) -> AppState {
    let mut st = AppState::new("me".into());
    antares_api::wire(&mut st).await;
    for doc in [evolution(IN_A, "A", "B"), evolution(IN_B, "B", "A")] {
        let (status, body) = send(&st, "POST", "/ngsi-ld/v1/temporal/entities", Some(doc)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    st.policy = Some(engine);
    st
}

async fn send(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => {
            let payload = v.to_string();
            b.header("Content-Type", "application/json")
                .header("Content-Length", payload.len())
                .body(Body::from(payload))
                .expect("request")
        }
        None => b.body(Body::empty()).expect("request"),
    };
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn evolution_of(st: &AppState, id: &str) -> Value {
    let tenant = TenantId::default();
    st.temporal
        .get(&tenant, id)
        .await
        .expect("temporal get")
        .unwrap_or(Value::Null)
}

fn speed_instance(doc: &Value) -> String {
    doc[format!("{DC}speed")][0]["instanceId"]
        .as_str()
        .unwrap_or("none")
        .to_owned()
}

/// Every conditioned temporal write, as (clause, method, path, body).
fn writes(id: &str, instance: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    let e = format!("/ngsi-ld/v1/temporal/entities/{id}");
    vec![
        (
            "5.6.12",
            "POST",
            format!("{e}/attrs"),
            Some(json!({"speed": [{"type": "Property", "value": 9,
                                   "observedAt": "2026-01-01T00:00:03Z"}]})),
        ),
        (
            "5.6.14",
            "PATCH",
            format!("{e}/attrs/speed/{instance}"),
            Some(json!({"type": "Property", "value": 9, "observedAt": "2026-01-01T00:00:01Z"})),
        ),
        (
            "5.6.15",
            "DELETE",
            format!("{e}/attrs/speed/{instance}"),
            None,
        ),
        ("5.6.13", "DELETE", format!("{e}/attrs/speed"), None),
    ]
}

/// The evolution whose latest zone is A is written; the one whose latest
/// zone is B, although it was in A once, is a 404 with the body an absent
/// evolution gets, and stays as it was.
#[tokio::test(flavor = "multi_thread")]
async fn a_temporal_write_lands_only_on_an_evolution_the_condition_selects() {
    for clause in ["5.6.12", "5.6.13", "5.6.14", "5.6.15"] {
        let st = state(Arc::new(Conditioned)).await;
        let a = speed_instance(&evolution_of(&st, IN_A).await);
        let b = speed_instance(&evolution_of(&st, IN_B).await);
        let (_, method, uri, body) = writes(IN_A, &a)
            .into_iter()
            .find(|w| w.0 == clause)
            .expect("write");
        let (_, _, outside, _) = writes(IN_B, &b)
            .into_iter()
            .find(|w| w.0 == clause)
            .expect("write");
        let (status, out) = send(&st, method, &uri, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::NO_CONTENT,
            "{clause} {method} {uri}: {out}"
        );

        let before = evolution_of(&st, IN_B).await;
        let (status, refused) = send(&st, method, &outside, body.clone()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{clause} {method} {outside}");
        assert_eq!(evolution_of(&st, IN_B).await, before, "{clause}: unchanged");

        let absent = outside.replace(IN_B, ABSENT);
        let (status, missing) = send(&st, method, &absent, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{clause} {method} {absent}");
        assert_eq!(
            refused.to_string().replace(IN_B, ABSENT),
            missing.to_string(),
            "{clause}: a non-matching evolution answers as an absent one"
        );
    }
}

/// Delete Temporal Evolution has no conditional delete to judge in: a
/// condition on it stays a refusal, and the evolution stays.
#[tokio::test(flavor = "multi_thread")]
async fn a_condition_on_deleting_an_evolution_is_a_refusal() {
    let st = state(Arc::new(On("5.6.16"))).await;
    let uri = format!("/ngsi-ld/v1/temporal/entities/{IN_A}");
    let (status, body) = send(&st, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_ne!(evolution_of(&st, IN_A).await, Value::Null);
}

/// Moves IN_A to zone B (a later zone instance) just before the first
/// `mutate` runs: the second writer between the decision and the commit.
struct Racing {
    inner: Arc<dyn TemporalDriver>,
    raced: AtomicBool,
}

#[async_trait::async_trait]
impl TemporalDriver for Racing {
    async fn temporal_append(
        &self,
        t: &TenantId,
        id: &str,
        shell: &Value,
        additions: &Value,
    ) -> Result<(), NgsiError> {
        self.inner.temporal_append(t, id, shell, additions).await
    }
    async fn query_temporal(
        &self,
        t: &TenantId,
        f: &filter::TemporalFilter<'_>,
    ) -> Result<filter::TemporalOutcome, NgsiError> {
        self.inner.query_temporal(t, f).await
    }
    async fn get_temporal(
        &self,
        t: &TenantId,
        id: &str,
        f: &filter::TemporalFilter<'_>,
    ) -> Result<Option<Value>, NgsiError> {
        self.inner.get_temporal(t, id, f).await
    }
    async fn get(&self, t: &TenantId, id: &str) -> Result<Option<Value>, NgsiError> {
        self.inner.get(t, id).await
    }
    async fn create(&self, t: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError> {
        self.inner.create(t, id, doc).await
    }
    async fn upsert(&self, t: &TenantId, id: &str, doc: Value) -> Result<bool, NgsiError> {
        self.inner.upsert(t, id, doc).await
    }
    async fn delete(&self, t: &TenantId, id: &str) -> Result<bool, NgsiError> {
        self.inner.delete(t, id).await
    }
    async fn list(&self, t: &TenantId) -> Result<Vec<Value>, NgsiError> {
        self.inner.list(t).await
    }
    async fn mutate_boxed<'a>(
        &self,
        t: &TenantId,
        id: &str,
        f: MutateFn<'a>,
    ) -> Result<Option<Result<(), ()>>, NgsiError> {
        if !self.raced.swap(true, Ordering::SeqCst) {
            if let Some(mut doc) = self.inner.get(t, id).await? {
                if let Some(zones) = doc
                    .get_mut(format!("{DC}zone"))
                    .and_then(Value::as_array_mut)
                {
                    zones.push(zone("B", "2026-01-01T00:00:09Z"));
                }
                self.inner.upsert(t, id, doc).await?;
            }
        }
        self.inner.mutate_boxed(t, id, f).await
    }
}

/// The engine decided on an evolution in zone A; a second writer moves it
/// to zone B before the write commits. The temporal store judges the
/// document it is about to change, so the write does not land.
#[tokio::test(flavor = "multi_thread")]
async fn a_concurrent_change_cannot_carry_a_temporal_write_through() {
    for clause in ["5.6.12", "5.6.13", "5.6.14", "5.6.15"] {
        let mut st = state(Arc::new(Conditioned)).await;
        let a = speed_instance(&evolution_of(&st, IN_A).await);
        st.temporal = Arc::new(Racing {
            inner: st.temporal.clone(),
            raced: AtomicBool::new(false),
        });
        let (_, method, uri, body) = writes(IN_A, &a)
            .into_iter()
            .find(|w| w.0 == clause)
            .expect("write");
        let (status, out) = send(&st, method, &uri, body).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{clause} {method} {uri}: {out}"
        );
        let after = evolution_of(&st, IN_A).await;
        let speeds = after[format!("{DC}speed")].as_array().cloned();
        assert_eq!(
            speeds.map(|s| s.iter().map(|i| i["value"].clone()).collect::<Vec<_>>()),
            Some(vec![json!(1)]),
            "{clause}: {after}"
        );
    }
}

/// A conditioned temporal write that would reach a Context Source is
/// refused, and nothing is written here either.
#[tokio::test(flavor = "multi_thread")]
async fn a_conditioned_temporal_write_is_not_forwarded() {
    antares_jsonld::allow_private_egress(true);
    let mut st = state(Arc::new(Conditioned)).await;
    let engine = st.policy.take();
    let reg = json!({
        "type": "ContextSourceRegistration",
        "information": [{"entities": [{"id": IN_A, "type": "Vehicle"}]}],
        "mode": "inclusive",
        "operations": ["deleteAttrsTemporal"],
        "endpoint": "http://127.0.0.1:9",
    });
    let (status, body) = send(&st, "POST", "/ngsi-ld/v1/csourceRegistrations", Some(reg)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    st.policy = engine;
    let before = evolution_of(&st, IN_A).await;
    let uri = format!("/ngsi-ld/v1/temporal/entities/{IN_A}/attrs/speed");
    let (status, out) = send(&st, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(out["detail"], CONDITION_NOT_FORWARDED);
    assert_eq!(evolution_of(&st, IN_A).await, before);
}

/// A temporal write by id has no write guard (ADR-0023 rule 7), so under a
/// view it stays refused, conditioned or not.
#[cfg(feature = "views")]
#[test]
fn a_temporal_write_by_id_is_refused_under_a_view() {
    use antares_api::policy::Rule;
    use antares_api::views::{with_view, CompiledView};
    let cars = CompiledView::new(
        &["Car".into()],
        None,
        None,
        None,
        vec![Rule {
            types: vec!["Car".into()],
            attributes: vec!["speed".into()],
            ..Rule::default()
        }],
    )
    .expect("a view");
    for clause in ["5.6.12", "5.6.13", "5.6.14", "5.6.15"] {
        assert!(WRITE_BY_ID.contains(&clause), "{clause}");
        for engine in [Filter::default(), zone_a()] {
            assert!(
                with_view(Some(&cars), clause, Ok(Arc::new(engine))).is_err(),
                "{clause}"
            );
        }
    }
}
