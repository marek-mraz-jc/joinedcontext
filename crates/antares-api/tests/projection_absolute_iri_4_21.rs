// SPDX-License-Identifier: EUPL-1.2
//! 4.21 projections name Attributes, and an expanded IRI is a valid Attribute
//! name in a request (5.5.7): `pick` and `omit` take an absolute IRI holding
//! dots as one name. The dotted path stays a short name's syntax only, so
//! `speed.value` still projects `speed` (T-3660).

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const ID: &str = "urn:ngsi-ld:City:p1";
const IRI: &str = "https://example.org/city/speed";

async fn send(st: &AppState, req: Request<Body>) -> (StatusCode, Value) {
    let res = antares_api::router(st.clone())
        .oneshot(req)
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

async fn get(st: &AppState, uri: &str) -> Value {
    let (status, body) = send(
        st,
        Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

async fn seeded() -> AppState {
    let mut st = AppState::new("me".into());
    antares_api::wire(&mut st).await;
    let body = json!({"id": ID, "type": "City",
        IRI: {"type": "Property", "value": 50},
        "speed": {"type": "Property", "value": 10}})
    .to_string();
    let (status, b) = send(
        &st,
        Request::builder()
            .method("POST")
            .uri("/ngsi-ld/v1/entities")
            .header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "seed: {b}");
    st
}

/// The entity of a Retrieve answer, or the one entity of a Query answer.
fn entity(body: &Value) -> &Value {
    body.as_array().and_then(|a| a.first()).unwrap_or(body)
}

fn attributes(e: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = e
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    keys.retain(|k| !["id", "type", "@context"].contains(k));
    keys.sort_unstable();
    keys
}

fn urls() -> [String; 2] {
    [
        format!("/ngsi-ld/v1/entities/{ID}?"),
        "/ngsi-ld/v1/entities?type=City&".to_owned(),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn pick_takes_an_absolute_iri_with_dots_as_one_attribute() {
    let st = seeded().await;
    for base in urls() {
        let body = get(&st, &format!("{base}pick={IRI}")).await;
        assert_eq!(attributes(entity(&body)), [IRI], "{base}: {body}");
        assert_eq!(entity(&body)[IRI]["value"], 50, "{base}: {body}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn omit_takes_an_absolute_iri_with_dots_as_one_attribute() {
    let st = seeded().await;
    for base in urls() {
        let body = get(&st, &format!("{base}omit={IRI}")).await;
        assert_eq!(attributes(entity(&body)), ["speed"], "{base}: {body}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_short_dotted_name_is_still_a_path() {
    let st = seeded().await;
    for base in urls() {
        let body = get(&st, &format!("{base}pick=speed.value")).await;
        assert_eq!(attributes(entity(&body)), ["speed"], "{base}: {body}");
        let body = get(&st, &format!("{base}omit=speed.value")).await;
        assert_eq!(attributes(entity(&body)), [IRI], "{base}: {body}");
    }
}
