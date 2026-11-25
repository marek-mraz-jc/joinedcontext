// SPDX-License-Identifier: EUPL-1.2
//! 5.7.5 to 5.7.10 over drivers that do less than the seam allows: one that
//! cannot page, one that ignores the keyset cursor, one that applies no
//! selection at all and leaves the judging to the caller, and one that keeps
//! no type list. Each folds the Tenant itself, and every discovery answer is
//! the one the full store gives.
#![cfg(feature = "test-kit")]

use crate::common::{Double, Lesser};
use antares_api::AppState;
use antares_sql::store::any::AnyStore;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

async fn send(st: &AppState, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let b = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => {
            let payload = v.to_string();
            b.header("Content-Type", "application/json")
                .header("Content-Length", payload.len())
                .body(Body::from(payload))
        }
        None => b.body(Body::empty()),
    }
    .expect("request");
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let code = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// The answer without the list's minted id, which differs per call.
fn comparable(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        o.remove("id");
    }
    v
}

const READS: [&str; 7] = [
    "/ngsi-ld/v1/types",
    "/ngsi-ld/v1/types?details=true",
    "/ngsi-ld/v1/types/Car",
    "/ngsi-ld/v1/types/Bike",
    "/ngsi-ld/v1/attributes",
    "/ngsi-ld/v1/attributes?details=true",
    "/ngsi-ld/v1/attributes/speed",
];

#[tokio::test(flavor = "multi_thread")]
async fn every_lesser_driver_answers_discovery_as_the_full_store() {
    let inner = Arc::new(AnyStore::Mem(Default::default()));
    let full = AppState::with_store("me".into(), inner.clone(), "memory");
    for i in 0..7 {
        let car = json!({"id": format!("urn:ngsi-ld:Car:{i}"), "type": "Car",
                         "speed": {"type": "Property", "value": i},
                         "owner": {"type": "Relationship", "object": "urn:ngsi-ld:P:1"}});
        let (code, body) = send(&full, "POST", "/ngsi-ld/v1/entities", Some(car)).await;
        assert_eq!(code, StatusCode::CREATED, "{body}");
    }
    for i in 0..3 {
        let bike = json!({"id": format!("urn:ngsi-ld:Bike:{i}"), "type": ["Bike", "Car"],
                          "colour": {"type": "Property", "value": "red"}});
        let (code, body) = send(&full, "POST", "/ngsi-ld/v1/entities", Some(bike)).await;
        assert_eq!(code, StatusCode::CREATED, "{body}");
    }
    let mut want = Vec::new();
    for uri in READS {
        let (code, body) = send(&full, "GET", uri, None).await;
        assert_eq!(code, StatusCode::OK, "{uri}: {body}");
        want.push(comparable(body));
    }
    let lesser = [
        ("unpaging", Double::unpaging(inner.clone())),
        ("no keyset", Double::lesser(inner.clone(), Lesser::NoKeyset)),
        (
            "undecided",
            Double::lesser(inner.clone(), Lesser::Undecided),
        ),
        (
            "no type names",
            Double::lesser(inner.clone(), Lesser::NoTypeNames),
        ),
    ];
    for (name, driver) in lesser {
        let st = AppState::with_drivers("me".into(), Arc::new(driver), inner.clone(), "memory");
        for (uri, want) in READS.iter().zip(&want) {
            let (code, body) = send(&st, "GET", uri, None).await;
            assert_eq!(code, StatusCode::OK, "{name} {uri}: {body}");
            assert_eq!(&comparable(body), want, "{name} {uri}");
        }
    }
}
