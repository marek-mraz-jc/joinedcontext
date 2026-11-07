// SPDX-License-Identifier: EUPL-1.2
//! A late value is a backfill (T-4665, platform ADR-N-054 2.1): with
//! `ANTARES_HISTORY_BACKFILL=on`, a Core API write whose `observedAt` is
//! older than the instance the Entity holds goes to history only, and the
//! current state keeps the newer value. Its own process: the switch is read
//! once.

use antares_api::AppState;
use antares_sql::store::any::AnyStore;
use antares_sql::store::Store;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

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

fn values(doc: &Value, attr: &str) -> Vec<Value> {
    let rows = match doc.get(attr) {
        Some(Value::Array(a)) => a.clone(),
        Some(v) => vec![v.clone()],
        None => vec![],
    };
    rows.iter().map(|r| r["value"].clone()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_late_value_reaches_history_and_never_the_current_state() {
    // Set before the broker reads it, in a process of its own.
    std::env::set_var("ANTARES_HISTORY_BACKFILL", "on");
    let store = Arc::new(AnyStore::Mem(Store::default()));
    let mut st = AppState::with_drivers("me".into(), store.clone(), store, "memory");
    antares_api::wire(&mut st).await;
    let id = "urn:ngsi-ld:Station:late";
    let level = |v: f64, at: &str| json!({"type": "Property", "value": v, "observedAt": at});
    let (s, b) = req(
        &st,
        "POST",
        "/ngsi-ld/v1/entities",
        Some(json!({"id": id, "type": "Station", "level": level(12.0, "2026-10-18T10:00:00Z")})),
    )
    .await;
    assert_eq!(s, 201, "{b}");
    // Yesterday's reading arrives today, through each Core API path.
    for (method, path, at) in [
        (
            "PATCH",
            format!("/ngsi-ld/v1/entities/{id}/attrs"),
            "2026-10-17T10:00:00Z",
        ),
        (
            "POST",
            format!("/ngsi-ld/v1/entities/{id}/attrs"),
            "2026-10-17T11:00:00Z",
        ),
        (
            "PATCH",
            format!("/ngsi-ld/v1/entities/{id}"),
            "2026-10-17T12:00:00Z",
        ),
    ] {
        let (s, b) = req(&st, method, &path, Some(json!({"level": level(9.0, at)}))).await;
        assert_eq!(s, 204, "{method} {path}: {b}");
    }
    let (s, b) = req(
        &st,
        "POST",
        "/ngsi-ld/v1/entityOperations/upsert?options=update",
        Some(json!([{"id": id, "type": "Station", "level": level(9.5, "2026-10-17T13:00:00Z")}])),
    )
    .await;
    assert!(s.is_success(), "{s} {b}");

    let (_, now) = req(&st, "GET", &format!("/ngsi-ld/v1/entities/{id}"), None).await;
    assert_eq!(now["level"]["value"], 12.0, "{now}");
    let (_, history) = req(
        &st,
        "GET",
        &format!("/ngsi-ld/v1/temporal/entities/{id}"),
        None,
    )
    .await;
    let mut kept = values(&history, "level");
    kept.sort_by(|a, b| a.as_f64().partial_cmp(&b.as_f64()).expect("numbers"));
    assert_eq!(
        kept,
        [json!(9.0), json!(9.0), json!(9.0), json!(9.5), json!(12.0)],
        "{history}"
    );

    // A newer reading still moves the current state.
    let (s, _) = req(
        &st,
        "PATCH",
        &format!("/ngsi-ld/v1/entities/{id}/attrs"),
        Some(json!({"level": level(13.0, "2026-10-18T11:00:00Z")})),
    )
    .await;
    assert_eq!(s, 204);
    let (_, now) = req(&st, "GET", &format!("/ngsi-ld/v1/entities/{id}"), None).await;
    assert_eq!(now["level"]["value"], 13.0);
}
