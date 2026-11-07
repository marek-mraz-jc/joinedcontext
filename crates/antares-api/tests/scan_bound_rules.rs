// SPDX-License-Identifier: EUPL-1.2
//! F-ANT2-010: a page under conditional rules reads the store in pages until
//! the client's window holds; rules that admit nothing never fill it, so the
//! walk is bounded by MAX_FOLD_DOCS like a `count=true` walk, and answers
//! TooManyResults (5.5.6) past it instead of reading the whole tenant. Own
//! process: sets ANTARES_DISCOVERY_SCAN_MAX before the bound is first read.
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

const SCAN_MAX: usize = 40;

/// Narrows every read to Cars faster than any seeded one.
struct NoneAdmitted;

impl PolicyEngine for NoneAdmitted {
    fn name(&self) -> &str {
        "none-admitted"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        let answer = if antares_api::policy::FILTERABLE.contains(&op.clause) {
            Decision::Filter(Filter {
                rules: vec![Rule {
                    types: vec!["https://uri.etsi.org/ngsi-ld/default-context/Car".into()],
                    q: Some(Box::new(antares_ql::parse_q("speed>1000").unwrap())),
                    every_attribute: true,
                    ..Rule::default()
                }],
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
    let code = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_page_under_rules_that_admit_nothing_stops_at_the_scan_bound() {
    std::env::set_var("ANTARES_DISCOVERY_SCAN_MAX", SCAN_MAX.to_string());
    let mut st = AppState::new("me".into()).with_policy(Arc::new(NoneAdmitted));
    antares_api::wire(&mut st).await;
    let cars: Vec<Value> = (0..2 * SCAN_MAX)
        .map(|i| {
            json!({"id": format!("urn:ngsi-ld:Car:{i:03}"), "type": "Car",
                   "speed": {"type": "Property", "value": i}})
        })
        .collect();
    let (code, body) = send(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/create",
        Some(Value::Array(cars)),
    )
    .await;
    assert!(code.is_success(), "{code} {body}");
    let (code, body) = send(&st, "GET", "/ngsi-ld/v1/entities?type=Car&limit=1", None).await;
    assert_eq!(code, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["title"], "TooManyResults", "{body}");
}
