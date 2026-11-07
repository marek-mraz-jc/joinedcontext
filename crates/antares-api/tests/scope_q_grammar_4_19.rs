// SPDX-License-Identifier: EUPL-1.2
//! 4.19: every operation that takes a `scopeQ` refuses one outside the
//! grammar (`/a,,/b`, `/a;;/b`, `//a`, and since forge 019_03_07 a bare
//! conjunction `/a;/b` or a level not starting with a letter, TS 104 175
//! 7.2.5) with 400 BadRequestData before it
//! reads anything, in the URI, a Query body and a Subscription alike; a
//! well-formed one (`/Madrid/#`) is still answered (T-3959).

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const BAD: [&str; 5] = [
    "/a,,/b",
    "/a;;/b",
    "//a",
    "/Madrid;/CompanyA",
    "/Madrid/1st",
];
const GOOD: &str = "/Madrid/#";

async fn send(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(b) => {
            let b = b.to_string();
            req = req
                .header("Content-Type", "application/json")
                .header("Content-Length", b.len());
            Body::from(b)
        }
        None => Body::empty(),
    };
    let res = antares_api::router(st.clone())
        .oneshot(req.body(body).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn enc(s: &str) -> String {
    s.replace('/', "%2F")
        .replace(',', "%2C")
        .replace(';', "%3B")
        .replace('#', "%23")
}

/// The URI operations, each with a scopeQ slot.
fn uris(q: &str) -> Vec<String> {
    let q = enc(q);
    vec![
        format!("/ngsi-ld/v1/entities?type=T&scopeQ={q}"),
        format!(
            "/ngsi-ld/v1/temporal/entities?type=T&timerel=after&timeAt=2026-01-01T00:00:00Z&scopeQ={q}"
        ),
        format!("/ngsi-ld/v1/csourceRegistrations?type=T&scopeQ={q}"),
    ]
}

/// The body operations: a Query (entities and temporal) and a Subscription.
fn bodies(q: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "/ngsi-ld/v1/entityOperations/query",
            json!({"type": "Query", "entities": [{"type": "T"}], "scopeQ": q}),
        ),
        (
            "/ngsi-ld/v1/temporal/entityOperations/query",
            json!({"type": "Query", "entities": [{"type": "T"}], "scopeQ": q,
                   "temporalQ": {"timerel": "after", "timeAt": "2026-01-01T00:00:00Z"}}),
        ),
        (
            "/ngsi-ld/v1/subscriptions",
            json!({"type": "Subscription", "entities": [{"type": "T"}], "scopeQ": q,
                   "notification": {"endpoint": {"uri": "http://localhost:1/n"}}}),
        ),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_scope_q_is_refused_at_every_entry() {
    let st = AppState::new("me".into());
    for bad in BAD {
        for uri in uris(bad) {
            let (status, body) = send(&st, "GET", &uri, None).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
            assert!(
                body["type"]
                    .as_str()
                    .is_some_and(|t| t.ends_with("BadRequestData")),
                "{uri}: {body}"
            );
        }
        for (uri, doc) in bodies(bad) {
            let (status, body) = send(&st, "POST", uri, Some(doc)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{uri} {bad}: {body}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_well_formed_scope_q_is_still_answered() {
    let st = AppState::new("me".into());
    for uri in uris(GOOD) {
        let (status, body) = send(&st, "GET", &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    }
    for (uri, doc) in bodies(GOOD) {
        let (status, body) = send(&st, "POST", uri, Some(doc)).await;
        // TS 104 176 7.12.3.1 / 8.x query: a valid query answers 200;
        // 7.13.3.2: a created Subscription answers 201
        let want = if uri.ends_with("/subscriptions") {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        };
        assert_eq!(status, want, "{uri}: {body}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subscription_patch_and_a_non_string_scope_q_are_refused() {
    let st = AppState::new("me".into());
    let (_, doc) = bodies(GOOD).swap_remove(2);
    let (status, body) = send(&st, "POST", "/ngsi-ld/v1/subscriptions", Some(doc)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (_, list) = send(&st, "GET", "/ngsi-ld/v1/subscriptions", None).await;
    let id = list[0]["id"].as_str().expect("subscription id").to_owned();
    let uri = format!("/ngsi-ld/v1/subscriptions/{id}");
    let (status, body) = send(&st, "PATCH", &uri, Some(json!({"scopeQ": "/a,,/b"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (_, doc) = bodies(GOOD).swap_remove(2);
    let mut doc = doc;
    doc["scopeQ"] = json!(["/a"]);
    let (status, body) = send(&st, "POST", "/ngsi-ld/v1/subscriptions", Some(doc)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}
