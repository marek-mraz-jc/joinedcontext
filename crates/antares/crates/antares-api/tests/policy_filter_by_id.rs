// SPDX-License-Identifier: EUPL-1.2
//! A rule narrows by Entity id (gap G2 of `docs/analysis/gateway-to-broker-map.md`, T-3631).
//!
//! The platform's grants name entities by `id` and by an anchored `idPattern`
//! (map rule kind K8); `Rule` named only types and a condition, so a grant on
//! one Car served every Car. A rule's `ids` and `id_patterns` say which
//! Entities it holds for, and every document the answer carries is judged by
//! them: a query and its count, a retrieve (404 for an id outside, like an
//! absent one), a joined target and a temporal history.
#![cfg(feature = "test-kit")]
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Rule, Subject,
};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const DEFAULT: &str = "https://uri.etsi.org/ngsi-ld/default-context/";

fn iri(term: &str) -> String {
    format!("{DEFAULT}{term}")
}

/// Narrows every read with the same Filter; allows everything else, so a
/// test seeds through the router.
struct Narrowing(Filter);

impl PolicyEngine for Narrowing {
    fn name(&self) -> &str {
        "by-id"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let answer = if antares_api::policy::FILTERABLE.contains(&op.clause) {
            Decision::Filter(std::sync::Arc::new(self.0.clone()))
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(answer))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Filter(Box::new(self.0.clone()))
    }
}

/// Cars by `ids` and `id_patterns`, showing `speed` and `ownedBy`; Users
/// by `user_ids`, showing `name`.
fn grant(ids: &[&str], id_patterns: &[&str], user_ids: &[&str]) -> Filter {
    let owned = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect();
    Filter {
        rules: vec![
            Rule {
                types: vec![iri("Car")],
                ids: owned(ids),
                id_patterns: owned(id_patterns),
                attributes: vec![iri("speed"), iri("ownedBy")],
                ..Rule::default()
            },
            Rule {
                types: vec![iri("User")],
                ids: owned(user_ids),
                attributes: vec![iri("name")],
                ..Rule::default()
            },
        ],
        ..Filter::default()
    }
}

async fn state(f: Filter) -> AppState {
    antares_jsonld::allow_private_egress(true);
    let mut st = AppState::new("me".into()).with_policy(Arc::new(Narrowing(f)));
    antares_api::wire(&mut st).await;
    st
}

async fn send(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => {
            let payload = v.to_string();
            b = b
                .header("Content-Type", "application/json")
                .header("Content-Length", payload.len());
            b.body(Body::from(payload)).unwrap()
        }
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = antares_api::router(st.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let doc = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, doc)
}

fn prop(v: Value) -> Value {
    json!({"type": "Property", "value": v})
}

/// Cars 1, 2 and 10, each owned by User 1.
async fn seed(st: &AppState) {
    for n in ["1", "2", "10"] {
        let car = json!({"id": format!("urn:ngsi-ld:Car:{n}"), "type": "Car",
                         "speed": prop(json!(80)),
                         "ownedBy": {"type": "Relationship", "object": "urn:ngsi-ld:User:1"}});
        let (code, body) = send(st, "POST", "/ngsi-ld/v1/entities", Some(car)).await;
        assert_eq!(code, StatusCode::CREATED, "{body}");
    }
    let user = json!({"id": "urn:ngsi-ld:User:1", "type": "User", "name": prop(json!("Ana"))});
    let (code, body) = send(st, "POST", "/ngsi-ld/v1/entities", Some(user)).await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
}

fn ids(list: &Value) -> Vec<String> {
    let mut v: Vec<String> = list
        .as_array()
        .unwrap_or_else(|| panic!("not a list: {list}"))
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    v.sort();
    v
}

/// `NGSILD-Results-Count` of a GET.
async fn count(st: &AppState, uri: &str) -> Option<u64> {
    antares_api::router(st.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .headers()
        .get("NGSILD-Results-Count")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_id_outside_the_rule_is_neither_listed_nor_counted_nor_retrieved() {
    let st = state(grant(&["urn:ngsi-ld:Car:1"], &[], &[])).await;
    seed(&st).await;
    let (code, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car", None).await;
    assert_eq!(code, StatusCode::OK, "{list}");
    assert_eq!(ids(&list), ["urn:ngsi-ld:Car:1"]);
    assert_eq!(
        count(&st, "/ngsi-ld/v1/entities?type=Car&count=true").await,
        Some(1)
    );
    let (code, _) = send(&st, "GET", "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:1", None).await;
    assert_eq!(code, StatusCode::OK);
    let (hidden, hidden_body) =
        send(&st, "GET", "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:2", None).await;
    let (absent, absent_body) =
        send(&st, "GET", "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:99", None).await;
    assert_eq!(hidden, StatusCode::NOT_FOUND, "{hidden_body}");
    assert_eq!(absent, StatusCode::NOT_FOUND);
    assert_eq!(
        hidden_body["type"], absent_body["type"],
        "a hidden id answers like an absent one"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_id_pattern_is_anchored_and_a_rule_without_ids_holds_for_every_id() {
    let st = state(grant(&[], &["urn:ngsi-ld:Car:1"], &[])).await;
    seed(&st).await;
    let (_, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car", None).await;
    assert_eq!(
        ids(&list),
        ["urn:ngsi-ld:Car:1"],
        "Car:10 contains the pattern but does not match it whole"
    );

    let st = state(grant(&[], &["urn:ngsi-ld:Car:(1|2)"], &[])).await;
    seed(&st).await;
    let (_, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car", None).await;
    assert_eq!(ids(&list), ["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:2"]);

    let st = state(grant(&[], &[], &[])).await;
    seed(&st).await;
    let (_, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car", None).await;
    assert_eq!(ids(&list).len(), 3, "no id narrowing: every Car");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ids_and_id_patterns_together_hold_for_either() {
    let st = state(grant(&["urn:ngsi-ld:Car:10"], &["urn:ngsi-ld:Car:1"], &[])).await;
    seed(&st).await;
    let (_, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car", None).await;
    assert_eq!(ids(&list), ["urn:ngsi-ld:Car:1", "urn:ngsi-ld:Car:10"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invalid_or_oversized_id_pattern_holds_for_nothing() {
    for pattern in ["urn:ngsi-ld:Car:(", "(?:a{1000}){1000}"] {
        let st = state(grant(&[], &[pattern], &[])).await;
        seed(&st).await;
        let (code, list) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car", None).await;
        assert_eq!(code, StatusCode::OK, "{pattern}: {list}");
        assert_eq!(ids(&list), Vec::<String>::new(), "{pattern}: fail closed");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_joined_target_outside_its_rule_is_not_inlined() {
    let st = state(grant(&["urn:ngsi-ld:Car:1"], &[], &["urn:ngsi-ld:User:2"])).await;
    seed(&st).await;
    for join in ["inline", "flat"] {
        let (code, body) = send(
            &st,
            "GET",
            &format!("/ngsi-ld/v1/entities/urn:ngsi-ld:Car:1?join={join}&joinLevel=1"),
            None,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{body}");
        assert!(
            !body.to_string().contains("Ana"),
            "{join}: User:1 is outside its rule's ids: {body}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_temporal_history_outside_the_rule_is_not_served() {
    let st = state(grant(&["urn:ngsi-ld:Car:t1"], &[], &[])).await;
    for id in ["urn:ngsi-ld:Car:t1", "urn:ngsi-ld:Car:t2"] {
        let doc = json!({"id": id, "type": "Car",
                         "speed": [{"type": "Property", "value": 1,
                                    "observedAt": "2026-01-01T00:00:00Z"}]});
        let (code, body) = send(&st, "POST", "/ngsi-ld/v1/temporal/entities", Some(doc)).await;
        // TS 104 176 8.2.3.2: a new Temporal Representation is 201
        assert_eq!(code, StatusCode::CREATED, "{body}");
    }
    let (code, list) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities?type=Car&timerel=after&timeAt=1970-01-01T00:00:00Z",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{list}");
    assert_eq!(ids(&list), ["urn:ngsi-ld:Car:t1"]);
    let (code, body) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/temporal/entities/urn:ngsi-ld:Car:t2",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND, "{body}");
}
