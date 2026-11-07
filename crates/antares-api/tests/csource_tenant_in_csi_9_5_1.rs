// SPDX-License-Identifier: EUPL-1.2
//! TS 104 175 V1.1.1 9.5.1: "As Tenant information, if applicable, is
//! directly specified in the CSourceRegistration, it shall not be part of
//! contextSourceInfo."
//!
//! Tenant routing of a forward comes from the registration's `tenant` member
//! alone (ADR-0025 rule 4, threat C3). A registration that tries to carry it
//! in `contextSourceInfo` is refused when it is created or updated, and a row
//! that holds the key anyway (written before this rule, or around the API)
//! never sends it.
#![allow(clippy::unwrap_used)] // an unwrap here is the assertion

use antares_api::AppState;
use antares_model::TenantId;
use antares_store::Kind;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

async fn send(st: &AppState, method: &str, path: &str, body: &Value) -> StatusCode {
    let payload = body.to_string();
    antares_api::router(st.clone())
        .oneshot(
            Request::builder()
                .method(method)
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

fn registration(id: &str, csi: Value) -> Value {
    json!({
        "id": id,
        "type": "ContextSourceRegistration",
        "endpoint": "http://cs.invalid/ngsi-ld/v1",
        "information": [{"entities": [{"type": "Vehicle"}]}],
        "contextSourceInfo": csi,
    })
}

#[tokio::test]
async fn a_tenant_key_in_context_source_info_is_refused() {
    let st = AppState::new("me".into());
    for (i, key) in ["NGSILD-Tenant", "ngsild-tenant", "NGSILD-TENANT"]
        .into_iter()
        .enumerate()
    {
        let id = format!("urn:ngsi-ld:ContextSourceRegistration:spelling-{i}");
        let csi = json!([{"key": key, "value": "other"}]);
        assert_eq!(
            send(
                &st,
                "POST",
                "csourceRegistrations",
                &registration(&id, csi.clone())
            )
            .await,
            StatusCode::BAD_REQUEST,
            "create with contextSourceInfo key {key}"
        );

        // The same registration without the key is stored; adding the key
        // by an update is refused too, and the stored row keeps no such key.
        let clean = json!([{"key": "X-Custom-Info", "value": "fixed"}]);
        assert_eq!(
            send(
                &st,
                "POST",
                "csourceRegistrations",
                &registration(&id, clean.clone())
            )
            .await,
            StatusCode::CREATED
        );
        assert_eq!(
            send(
                &st,
                "PATCH",
                &format!("csourceRegistrations/{id}"),
                &json!({"contextSourceInfo": csi}),
            )
            .await,
            StatusCode::BAD_REQUEST,
            "update with contextSourceInfo key {key}"
        );
        let stored = st
            .store
            .get(&TenantId::new("default").unwrap(), Kind::Registration, &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored["contextSourceInfo"], clean,
            "nothing of the refused update was stored"
        );
    }
}

/// A source that records the head of every request it receives and answers
/// an empty Entity list.
fn recording_source() -> (u16, Arc<Mutex<Vec<String>>>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let sink = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let sink = sink.clone();
            std::thread::spawn(move || {
                use std::io::{Read, Write};
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                sink.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf).to_string());
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\n\
                      Content-Length: 2\r\n\r\n[]",
                );
            });
        }
    });
    (port, seen)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stored_tenant_key_is_never_sent_to_the_source() {
    let st = AppState::new("me".into());
    let (port, seen) = recording_source();
    let id = "urn:ngsi-ld:ContextSourceRegistration:around-the-api";
    // Written around the API, as a row from before the rule would be.
    let mut row = registration(
        id,
        json!([
            {"key": "NGSILD-Tenant", "value": "other"},
            {"key": "X-Custom-Info", "value": "fixed"}
        ]),
    );
    row["endpoint"] = json!(format!("http://127.0.0.1:{port}/ngsi-ld/v1"));
    st.store
        .create(
            &TenantId::new("default").unwrap(),
            Kind::Registration,
            id,
            row,
        )
        .await
        .unwrap();

    let status = antares_api::router(st.clone())
        .oneshot(
            Request::builder()
                .uri("/ngsi-ld/v1/entities?type=Vehicle")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status();
    assert_eq!(status, StatusCode::OK);

    let heads = seen.lock().unwrap().clone();
    assert!(!heads.is_empty(), "the query was forwarded to the source");
    for head in heads {
        let lower = head.to_lowercase();
        assert!(
            lower.contains("x-custom-info: fixed"),
            "other pairs still travel: {head}"
        );
        assert!(
            !lower.contains("ngsild-tenant"),
            "the tenant key never travels: {head}"
        );
    }
}
