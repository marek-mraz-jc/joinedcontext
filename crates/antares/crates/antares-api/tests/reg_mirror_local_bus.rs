// SPDX-License-Identifier: EUPL-1.2
//! The registration mirror under `ANTARES_BUS=local` (T-5499).
//!
//! Every NGSI-LD query that may federate (5.7) asks which Context Source
//! Registrations match it. Without a mirror that question is a store read
//! per request: on Postgres a transaction, a `set_config` and a SELECT over
//! `csource_registrations`, millions of times a day on dev for a table of
//! three rows. bus=nats hydrated a mirror; bus=local, the single-process
//! deployment dev and prod run, did not. These tests pin that a wired local
//! broker answers from the mirror and that the mirror follows registration
//! writes at once, without a restart.
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use crate::common;

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::Double;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const REMOTE: &str = "urn:ngsi-ld:Vehicle:remote";

async fn call(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let mut req = Request::builder().method(method).uri(uri);
    if !payload.is_empty() {
        req = req
            .header("Content-Type", "application/json")
            .header("Content-Length", payload.len());
    }
    let resp = antares_api::router(st.clone())
        .oneshot(req.body(Body::from(payload)).expect("request"))
        .await
        .expect("response");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A Context Source holding one Vehicle, served on an ephemeral port.
async fn source() -> u16 {
    antares_jsonld::allow_private_egress(true);
    let peer = AppState::new("t5499-source".into());
    let (status, _) = call(
        &peer,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(json!({"id": REMOTE, "type": "Vehicle",
                    "speed": {"type": "Property", "value": 1}})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let router = antares_api::router(peer);
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    port
}

async fn register(st: &AppState, port: u16) -> String {
    let id = format!("urn:ngsi-ld:ContextSourceRegistration:t5499-{port}");
    let (status, body) = call(
        st,
        "POST",
        "/ngsi-ld/v1/csourceRegistrations",
        Some(json!({"id": id, "type": "ContextSourceRegistration",
                    "endpoint": format!("http://127.0.0.1:{port}"),
                    "information": [{"entities": [{"type": "Vehicle"}]}]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    id
}

/// The broker as `antares_broker::run` wires it for bus=local, over a store
/// double that counts registration reads.
async fn wired(st: &mut AppState) -> Arc<Double> {
    let double = Arc::new(Double::passthrough(st.store.clone()));
    st.store = double.clone();
    antares_api::wire(st).await;
    double
}

async fn finds_remote(st: &AppState) -> bool {
    let (status, body) = call(st, "GET", "/ngsi-ld/v1/entities?type=Vehicle", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body.as_array()
        .expect("array")
        .iter()
        .any(|e| e["id"] == json!(REMOTE))
}

/// A registration stored before boot is hydrated into the mirror, and the
/// queries after boot never read the registration store.
#[tokio::test]
async fn a_local_bus_matches_registrations_from_the_mirror() {
    let port = source().await;
    let mut st = AppState::new("t5499-home".into());
    register(&st, port).await;
    let double = wired(&mut st).await;
    assert!(
        finds_remote(&st).await,
        "the hydrated registration must forward"
    );
    assert!(finds_remote(&st).await, "and again on the next query");
    assert_eq!(
        double.registration_reads(),
        0,
        "a query must not read the registration store"
    );
}

/// A registration created through the API after boot is matched by the next
/// query, with no restart and no store read.
#[tokio::test]
async fn a_registration_written_after_boot_is_matched_at_once() {
    let port = source().await;
    let mut st = AppState::new("t5499-home".into());
    let double = wired(&mut st).await;
    assert!(!finds_remote(&st).await, "nothing is registered yet");
    register(&st, port).await;
    assert!(
        finds_remote(&st).await,
        "the new registration must forward at once"
    );
    assert_eq!(double.registration_reads(), 0);
}

/// A registration deleted through the API stops forwarding at once: the
/// mirror drops it with the store row.
#[tokio::test]
async fn a_deleted_registration_no_longer_forwards() {
    let port = source().await;
    let mut st = AppState::new("t5499-home".into());
    wired(&mut st).await;
    let id = register(&st, port).await;
    assert!(finds_remote(&st).await);
    let (status, body) = call(
        &st,
        "DELETE",
        &format!("/ngsi-ld/v1/csourceRegistrations/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(
        !finds_remote(&st).await,
        "a deleted registration must not forward"
    );
}
