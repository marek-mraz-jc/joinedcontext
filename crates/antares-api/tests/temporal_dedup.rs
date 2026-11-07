// SPDX-License-Identifier: EUPL-1.2
//! 4.5.7: a temporal Property instance is the Property "at a particular point
//! in time … recorded as a Temporal Property of the instance (typically
//! observedAt)". A Core-API re-send carrying the same observedAt for the same
//! attribute and datasetId therefore corrects THAT instance instead of
//! appending a second one; an instance without observedAt has no such point
//! in time and stays append-only.

use antares_api::AppState;
use antares_sql::store::any::AnyStore;
use antares_sql::store::Store;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

async fn state() -> AppState {
    let store = Arc::new(AnyStore::Mem(Store::default()));
    let mut st = AppState::with_drivers("me".into(), store.clone(), store, "memory");
    antares_api::wire(&mut st).await;
    st
}

async fn req(st: &AppState, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(path);
    let body = match body {
        Some(v) => {
            let s = v.to_string();
            b = b
                .header("Content-Type", "application/json")
                .header("Content-Length", s.len());
            Body::from(s)
        }
        None => Body::empty(),
    };
    let resp = antares_api::router(st.clone())
        .oneshot(b.body(body).expect("req"))
        .await
        .expect("resp");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn instances(doc: &Value, attr: &str) -> Vec<Value> {
    match doc.get(attr) {
        Some(Value::Array(a)) => a.clone(),
        Some(v) => vec![v.clone()],
        None => vec![],
    }
}

const ID: &str = "urn:ngsi-ld:D:1";
const T1: &str = "2026-01-01T00:00:00Z";
const T2: &str = "2026-01-01T00:01:00Z";

#[tokio::test(flavor = "multi_thread")]
async fn a_resend_with_the_same_observed_at_corrects_the_instance() {
    let st = state().await;
    let (s, b) = req(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(json!({"id": ID, "type": "D",
            "speed": {"type": "Property", "value": 1, "observedAt": T1},
            "heading": {"type": "Property", "value": 10}})),
    )
    .await;
    assert_eq!(s, 201, "{b}");
    let patch = |v: Value| json!({"speed": {"type": "Property", "value": v, "observedAt": T1}});
    let (s, b) = req(
        &st,
        "PATCH",
        &format!("/ngsi-ld/v1/entities/{ID}/attrs"),
        Some(patch(json!(2))),
    )
    .await;
    assert_eq!(s, 204, "{b}");
    let (s, doc) = req(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities/{ID}"),
        None,
    )
    .await;
    assert_eq!(s, 200, "{doc}");
    let speed = instances(&doc, "speed");
    assert_eq!(
        speed.len(),
        1,
        "same observedAt = same instance, not two: {doc}"
    );
    assert_eq!(
        speed[0]["value"], 2,
        "the corrected value replaced the first: {doc}"
    );

    // a new point in time is a new instance
    let (s, _) = req(
        &st,
        "PATCH",
        &format!("/ngsi-ld/v1/entities/{ID}/attrs"),
        Some(json!({"speed": {"type": "Property", "value": 3, "observedAt": T2}})),
    )
    .await;
    assert_eq!(s, 204);
    let (_, doc) = req(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities/{ID}"),
        None,
    )
    .await;
    let speed = instances(&doc, "speed");
    assert_eq!(speed.len(), 2, "{doc}");
    let ids: std::collections::BTreeSet<_> = speed
        .iter()
        .map(|i| i["instanceId"].as_str().unwrap_or("").to_owned())
        .collect();
    assert_eq!(
        ids.len(),
        2,
        "distinct instances carry distinct instanceIds: {doc}"
    );

    // no observedAt: no point in time to key on, every change appends
    for v in [11, 12] {
        let (s, _) = req(
            &st,
            "PATCH",
            &format!("/ngsi-ld/v1/entities/{ID}/attrs"),
            Some(json!({"heading": {"type": "Property", "value": v}})),
        )
        .await;
        assert_eq!(s, 204);
    }
    let (_, doc) = req(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities/{ID}"),
        None,
    )
    .await;
    assert_eq!(instances(&doc, "heading").len(), 3, "{doc}");
}

#[tokio::test(flavor = "multi_thread")]
async fn dataset_ids_keep_their_own_instances_at_one_observed_at() {
    let st = state().await;
    let (s, b) = req(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(json!({"id": ID, "type": "D",
            "speed": [
                {"type": "Property", "value": 1, "observedAt": T1},
                {"type": "Property", "value": 5, "observedAt": T1, "datasetId": "urn:ngsi-ld:ds:gps"}]})),
    )
    .await;
    assert_eq!(s, 201, "{b}");
    let (_, doc) = req(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities/{ID}"),
        None,
    )
    .await;
    let speed = instances(&doc, "speed");
    assert_eq!(
        speed.len(),
        2,
        "two datasets, two instances at the same instant: {doc}"
    );
    assert_ne!(speed[0]["instanceId"], speed[1]["instanceId"], "{doc}");
}

const UP: &str = "urn:ngsi-ld:D:up";

/// 5.6.11.4 sends a known Temporal Evolution through 5.6.12: its instances
/// "shall be added to the existing" one. Instance identity is the
/// system-generated instanceId, never observedAt, so a second upsert at the
/// same instant appends (forge 167fa185, 008_01). Each instanceId the client
/// was handed still addresses its own instance.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_temporal_upsert_at_the_same_instant_appends() {
    let st = state().await;
    let post = |speed: Value| {
        json!({"id": UP, "type": "D", "speed": [{"type": "Property", "value": speed,
               "observedAt": T1}]})
    };
    for v in [120, 121] {
        let (s, b) = req(
            &st,
            "POST",
            "/ngsi-ld/v1/temporal/entities",
            Some(post(json!(v))),
        )
        .await;
        assert!(s.is_success(), "{s} {b}");
    }
    let (s, doc) = req(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities/{UP}"),
        None,
    )
    .await;
    assert_eq!(s, 200, "{doc}");
    let speed = instances(&doc, "speed");
    let mut values: Vec<i64> = speed.iter().filter_map(|i| i["value"].as_i64()).collect();
    values.sort_unstable();
    assert_eq!(values, [120, 121], "both instances are kept: {doc}");
    assert_ne!(speed[0]["instanceId"], speed[1]["instanceId"], "{doc}");

    // the first instanceId still names its own instance (5.6.15)
    let first = speed
        .iter()
        .find(|i| i["value"] == 120)
        .and_then(|i| i["instanceId"].as_str())
        .expect("instanceId of 120")
        .to_owned();
    let (s, b) = req(
        &st,
        "DELETE",
        &format!("/ngsi-ld/v1/temporal/entities/{UP}/attrs/speed/{first}"),
        None,
    )
    .await;
    assert_eq!(s, 204, "{b}");
    let (_, doc) = req(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities/{UP}"),
        None,
    )
    .await;
    let left: Vec<Value> = instances(&doc, "speed")
        .iter()
        .map(|i| i["value"].clone())
        .collect();
    assert_eq!(left, [json!(121)], "only the 120 instance went: {doc}");
}

const AR: &str = "urn:ngsi-ld:D:ar";

/// The same rule on the Core-API mirror: a re-send with the same observedAt
/// corrects the recorded instance (4.5.7), so that instance's createdAt is
/// still the moment it was first recorded.
#[tokio::test(flavor = "multi_thread")]
async fn an_auto_recorded_correction_keeps_the_instance_it_corrects() {
    let st = state().await;
    let (s, b) = req(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(json!({"id": AR, "type": "D",
            "speed": {"type": "Property", "value": 1, "observedAt": T1}})),
    )
    .await;
    assert_eq!(s, 201, "{b}");
    let read = || async {
        let (s, doc) = req(
            &st,
            "GET",
            &format!("/ngsi-ld/v1/temporal/entities/{AR}?options=sysAttrs"),
            None,
        )
        .await;
        assert_eq!(s, 200, "{doc}");
        let mut all = instances(&doc, "speed");
        assert_eq!(all.len(), 1, "one instant, one instance: {doc}");
        all.remove(0)
    };
    let before = read().await;

    let (s, b) = req(
        &st,
        "PATCH",
        &format!("/ngsi-ld/v1/entities/{AR}/attrs"),
        Some(json!({"speed": {"type": "Property", "value": 2, "observedAt": T1}})),
    )
    .await;
    assert_eq!(s, 204, "{b}");
    let after = read().await;
    assert_eq!(after["value"], 2, "{after}");
    assert_eq!(
        after["instanceId"], before["instanceId"],
        "the corrected instance changed identity: {after}"
    );
    assert_eq!(
        after["createdAt"], before["createdAt"],
        "createdAt shall remain unchanged: {after}"
    );
}
