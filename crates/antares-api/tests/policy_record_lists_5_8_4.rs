// SPDX-License-Identifier: EUPL-1.2
//! Query Subscriptions (5.8.4, 5.11.5) and Query Registrations (5.10.2)
//! narrowed by a policy Filter (ADR-0020, T-3673).
//!
//! An engine that could only allow or deny the whole list had to show every
//! subject every Subscription of the tenant. A `Decision::Filter` there now
//! selects which documents are listed, and the count and the pages are over
//! that selection, so no foreign id, count or page boundary shows. The
//! subject that created a Subscription is the `subject` the condition reads;
//! a member a client writes under that name is not. A Filter that projects
//! is still a deny: these lists serve records, not Entities.
#![cfg(feature = "test-kit")]
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Subject,
    RECORD_LISTS,
};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const SUBJECT: &str = "X-Subject";

/// Lists only what the asking subject made: Subscriptions by the subject
/// the broker recorded, Registrations by their `registrationName`, or by
/// the recorded subject too when the flag is set (T-3692). Every other
/// operation is allowed, so a test seeds through the router.
struct Owner(bool);

impl PolicyEngine for Owner {
    fn name(&self) -> &str {
        "owner"
    }

    fn decide<'a>(&'a self, s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let me = s
            .headers
            .first()
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let member = if op.clause == "5.10.2" && !self.0 {
            "registrationName"
        } else {
            "subject"
        };
        let answer = if RECORD_LISTS.contains(&op.clause) {
            Decision::Filter(Filter {
                q: Some(Box::new(
                    antares_ql::parse_q(&format!("{member}=={me:?}")).unwrap(),
                )),
                ..Filter::default()
            })
        } else {
            Decision::Allow
        };
        Box::pin(std::future::ready(answer))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

/// Answers every operation with the same decision.
struct Fixed(Decision);

impl PolicyEngine for Fixed {
    fn name(&self) -> &str {
        "fixed"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, _op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(self.0.clone()))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

fn state(engine: Arc<dyn PolicyEngine>) -> AppState {
    // before the first request: the header list is read once per process
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", SUBJECT);
    AppState::new("me".into()).with_policy(engine)
}

async fn send(
    st: &AppState,
    who: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let b = Request::builder()
        .method(method)
        .uri(uri)
        .header(SUBJECT, who);
    let req = match body {
        Some(v) => {
            let payload = v.to_string();
            b.header("Content-Type", "application/json")
                .header("Content-Length", payload.len())
                .body(Body::from(payload))
                .unwrap()
        }
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = antares_api::router(st.clone()).oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
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

fn count(h: &HeaderMap) -> Option<u64> {
    h.get("NGSILD-Results-Count")?.to_str().ok()?.parse().ok()
}

/// Alice makes 0, 2 and 4, Bob 1 and 3 — interleaved, so a window over the
/// tenant would cut through both. Bob's 3 claims to be Alice's in a member.
async fn seed_subscriptions(st: &AppState, path: &str, type_: &str) -> Vec<String> {
    let mut alices = Vec::new();
    for n in 0..5 {
        let who = if n % 2 == 0 { "alice" } else { "bob" };
        let id = format!("urn:ngsi-ld:{type_}:{n}");
        let mut sub = json!({"id": id, "type": type_, "entities": [{"type": "Car"}],
                             "notification": {"endpoint": {"uri": "http://127.0.0.1:1/n"}}});
        if n == 3 {
            sub["subject"] = json!("alice");
        }
        let (code, _, body) = send(st, who, "POST", path, Some(sub)).await;
        assert_eq!(code, StatusCode::CREATED, "{body}");
        if who == "alice" {
            alices.push(id);
        }
    }
    alices
}

async fn lists_only_the_callers_own(path: &str, type_: &str) {
    let st = state(Arc::new(Owner(false)));
    let alices = seed_subscriptions(&st, path, type_).await;

    let (code, h, list) = send(&st, "alice", "GET", &format!("{path}?count=true"), None).await;
    assert_eq!(code, StatusCode::OK, "{list}");
    assert_eq!(ids(&list), alices);
    assert_eq!(count(&h), Some(3), "the count is the narrowed one");

    // every page of one is Alice's, in turn, and the window ends with hers
    let mut paged = Vec::new();
    for offset in 0..4 {
        let uri = format!("{path}?limit=1&offset={offset}&count=true");
        let (code, h, page) = send(&st, "alice", "GET", &uri, None).await;
        assert_eq!(code, StatusCode::OK, "{page}");
        assert_eq!(count(&h), Some(3));
        paged.extend(ids(&page));
    }
    paged.sort();
    assert_eq!(
        paged, alices,
        "no foreign id on any page, none past the third"
    );

    let (_, h, list) = send(&st, "bob", "GET", &format!("{path}?count=true"), None).await;
    assert_eq!(
        ids(&list),
        vec![
            format!("urn:ngsi-ld:{type_}:1"),
            format!("urn:ngsi-ld:{type_}:3")
        ]
    );
    assert_eq!(count(&h), Some(2));
}

#[tokio::test]
async fn query_subscriptions_lists_only_the_callers_own() {
    lists_only_the_callers_own("/ngsi-ld/v1/subscriptions", "Subscription").await;
}

#[tokio::test]
async fn query_csource_subscriptions_lists_only_the_callers_own() {
    lists_only_the_callers_own("/ngsi-ld/v1/csourceSubscriptions", "Subscription").await;
}

#[tokio::test]
async fn query_registrations_lists_only_what_the_filter_selects() {
    let st = state(Arc::new(Owner(false)));
    for n in 0..5 {
        let who = if n % 2 == 0 { "alice" } else { "bob" };
        let reg = json!({"id": format!("urn:ngsi-ld:ContextSourceRegistration:{n}"),
                         "type": "ContextSourceRegistration", "registrationName": who,
                         "information": [{"entities": [{"type": "Car"}]}],
                         "endpoint": "http://127.0.0.1:1"});
        let (code, _, body) = send(
            &st,
            who,
            "POST",
            "/ngsi-ld/v1/csourceRegistrations",
            Some(reg),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "{body}");
    }
    let alices: Vec<String> = [0, 2, 4]
        .iter()
        .map(|n| format!("urn:ngsi-ld:ContextSourceRegistration:{n}"))
        .collect();
    let path = "/ngsi-ld/v1/csourceRegistrations?type=Car&count=true";
    let (code, h, list) = send(&st, "alice", "GET", path, None).await;
    assert_eq!(code, StatusCode::OK, "{list}");
    assert_eq!(ids(&list), alices);
    assert_eq!(count(&h), Some(3));

    let mut paged = Vec::new();
    for offset in 0..4 {
        let uri = format!("{path}&limit=1&offset={offset}");
        let (code, h, page) = send(&st, "alice", "GET", &uri, None).await;
        assert_eq!(code, StatusCode::OK, "{page}");
        assert_eq!(count(&h), Some(3));
        paged.extend(ids(&page));
    }
    paged.sort();
    assert_eq!(paged, alices);
}

#[tokio::test]
async fn a_projection_on_a_record_list_stays_a_deny() {
    for f in [
        Filter {
            pick: vec!["id".into()],
            ..Filter::default()
        },
        Filter {
            omit: vec!["description".into()],
            ..Filter::default()
        },
    ] {
        let st = state(Arc::new(Fixed(Decision::Filter(f))));
        for path in [
            "/ngsi-ld/v1/subscriptions",
            "/ngsi-ld/v1/csourceSubscriptions",
            "/ngsi-ld/v1/csourceRegistrations?type=Car",
        ] {
            let (code, _, body) = send(&st, "alice", "GET", path, None).await;
            assert_eq!(code, StatusCode::FORBIDDEN, "{path}: {body}");
        }
    }
}

/// A Registration records the subject that created it (T-3692): Query
/// Registrations narrows by it as Query Subscriptions does, count and pages
/// included. Bob's 3 claims to be Alice's under `subject` and under the
/// broker's own `__subject`; neither selects, and no served Registration
/// carries the member.
#[tokio::test]
async fn query_registrations_lists_the_callers_own_by_the_recorded_subject() {
    let st = state(Arc::new(Owner(true)));
    let path = "/ngsi-ld/v1/csourceRegistrations";
    for n in 0..5 {
        let who = if n % 2 == 0 { "alice" } else { "bob" };
        let mut reg = json!({"id": format!("urn:ngsi-ld:ContextSourceRegistration:{n}"),
                             "type": "ContextSourceRegistration",
                             "information": [{"entities": [{"type": "Car"}]}],
                             "endpoint": "http://127.0.0.1:1"});
        if n == 3 {
            reg["subject"] = json!("alice");
            reg["__subject"] = json!([[SUBJECT, "alice"]]);
        }
        let (code, _, body) = send(&st, who, "POST", path, Some(reg)).await;
        assert_eq!(code, StatusCode::CREATED, "{body}");
    }
    // a 5.9.3 update cannot claim the Registration either
    let (code, _, body) = send(
        &st,
        "bob",
        "PATCH",
        &format!("{path}/urn:ngsi-ld:ContextSourceRegistration:1"),
        Some(json!({"__subject": [[SUBJECT, "alice"]], "registrationName": "renamed"})),
    )
    .await;
    assert_eq!(code, StatusCode::NO_CONTENT, "{body}");

    let alices: Vec<String> = [0, 2, 4]
        .iter()
        .map(|n| format!("urn:ngsi-ld:ContextSourceRegistration:{n}"))
        .collect();
    let list_path = format!("{path}?type=Car&count=true");
    let (code, h, list) = send(&st, "alice", "GET", &list_path, None).await;
    assert_eq!(code, StatusCode::OK, "{list}");
    assert_eq!(ids(&list), alices);
    assert_eq!(count(&h), Some(3));
    let mut paged = Vec::new();
    for offset in 0..4 {
        let uri = format!("{list_path}&limit=1&offset={offset}");
        let (code, h, page) = send(&st, "alice", "GET", &uri, None).await;
        assert_eq!(code, StatusCode::OK, "{page}");
        assert_eq!(count(&h), Some(3));
        paged.extend(ids(&page));
    }
    paged.sort();
    assert_eq!(paged, alices, "no foreign id on any page");

    let (_, h, list) = send(&st, "bob", "GET", &list_path, None).await;
    assert_eq!(count(&h), Some(2), "{list}");
    let (code, _, one) = send(
        &st,
        "bob",
        "GET",
        &format!("{path}/urn:ngsi-ld:ContextSourceRegistration:3"),
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{one}");
    for doc in list.as_array().unwrap().iter().chain([&one]) {
        assert!(
            doc.as_object()
                .unwrap()
                .keys()
                .all(|k| !k.starts_with("__")),
            "a served Registration carries no broker member: {doc}"
        );
    }
}
