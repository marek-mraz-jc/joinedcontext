// SPDX-License-Identifier: EUPL-1.2
//! `POST /q/views/{id}/preview` runs under the subject headers of the `/q` request (T-3813):
//! the Portal, holding the admin credential, names whose reading it previews, and a request that
//! names nobody is the cluster's own caller, as on every other path. One test in its own binary,
//! because it sets the subject header list in the environment before the broker first reads it.

use antares_api::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

/// Allows the caller named `alice` and refuses everyone else, the request with no subject
/// header included.
struct OnlyAlice;

impl PolicyEngine for OnlyAlice {
    fn name(&self) -> &'static str {
        "only-alice"
    }
    fn decide<'a>(&'a self, s: &'a Subject, _: &'a Operation<'a>) -> DecisionFuture<'a> {
        let alice = s
            .headers
            .iter()
            .any(|(k, v)| k == "x-subject" && v == "alice");
        let d = if alice {
            Decision::Allow
        } else {
            Decision::Deny("not alice".into())
        };
        Box::pin(std::future::ready(d))
    }
    fn pre_notify(&self, _: &Subject, _: &Value, _: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn send(app: &axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let Ok(resp) = app.clone().oneshot(req).await;
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn post(uri: &str, body: &Value, headers: &[(&str, &str)]) -> Request<Body> {
    let text = body.to_string();
    let mut req = Request::post(uri)
        .header("Content-Type", "application/json")
        .header("Content-Length", text.len());
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.body(Body::from(text)).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_preview_runs_under_the_subject_the_request_names() {
    // Before anything reads the list: the broker carries `x-subject` to the engine.
    std::env::set_var("ANTARES_POLICY_SUBJECT_HEADERS", "x-subject");
    let mut state = AppState::new("antares-test".into());
    antares_api::wire(&mut state).await;
    let open = antares_api::router(state.clone());
    let car = json!({"id": "urn:ngsi-ld:Car:1", "type": "Car",
                     "speed": {"type": "Property", "value": 10}});
    let (s, b) = send(
        &open,
        post("/ngsi-ld/v1/entities", &car, &[("NGSILD-Tenant", "city")]),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let state = state.with_policy(Arc::new(OnlyAlice));
    let _reload = antares_ops::views::install(&state)
        .await
        .expect("installed");
    let app = antares_api::router(
        state
            .with_surfaces(vec![Box::new(antares_api::Admin)])
            .expect("admin"),
    );
    let draft = json!({"id": "v", "tenant": "city", "selector": ["Car"],
                       "rules": [{"types": ["Car"], "attributes": ["speed"]}],
                       "operations": ["query_entities"]});
    let uri = "/q/views/v/preview?type=Car";

    let (s, b) = send(&app, post(uri, &draft, &[("x-subject", "alice")])).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["count"], 1, "{b}");
    let (s, b) = send(&app, post(uri, &draft, &[("x-subject", "bob")])).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{b}");
    let (s, b) = send(&app, post(uri, &draft, &[])).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "no subject is nobody's reading: {b}"
    );
}
