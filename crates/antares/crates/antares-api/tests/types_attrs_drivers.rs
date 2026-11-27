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

/// The next-Link target of a reply, as the client follows it.
async fn page_after(st: &AppState, uri: &str) -> (Value, Option<String>) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .expect("request");
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    assert_eq!(resp.status(), StatusCode::OK, "{uri}");
    let next = resp
        .headers()
        .get_all(axum::http::header::LINK)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|l| l.contains("rel=\"next\""))
        .and_then(|l| {
            l.split_once('<')?
                .1
                .split_once('>')
                .map(|(t, _)| t.to_owned())
        });
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    (serde_json::from_slice(&bytes).unwrap_or(Value::Null), next)
}

/// 6.3.10: a driver that cannot page still serves every page of a listing,
/// the next Link walking it one Entity at a time in id order.
#[tokio::test(flavor = "multi_thread")]
async fn a_listing_pages_through_a_driver_that_cannot_page() {
    let inner = Arc::new(AnyStore::Mem(Default::default()));
    let full = AppState::with_store("me".into(), inner.clone(), "memory");
    for i in 0..3 {
        let car = json!({"id": format!("urn:ngsi-ld:Car:{i}"), "type": "Car"});
        let (code, body) = send(&full, "POST", "/ngsi-ld/v1/entities", Some(car)).await;
        assert_eq!(code, StatusCode::CREATED, "{body}");
    }
    let st = AppState::with_drivers(
        "me".into(),
        Arc::new(Double::unpaging(inner.clone())),
        inner.clone(),
        "memory",
    );
    let mut uri = "/ngsi-ld/v1/entities?type=Car&limit=1".to_owned();
    let mut ids = Vec::new();
    for _ in 0..3 {
        let (page, next) = page_after(&st, &uri).await;
        assert_eq!(page.as_array().map(Vec::len), Some(1), "{page}");
        ids.push(page[0]["id"].as_str().map(str::to_owned));
        match next {
            Some(n) => uri = n,
            None => break,
        }
    }
    let want: Vec<_> = (0..3)
        .map(|i| Some(format!("urn:ngsi-ld:Car:{i}")))
        .collect();
    assert_eq!(ids, want);
}

/// 5.7.2: a listing re-decides every row a driver hands over undecided, so
/// the type, the Attribute selection and the id list give the full store's
/// answer; a pick that matches nothing on an Entity leaves it out.
#[tokio::test(flavor = "multi_thread")]
async fn a_listing_over_a_driver_that_decides_nothing_matches_the_full_store() {
    let inner = Arc::new(AnyStore::Mem(Default::default()));
    let full = AppState::with_store("me".into(), inner.clone(), "memory");
    for doc in [
        json!({"id": "urn:ngsi-ld:Car:1", "type": "Car",
               "speed": {"type": "Property", "value": 1}}),
        json!({"id": "urn:ngsi-ld:Car:2", "type": "Car",
               "colour": {"type": "Property", "value": "red"}}),
        json!({"id": "urn:ngsi-ld:Bike:1", "type": "Bike",
               "speed": {"type": "Property", "value": 3}}),
    ] {
        let (code, body) = send(&full, "POST", "/ngsi-ld/v1/entities", Some(doc)).await;
        assert_eq!(code, StatusCode::CREATED, "{body}");
    }
    let st = AppState::with_drivers(
        "me".into(),
        Arc::new(Double::lesser(inner.clone(), Lesser::Undecided)),
        inner.clone(),
        "memory",
    );
    let ids = |body: &Value| -> Vec<String> {
        let mut ids: Vec<String> = body
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["id"].as_str().map(str::to_owned))
            .collect();
        ids.sort();
        ids
    };
    for uri in [
        "/ngsi-ld/v1/entities?type=Car",
        "/ngsi-ld/v1/entities?type=Car&attrs=speed",
        "/ngsi-ld/v1/entities?type=Car,Bike&attrs=speed",
        "/ngsi-ld/v1/entities?type=Car&id=urn:ngsi-ld:Car:2",
    ] {
        let (code, want) = send(&full, "GET", uri, None).await;
        assert_eq!(code, StatusCode::OK, "{uri}: {want}");
        let (code, body) = send(&st, "GET", uri, None).await;
        assert_eq!(code, StatusCode::OK, "{uri}: {body}");
        assert_eq!(ids(&body), ids(&want), "{uri}");
    }
    let (code, picked) = send(
        &full,
        "GET",
        "/ngsi-ld/v1/entities?type=Car&pick=colour",
        None,
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{picked}");
    assert_eq!(picked.as_array().map(Vec::len), Some(1), "{picked}");
    assert_eq!(picked[0]["colour"]["value"], "red", "{picked}");
}

/// 5.6.17 step 4, 5.6.18 step 4: the `type` selector is judged again on the document
/// the store is about to change, so a writer that retypes the Entity between
/// the handler's read and its write wins and the replace or merge lands
/// nowhere.
#[tokio::test(flavor = "multi_thread")]
async fn a_type_selector_is_rejudged_on_the_document_the_store_writes() {
    const DC: &str = "https://uri.etsi.org/ngsi-ld/default-context/";
    let id = "urn:ngsi-ld:Vehicle:retyped";
    let moved = json!({
        "id": id,
        "type": [format!("{DC}Truck")],
        format!("{DC}speed"): [{"type": "Property", "value": 1}],
        "createdAt": "2026-01-01T00:00:00.000Z",
        "modifiedAt": "2026-01-01T00:00:00.000Z",
    });
    for (method, body) in [("PUT", json!({"id": id, "type": "Vehicle"}))] {
        let inner = Arc::new(AnyStore::Mem(Default::default()));
        let full = AppState::with_store("me".into(), inner.clone(), "memory");
        let doc = json!({"id": id, "type": "Vehicle",
                         "speed": {"type": "Property", "value": 5}});
        let (code, resp) = send(&full, "POST", "/ngsi-ld/v1/entities", Some(doc)).await;
        assert_eq!(code, StatusCode::CREATED, "{resp}");
        let racing = AppState::with_drivers(
            "me".into(),
            Arc::new(Double::racing_write(inner.clone(), moved.clone())),
            inner.clone(),
            "memory",
        );
        let uri = format!("/ngsi-ld/v1/entities/{id}?type=Vehicle");
        let (code, resp) = send(&racing, method, &uri, Some(body)).await;
        assert_eq!(code, StatusCode::NOT_FOUND, "{method}: {resp}");
        let (_, after) = send(&full, "GET", &format!("/ngsi-ld/v1/entities/{id}"), None).await;
        assert_eq!(after["speed"]["value"], 1, "{method}: {after}");
        assert_eq!(after["type"], "Truck", "{method}: {after}");
    }
}

/// T-4683: a tenant past one scan page, on a driver that ignores the keyset
/// cursor, is walked by OFFSET from where the fold is, and the answer is the
/// full store's, every Entity counted once.
#[tokio::test(flavor = "multi_thread")]
async fn a_fold_over_several_pages_on_a_driver_without_keyset_counts_each_entity_once() {
    let inner = Arc::new(AnyStore::Mem(Default::default()));
    let full = AppState::with_store("me".into(), inner.clone(), "memory");
    for i in 0..620 {
        let car = json!({"id": format!("urn:ngsi-ld:Car:{i:04}"), "type": "Car",
                         "speed": {"type": "Property", "value": i}});
        let (code, body) = send(&full, "POST", "/ngsi-ld/v1/entities", Some(car)).await;
        assert_eq!(code, StatusCode::CREATED, "{body}");
    }
    let st = AppState::with_drivers(
        "me".into(),
        Arc::new(Double::lesser(inner.clone(), Lesser::NoKeyset)),
        inner.clone(),
        "memory",
    );
    for uri in [
        "/ngsi-ld/v1/types?details=true",
        "/ngsi-ld/v1/attributes?details=true",
    ] {
        let (code, want) = send(&full, "GET", uri, None).await;
        assert_eq!(code, StatusCode::OK, "{uri}: {want}");
        let (code, body) = send(&st, "GET", uri, None).await;
        assert_eq!(code, StatusCode::OK, "{uri}: {body}");
        assert_eq!(comparable(body), comparable(want), "{uri}");
    }
}

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
