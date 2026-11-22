// SPDX-License-Identifier: EUPL-1.2
//! S4-17 (T-7547): a Channel acts as who bound it is now. It keeps no role,
//! group or other header value of the bind request to act with later. Its own
//! process: it names the subject headers, which are read once per process.

use antares_api::channels::{Bridge, Channels, Loopback};
use antares_api::policy::{
    Decision, DecisionFuture, NotifyDecision, Operation, PolicyEngine, Subject,
};
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;

const BUS: &str = "urn:ngsi-ld:Bus:subject";

/// Allows everything and keeps the subject of every Append Attributes.
#[derive(Default)]
struct Seen(Mutex<Vec<Vec<(String, String)>>>);

impl PolicyEngine for Seen {
    fn name(&self) -> &str {
        "seen"
    }

    fn decide<'a>(&'a self, s: &'a Subject, op: &'a Operation<'a>) -> DecisionFuture<'a> {
        if op.clause == "5.6.3" {
            self.0.lock().expect("seen").push(s.headers.clone());
        }
        Box::pin(std::future::ready(Decision::Allow))
    }

    fn pre_notify(&self, _s: &Subject, _sub: &Value, _n: &mut Value) -> NotifyDecision {
        NotifyDecision::Deliver
    }
}

async fn send(st: &AppState, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let body = body.to_string();
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("content-length", body.len())
        .header("x-jc-subject", "ana")
        .header("x-jc-roles", "writer")
        .body(Body::from(body))
        .expect("request");
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_sample_carries_no_stored_role_of_the_binder() {
    antares_api::policy::default_subject_headers("x-jc-subject,x-jc-roles");
    let lo = Arc::new(Loopback::new());
    let bridges: HashMap<String, Arc<dyn Bridge>> =
        HashMap::from([("lo".to_owned(), Arc::clone(&lo) as Arc<dyn Bridge>)]);
    let mut st = AppState::new("channels-subject".into())
        .with_surface(Box::new(Channels::new(bridges)))
        .expect("mount");
    let engine = Arc::new(Seen::default());
    st.policy = Some(engine.clone());
    antares_api::wire(&mut st).await;
    let entity = json!({"id": BUS, "type": "Bus", "speed": {"type": "Property", "value": 0}});
    let (status, body) = send(&st, "POST", "/ngsi-ld/v1/entities", entity).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let channel = json!({"entity": BUS, "attribute": "speed", "broker": "lo",
                         "topic": "bus/s/speed", "retention": "mirror"});
    let (status, body) = send(&st, "PUT", "/x/channels/s", channel).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    engine.0.lock().expect("seen").clear();
    lo.publish("bus/s/speed", "7");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let acted = loop {
        let seen = engine.0.lock().expect("seen").clone();
        if !seen.is_empty() || tokio::time::Instant::now() > deadline {
            break seen;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(!acted.is_empty(), "the sample was never decided");
    assert!(
        acted.iter().flatten().all(|(name, _)| name != "x-jc-roles"),
        "{acted:?}"
    );
}
