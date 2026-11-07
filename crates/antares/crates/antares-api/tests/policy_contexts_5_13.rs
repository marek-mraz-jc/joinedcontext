// SPDX-License-Identifier: EUPL-1.2
//! List and Serve @context under a narrowing engine (ADR-0023 rule 5,
//! T-4009): a stored `@context` may define a term for an Attribute the
//! caller may not see, so a narrowed caller lists the core alone and does
//! not find a stored one by its URL.
#![cfg(feature = "test-kit")]
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use antares_api::policy::{
    Decision, DecisionFuture, Filter, NotifyDecision, Operation, PolicyEngine, Rule, Subject,
    NAME_READS,
};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

/// Allows everything, and narrows the @context reads when `narrow`.
struct Names {
    narrow: bool,
}

impl PolicyEngine for Names {
    fn name(&self) -> &str {
        "names"
    }

    fn decide<'a>(&'a self, _s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        Box::pin(std::future::ready(
            if self.narrow && NAME_READS.contains(&op.clause) {
                Decision::Filter(Filter {
                    rules: vec![Rule::default()],
                    ..Filter::default()
                })
            } else {
                Decision::Allow
            },
        ))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn call(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(uri);
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
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A broker under `engine` holding one hosted @context; its local id.
async fn holding(narrow: bool) -> (AppState, String) {
    let st = AppState::new("me".into()).with_policy(Arc::new(Names { narrow }));
    let payload = json!({"@context": {"secret": "https://example.org/secret"}}).to_string();
    let req = Request::builder()
        .method("POST")
        .uri("/ngsi-ld/v1/jsonldContexts")
        .header("Content-Type", "application/json")
        .header("Content-Length", payload.len())
        .body(Body::from(payload))
        .unwrap();
    let resp = antares_api::router(st.clone()).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let loc = resp.headers()["location"].to_str().unwrap().to_owned();
    (st, loc.rsplit('/').next().unwrap().to_owned())
}

fn urls(list: &Value) -> Vec<String> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| u.as_str().or_else(|| u["url"].as_str()))
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn a_narrowed_caller_lists_the_core_alone_and_finds_no_stored_context() {
    let (st, id) = holding(true).await;
    for uri in [
        "/ngsi-ld/v1/jsonldContexts",
        "/ngsi-ld/v1/jsonldContexts?details=true",
    ] {
        let (status, list) = call(&st, "GET", uri, None).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {list}");
        assert!(
            !list.to_string().contains("secret") && !list.to_string().contains(&id),
            "{list}"
        );
        assert!(
            urls(&list).iter().all(|u| u.contains("uri.etsi.org")),
            "{list}"
        );
    }
    for uri in [
        format!("/ngsi-ld/v1/jsonldContexts/{id}"),
        format!("/ngsi-ld/v1/jsonldContexts/{id}?details=true"),
    ] {
        let (status, body) = call(&st, "GET", &uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
        assert!(!body.to_string().contains("secret"), "{body}");
    }
}

#[tokio::test]
async fn an_unnarrowed_caller_lists_and_serves_the_stored_context() {
    let (st, id) = holding(false).await;
    let (status, list) = call(&st, "GET", "/ngsi-ld/v1/jsonldContexts", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(urls(&list).iter().any(|u| u.ends_with(&id)), "{list}");
    let (status, body) = call(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/jsonldContexts/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["@context"]["secret"], "https://example.org/secret");
}
