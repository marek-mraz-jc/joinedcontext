// SPDX-License-Identifier: EUPL-1.2
//! TS 104 175 Table 5.2.6.5.2-1 marks `notificationTrigger`, `q`,
//! `expandValues`, `jsonKeys`, `lang`, `localOnly`, `splitEntities` and
//! `throttling` "not applicable and shall be ignored" for a Context Source
//! Registration subscription: they are accepted, and change nothing.
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn post(st: &AppState, path: &str, body: Value) -> StatusCode {
    let payload = body.to_string();
    antares_api::router(st.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/ngsi-ld/v1/{path}"))
                .header("Content-Type", "application/json")
                .header("Content-Length", payload.len())
                .body(Body::from(payload))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// A capture endpoint yielding the body of each notification POST.
async fn capture() -> (String, tokio::sync::mpsc::Receiver<Value>) {
    let (tx, rx) = tokio::sync::mpsc::channel::<Value>(8);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/notify",
        axum::routing::post(move |body: axum::body::Bytes| {
            let tx = tx.clone();
            async move {
                let _ = tx
                    .send(serde_json::from_slice(&body).unwrap_or(Value::Null))
                    .await;
                StatusCode::OK
            }
        }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/notify"), rx)
}

/// The registration ids a notification carries.
fn registrations(n: &Value) -> Vec<String> {
    n["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["id"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clause_5_2_6_5_2_q_and_throttling_ignored_for_csource_subscriptions() {
    antares_jsonld::allow_private_egress(true);
    let mut st = AppState::new("me".into());
    antares_api::wire(&mut st).await;
    let (uri, mut rx) = capture().await;
    let status = post(
        &st,
        "csourceSubscriptions",
        json!({
            "id": "urn:ngsi-ld:Subscription:ignored-members",
            "type": "Subscription",
            "entities": [{"type": "Vehicle"}],
            "q": "nothing==\"matches\"",
            "throttling": 3600,
            "notification": {"endpoint": {"uri": uri}},
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let mut seen: Vec<String> = Vec::new();
    for i in 0..2 {
        let id = format!("urn:ngsi-ld:ContextSourceRegistration:ignored-{i}");
        let status = post(
            &st,
            "csourceRegistrations",
            json!({
                "id": id,
                "type": "ContextSourceRegistration",
                "information": [{"entities": [{"type": "Vehicle"}]}],
                "endpoint": "http://127.0.0.1:9",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "registration {i}");
        let deadline = std::time::Duration::from_secs(10 * antares_api::state::slow_factor());
        while !seen.contains(&id) {
            match tokio::time::timeout(deadline, rx.recv()).await {
                Ok(Some(n)) => seen.extend(registrations(&n)),
                _ => {
                    panic!("registration {i} was not notified (q or throttling applied): {seen:?}")
                }
            }
        }
    }
}
