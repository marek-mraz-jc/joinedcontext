// SPDX-License-Identifier: EUPL-1.2
//! `attributeNames` scopes a registration (TS 104 175 9.4.2, Table
//! 5.2.6.6.5-1, T-3451): "the specified Attribute identifiers shall match
//! the attributeNames (or the combination of propertyNames and
//! relationshipNames) in the RegistrationInfo", and the old pair is "only
//! used if attributeNames is not present". A registration scoped by
//! `attributeNames` alone is neither listed for nor forwarded a query about
//! another Attribute.

use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

const REG: &str = "urn:ngsi-ld:ContextSourceRegistration:names";

async fn send(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => {
            let payload = v.to_string();
            b = b
                .header("Content-Type", "application/json")
                .header("Content-Length", payload.len());
            b.body(Body::from(payload)).expect("request")
        }
        None => b.body(Body::empty()).expect("request"),
    };
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

/// A Context Source answering every request with one Vehicle, counting the
/// requests it is sent.
fn source() -> (u16, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&asked);
    let body = json!([{"id": "urn:ngsi-ld:Vehicle:remote", "type": "Vehicle",
                       "speed": {"type": "Property", "value": 5}}])
    .to_string();
    let reply = format!(
        "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            use std::io::{Read, Write};
            let mut buf = [0u8; 8192];
            let _ = s.read(&mut buf);
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = s.write_all(reply.as_bytes());
        }
    });
    (port, asked)
}

async fn register(st: &AppState, info: Value, port: u16) {
    let (code, body) = send(
        st,
        "POST",
        "/ngsi-ld/v1/csourceRegistrations",
        Some(json!({
            "id": REG,
            "type": "ContextSourceRegistration",
            "information": [info],
            "endpoint": format!("http://127.0.0.1:{port}"),
        })),
    )
    .await;
    assert_eq!(code, StatusCode::CREATED, "{body}");
}

async fn listed(st: &AppState, attrs: &str) -> Vec<String> {
    let (code, body) = send(
        st,
        "GET",
        &format!("/ngsi-ld/v1/csourceRegistrations?type=Vehicle&attrs={attrs}"),
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    body.as_array()
        .expect("a list")
        .iter()
        .filter_map(|r| r["id"].as_str().map(str::to_owned))
        .collect()
}

/// 9.4.2: discovery and the federated query both read `attributeNames`.
#[tokio::test(flavor = "multi_thread")]
async fn clause_9_4_2_attribute_names_scope_registration_matching() {
    antares_jsonld::allow_private_egress(true);
    let st = AppState::new("csr-names".into());
    let (port, asked) = source();
    register(
        &st,
        json!({"entities": [{"type": "Vehicle"}], "attributeNames": ["speed"]}),
        port,
    )
    .await;
    assert!(
        listed(&st, "brand").await.is_empty(),
        "matched an Attribute it never registered"
    );
    assert_eq!(listed(&st, "speed").await, [REG]);

    let (code, body) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Vehicle&attrs=brand",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(
        asked.load(Ordering::SeqCst),
        0,
        "a query about brand was forwarded: {body}"
    );
    let (code, body) = send(
        &st,
        "GET",
        "/ngsi-ld/v1/entities?type=Vehicle&attrs=speed",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert!(
        asked.load(Ordering::SeqCst) > 0,
        "a query about speed was not forwarded"
    );

    // the registration reads back as it was written
    let (code, body) = send(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/csourceRegistrations/{REG}"),
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(
        body["information"][0]["attributeNames"],
        json!(["speed"]),
        "{body}"
    );
}

/// Table 5.2.6.6.5-1: `attributeNames` is a non-empty array of names.
#[tokio::test(flavor = "multi_thread")]
async fn clause_5_2_6_6_5_empty_attribute_names_is_400() {
    let st = AppState::new("csr-names-400".into());
    for names in [json!([]), json!("speed"), json!([1])] {
        let (code, body) = send(
            &st,
            "POST",
            "/ngsi-ld/v1/csourceRegistrations",
            Some(json!({
                "type": "ContextSourceRegistration",
                "information": [{"entities": [{"type": "Vehicle"}], "attributeNames": names}],
                "endpoint": "http://cs.example.org:1026",
            })),
        )
        .await;
        assert_eq!(
            code,
            StatusCode::BAD_REQUEST,
            "attributeNames {names}: {body}"
        );
    }
}

/// `attributeNames` wins over the deprecated pair when both are present.
#[tokio::test(flavor = "multi_thread")]
async fn attribute_names_take_precedence_over_property_names() {
    let st = AppState::new("csr-names-both".into());
    register(
        &st,
        json!({"entities": [{"type": "Vehicle"}], "attributeNames": ["speed"],
               "propertyNames": ["brand"]}),
        9,
    )
    .await;
    assert!(
        listed(&st, "brand").await.is_empty(),
        "the deprecated pair was read beside attributeNames"
    );
    assert_eq!(listed(&st, "speed").await, [REG]);
}

/// A registration with only the old pair matches as before.
#[tokio::test(flavor = "multi_thread")]
async fn the_old_pair_alone_still_scopes() {
    let st = AppState::new("csr-names-old".into());
    register(
        &st,
        json!({"entities": [{"type": "Vehicle"}], "propertyNames": ["speed"],
               "relationshipNames": ["owner"]}),
        9,
    )
    .await;
    assert!(listed(&st, "brand").await.is_empty());
    assert_eq!(listed(&st, "owner").await, [REG]);
}
