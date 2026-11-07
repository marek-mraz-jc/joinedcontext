// SPDX-License-Identifier: EUPL-1.2
//! A write allowed only on the Entities a condition selects (gap G12,
//! T-3634): the engine answers `Decision::AllowIf(filter)`, and the store
//! applies the write only if the stored Entity matches the filter, judged
//! inside the store's own read-modify-write. An Entity it does not select
//! answers as an absent one, and a concurrent write between the decision
//! and the commit cannot carry the write onto an Entity it no longer
//! selects.
#![cfg(feature = "test-kit")]
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use crate::common::Double;
use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Rule, Subject,
    CONDITION_NOT_FORWARDED, WRITE_BY_ID,
};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context/";
const IN_A: &str = "urn:ngsi-ld:Vehicle:in-a";
const IN_B: &str = "urn:ngsi-ld:Vehicle:in-b";
const ABSENT: &str = "urn:ngsi-ld:Vehicle:absent";

/// Answers every write by id with `AllowIf(self.0)` and everything else
/// with `Allow`.
struct Conditioned(Filter);

impl PolicyEngine for Conditioned {
    fn name(&self) -> &str {
        "conditioned"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(if WRITE_BY_ID.contains(&op.clause) {
            Decision::AllowIf(self.0.clone())
        } else {
            Decision::Allow
        }))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// Answers `self.1` on the clause `self.0` and `Allow` everywhere else.
struct On(&'static str, Decision);

impl PolicyEngine for On {
    fn name(&self) -> &str {
        "on"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(if op.clause == self.0 {
            self.1.clone()
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

fn vehicle(id: &str, zone: &str) -> Value {
    json!({"id": id, "type": "Vehicle",
           "zone": {"type": "Property", "value": zone},
           "speed": {"type": "Property", "value": 1}})
}

/// The two Vehicles, created before the engine is attached.
async fn state(engine: Arc<dyn PolicyEngine>) -> AppState {
    let mut st = AppState::new("me".into());
    antares_api::wire(&mut st).await;
    for doc in [vehicle(IN_A, "A"), vehicle(IN_B, "B")] {
        let (status, _) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(doc)).await;
        assert_eq!(status, StatusCode::CREATED);
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

async fn stored(st: &AppState, id: &str) -> Value {
    send(st, "GET", &format!("/ngsi-ld/v1/entities/{id}"), None)
        .await
        .1
}

/// Every write by id, as (clause, method, path under the Entity, body).
fn writes(id: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    let e = format!("/ngsi-ld/v1/entities/{id}");
    let speed = json!({"speed": {"type": "Property", "value": 9}});
    vec![
        ("5.6.2", "PATCH", format!("{e}/attrs"), Some(speed.clone())),
        ("5.6.3", "POST", format!("{e}/attrs"), Some(speed.clone())),
        (
            "5.6.4",
            "PATCH",
            format!("{e}/attrs/speed"),
            Some(json!({"value": 9})),
        ),
        ("5.6.5", "DELETE", format!("{e}/attrs/speed"), None),
        ("5.6.17", "PATCH", e.clone(), Some(speed)),
        (
            "5.6.18",
            "PUT",
            e.clone(),
            Some(json!({"id": id, "type": "Vehicle", "speed": {"type": "Property", "value": 9}})),
        ),
        (
            "5.6.19",
            "PUT",
            format!("{e}/attrs/speed"),
            Some(json!({"type": "Property", "value": 9})),
        ),
        (
            "5.6.19",
            "PUT",
            format!("{e}/attrs/speed/value"),
            Some(json!(9)),
        ),
        ("5.6.6", "DELETE", e, None),
    ]
}

/// The Entity the condition selects is written; the one it does not is a
/// 404 with the very body an absent Entity gets, and stays as it was.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_by_id_lands_only_on_an_entity_the_condition_selects() {
    for ((clause, method, uri, body), (_, _, outside, _)) in
        writes(IN_A).into_iter().zip(writes(IN_B))
    {
        let st = state(Arc::new(Conditioned(zone_a()))).await;
        let before = stored(&st, IN_B).await;
        let (status, _) = send(&st, method, &uri, body.clone()).await;
        assert!(status.is_success(), "{clause} {method} {uri}: {status}");

        let (status, refused) = send(&st, method, &outside, body.clone()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{clause} {method} {outside}");
        assert_eq!(stored(&st, IN_B).await, before, "{clause}: unchanged");

        let absent = outside.replace(IN_B, ABSENT);
        let (status, missing) = send(&st, method, &absent, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{clause} {method} {absent}");
        assert_eq!(
            refused.to_string().replace(IN_B, ABSENT),
            missing.to_string(),
            "{clause}: a non-matching Entity answers as an absent one"
        );
    }
}

/// A batch decides each member by its own stored Entity: the selected one
/// is written, the other lands in E as not found, and stays as it was.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_applies_the_condition_per_member() {
    let speed =
        |id: &str| json!({"id": id, "type": "Vehicle", "speed": {"type": "Property", "value": 9}});
    for (op, body) in [
        ("update", json!([speed(IN_A), speed(IN_B)])),
        ("merge", json!([speed(IN_A), speed(IN_B)])),
        ("delete", json!([IN_A, IN_B])),
    ] {
        let st = state(Arc::new(Conditioned(zone_a()))).await;
        let before = stored(&st, IN_B).await;
        let (status, out) = send(
            &st,
            "POST",
            &format!("/ngsi-ld/v1/entityOperations/{op}"),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::MULTI_STATUS, "{op}: {out}");
        assert_eq!(out["success"], json!([IN_A]), "{op}: {out}");
        assert_eq!(out["errors"][0]["entityId"], IN_B, "{op}: {out}");
        assert_eq!(out["errors"][0]["error"]["status"], 404, "{op}: {out}");
        assert_eq!(stored(&st, IN_B).await, before, "{op}: unchanged");
    }
}

/// Batch Upsert (T-3668): a conditioned member updates the Entity its
/// condition selects and never creates. The one it does not select and an
/// absent one answer the same 404 in E, and neither is written.
#[tokio::test(flavor = "multi_thread")]
async fn an_upsert_updates_what_the_condition_selects_and_creates_nothing() {
    let speed =
        |id: &str| json!({"id": id, "type": "Vehicle", "speed": {"type": "Property", "value": 9}});
    for uri in [
        "/ngsi-ld/v1/entityOperations/upsert",
        "/ngsi-ld/v1/entityOperations/upsert?options=update",
    ] {
        let st = state(Arc::new(Conditioned(zone_a()))).await;
        let before = stored(&st, IN_B).await;
        let body = json!([speed(IN_A), speed(IN_B), speed(ABSENT)]);
        let (status, out) = send(&st, "POST", uri, Some(body)).await;
        assert_eq!(status, StatusCode::MULTI_STATUS, "{uri}: {out}");
        assert_eq!(out["success"], json!([IN_A]), "{uri}: {out}");
        let errors = out["errors"].as_array().cloned().unwrap_or_default();
        let entry = |id: &str| {
            errors
                .iter()
                .find(|e| e["entityId"] == id)
                .cloned()
                .unwrap_or(Value::Null)
        };
        assert_eq!(entry(IN_B)["error"]["status"], 404, "{uri}: {out}");
        assert_eq!(
            entry(IN_B).to_string().replace(IN_B, ABSENT),
            entry(ABSENT).to_string(),
            "{uri}: a non-matching Entity answers as an absent one"
        );
        assert_eq!(stored(&st, IN_B).await, before, "{uri}: unchanged");
        assert_eq!(stored(&st, IN_A).await["speed"]["value"], 9, "{uri}");
        let (status, _) = send(&st, "GET", &format!("/ngsi-ld/v1/entities/{ABSENT}"), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: never created");
    }
}

/// An upsert's condition is judged on the document the store is about to
/// change: a writer that moves the Entity out of zone A first wins.
#[tokio::test(flavor = "multi_thread")]
async fn an_upsert_cannot_slip_past_a_concurrent_change() {
    let moved = json!({
        "id": IN_A,
        "type": [format!("{DC}Vehicle")],
        format!("{DC}zone"): [{"type": "Property", "value": "B"}],
        format!("{DC}speed"): [{"type": "Property", "value": 1}],
        "createdAt": "2026-01-01T00:00:00.000Z",
        "modifiedAt": "2026-01-01T00:00:00.000Z",
    });
    let body = json!([{"id": IN_A, "type": "Vehicle", "speed": {"type": "Property", "value": 9}}]);
    for uri in [
        "/ngsi-ld/v1/entityOperations/upsert",
        "/ngsi-ld/v1/entityOperations/upsert?options=update",
    ] {
        let mut st = state(Arc::new(Conditioned(zone_a()))).await;
        st.store = Arc::new(Double::racing_write(st.store.clone(), moved.clone()));
        let (status, out) = send(&st, "POST", uri, Some(body.clone())).await;
        assert_eq!(status, StatusCode::MULTI_STATUS, "{uri}: {out}");
        assert_eq!(out["errors"][0]["error"]["status"], 404, "{uri}: {out}");
        let after = stored(&st, IN_A).await;
        assert_eq!(after["zone"]["value"], "B", "{uri}: {after}");
        assert_eq!(after["speed"]["value"], 1, "{uri}: {after}");
    }
}

/// The engine decided on an Entity in zone A; a second writer moves it to
/// zone B before the write commits. The store judges the document it is
/// about to change, so the write does not land.
#[tokio::test(flavor = "multi_thread")]
async fn a_concurrent_change_between_decision_and_write_cannot_slip_through() {
    let moved = json!({
        "id": IN_A,
        "type": [format!("{DC}Vehicle")],
        format!("{DC}zone"): [{"type": "Property", "value": "B"}],
        format!("{DC}speed"): [{"type": "Property", "value": 1}],
        "createdAt": "2026-01-01T00:00:00.000Z",
        "modifiedAt": "2026-01-01T00:00:00.000Z",
    });
    for (clause, method, uri, body) in writes(IN_A) {
        let mut st = state(Arc::new(Conditioned(zone_a()))).await;
        st.store = Arc::new(Double::racing_write(st.store.clone(), moved.clone()));
        let (status, _) = send(&st, method, &uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{clause} {method} {uri}");
        let after = stored(&st, IN_A).await;
        assert_eq!(after["zone"]["value"], "B", "{clause}: {after}");
        assert_eq!(after["speed"]["value"], 1, "{clause}: {after}");
    }
}

/// An `AllowIf` holds only where it can be checked: on a write by id, and
/// with a filter that selects rather than projects. Anywhere else it is a
/// refusal; an empty condition is an allow.
#[tokio::test(flavor = "multi_thread")]
async fn a_condition_outside_a_write_by_id_or_with_a_projection_is_a_refusal() {
    let picking = Filter {
        pick: vec!["speed".into()],
        ..zone_a()
    };
    let showing = Filter {
        rules: vec![Rule {
            types: vec!["Vehicle".into()],
            attributes: vec!["speed".into()],
            ..Rule::default()
        }],
        ..Filter::default()
    };
    let patch = format!("/ngsi-ld/v1/entities/{IN_A}/attrs");
    let speed = Some(json!({"speed": {"type": "Property", "value": 9}}));
    for (clause, decision, method, uri, body, want) in [
        (
            "5.6.1",
            Decision::AllowIf(zone_a()),
            "POST",
            "/ngsi-ld/v1/entities".to_owned(),
            Some(vehicle("urn:ngsi-ld:Vehicle:new", "A")),
            StatusCode::FORBIDDEN,
        ),
        (
            "5.7.1",
            Decision::AllowIf(zone_a()),
            "GET",
            format!("/ngsi-ld/v1/entities/{IN_A}"),
            None,
            StatusCode::FORBIDDEN,
        ),
        (
            "5.6.2",
            Decision::AllowIf(picking),
            "PATCH",
            patch.clone(),
            speed.clone(),
            StatusCode::FORBIDDEN,
        ),
        (
            "5.6.2",
            Decision::AllowIf(showing),
            "PATCH",
            patch.clone(),
            speed.clone(),
            StatusCode::FORBIDDEN,
        ),
        (
            "5.6.2",
            Decision::AllowIf(Filter::default()),
            "PATCH",
            format!("/ngsi-ld/v1/entities/{IN_B}/attrs"),
            speed,
            StatusCode::NO_CONTENT,
        ),
    ] {
        let st = state(Arc::new(On(clause, decision))).await;
        let (status, body) = send(&st, method, &uri, body).await;
        assert_eq!(status, want, "{clause} {method} {uri}: {body}");
    }
}

/// A rule's selection counts as a condition: the type and the rule's own
/// `q` both have to hold for the stored Entity.
#[tokio::test(flavor = "multi_thread")]
async fn a_rule_selects_by_type_and_its_own_query() {
    let rule = Filter {
        rules: vec![Rule {
            types: vec!["Vehicle".into()],
            q: Some(Box::new(antares_ql::parse_q("zone==\"A\"").expect("q"))),
            ..Rule::default()
        }],
        ..Filter::default()
    };
    let st = state(Arc::new(Conditioned(rule))).await;
    let speed = Some(json!({"speed": {"type": "Property", "value": 9}}));
    for (id, want) in [
        (IN_A, StatusCode::NO_CONTENT),
        (IN_B, StatusCode::NOT_FOUND),
    ] {
        let uri = format!("/ngsi-ld/v1/entities/{id}/attrs");
        let (status, body) = send(&st, "PATCH", &uri, speed.clone()).await;
        assert_eq!(status, want, "{id}: {body}");
    }
}

/// A Context Source's copy is not in this store, so nothing here can judge
/// it: a conditioned write that would be forwarded is refused, and nothing
/// is written locally either.
#[tokio::test(flavor = "multi_thread")]
async fn a_conditioned_write_is_not_forwarded() {
    antares_jsonld::allow_private_egress(true);
    let mut st = state(Arc::new(Conditioned(zone_a()))).await;
    let engine = st.policy.take();
    let reg = json!({
        "type": "ContextSourceRegistration",
        "information": [{"entities": [{"id": IN_A, "type": "Vehicle"}]}],
        "mode": "inclusive",
        "operations": ["updateOps"],
        "endpoint": "http://127.0.0.1:9",
    });
    let (status, body) = send(&st, "POST", "/ngsi-ld/v1/csourceRegistrations", Some(reg)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    st.policy = engine;
    let before = stored(&st, IN_A).await;
    let speed = json!({"speed": {"type": "Property", "value": 9}});
    let mut batch = speed.clone();
    batch["id"] = json!(IN_A);
    batch["type"] = json!("Vehicle");
    for (method, uri, body) in [
        ("PATCH", format!("/ngsi-ld/v1/entities/{IN_A}/attrs"), speed),
        (
            "POST",
            "/ngsi-ld/v1/entityOperations/update".to_owned(),
            json!([batch]),
        ),
    ] {
        let (status, out) = send(&st, method, &uri, Some(body)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {out}");
        assert_eq!(out["detail"], CONDITION_NOT_FORWARDED, "{method} {uri}");
        assert_eq!(stored(&st, IN_A).await, before, "{method} {uri}");
    }
}
