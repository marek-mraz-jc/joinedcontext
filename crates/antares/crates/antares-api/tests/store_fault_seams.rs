// SPDX-License-Identifier: EUPL-1.2
//! The store and history calls an operation makes after its first one, each
//! failed on its own through [`crate::common::faults`]: the sweep of expired
//! documents goes on past a tenant whose confirmations cannot be swept, and
//! `GET /q/tenants/{tenant}` answers its counts when the history or the size
//! cannot be measured or estimated (T-5304, T-6085, T-6159).

use crate::common::faults::{store_down, Faults, StoreFaults, TemporalFaults};
use antares_api::AppState;
use antares_model::{NgsiError, TenantId};
use antares_store::Kind;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

async fn send(st: &AppState, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let text = body.map(|b| b.to_string()).unwrap_or_default();
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json")
        .header("Content-Length", text.len())
        .header("NGSILD-Tenant", "seams")
        .body(Body::from(text))
        .expect("request");
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

/// A broker with one Room in tenant `seams`, then `faults` on its store or
/// history.
async fn seeded() -> AppState {
    let mut st = AppState::new("store-fault-seams".into());
    antares_api::wire(&mut st).await;
    let room = json!({"id": "urn:ngsi-ld:Room:1", "type": "Room",
        "temperature": {"type": "Property", "value": 21.5,
                        "observedAt": "2026-01-01T00:00:00Z"}});
    let (s, b) = send(&st, "POST", "/ngsi-ld/v1/entities", Some(room)).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    st
}

fn failing_store(
    st: &mut AppState,
    rule: impl Fn(&str, usize) -> Option<NgsiError> + Send + Sync + 'static,
) {
    st.store = Arc::new(StoreFaults {
        inner: st.store.clone(),
        faults: Faults::new(rule),
    });
}

fn failing_history(
    st: &mut AppState,
    rule: impl Fn(&str, usize) -> Option<NgsiError> + Send + Sync + 'static,
) {
    st.temporal = Arc::new(TemporalFaults {
        inner: st.temporal.clone(),
        faults: Faults::new(rule),
    });
}

/// 4.22: a tenant whose expired write confirmations cannot be swept is
/// logged and passed; the same tick still reaps its expired EntityMap.
#[tokio::test(flavor = "multi_thread")]
async fn the_sweep_goes_on_past_a_confirmation_sweep_that_fails() {
    let st = AppState::new("store-fault-sweep".into());
    let id = "urn:ngsi-ld:EntityMap:gone";
    let map = json!({"id": id, "expiresAt": "2000-01-01T00:00:00Z"});
    let tenant = TenantId::default();
    assert!(st
        .store
        .create(&tenant, Kind::EntityMap, id, map)
        .await
        .expect("create"));
    let faults = Arc::new(StoreFaults {
        inner: st.store.clone(),
        faults: Faults::new(|m, _| (m == "confirmation_sweep").then(store_down)),
    });
    let mut st = st;
    st.store = faults.clone();
    assert_eq!(antares_api::sweep_expired_docs(&st).await, 1);
    assert!(faults.faults.calls("confirmation_sweep") >= 1);
    assert!(st
        .store
        .get(&tenant, Kind::EntityMap, id)
        .await
        .expect("read")
        .is_none());
}

/// ADR-0026 rule 7a, 4.22: a lapsed socket Subscription or EntityMap the
/// store cannot delete is not counted as swept, and it stays for the next
/// tick.
#[tokio::test(flavor = "multi_thread")]
async fn a_socket_subscription_the_store_cannot_delete_is_not_counted() {
    let mut st = AppState::new("store-fault-socket".into());
    let id = "urn:ngsi-ld:Subscription:ws-stuck";
    let sub = json!({"id": id, "type": "Subscription", "entities": [{"type": "Car"}],
        "expiresAt": "2000-01-01T00:00:00Z",
        "notification": {"endpoint": {"uri": "urn:antares:ws:gone"}}});
    let tenant = TenantId::default();
    st.store
        .create(&tenant, Kind::Subscription, id, sub)
        .await
        .expect("seed");
    let map = "urn:ngsi-ld:EntityMap:stuck";
    st.store
        .create(
            &tenant,
            Kind::EntityMap,
            map,
            json!({"id": map, "expiresAt": "2000-01-01T00:00:00Z"}),
        )
        .await
        .expect("seed");
    failing_store(&mut st, |m, _| (m == "delete").then(store_down));
    assert_eq!(antares_api::sweep_expired_docs(&st).await, 0);
    assert!(st
        .store
        .get(&tenant, Kind::EntityMap, map)
        .await
        .expect("read")
        .is_some());
    assert!(st
        .store
        .get(&tenant, Kind::Subscription, id)
        .await
        .expect("read")
        .is_some());
}

/// A history driver that cannot count leaves the history out: no
/// `attrInstances`, no `estimated`, the counts still 200.
#[tokio::test(flavor = "multi_thread")]
async fn a_history_that_cannot_be_counted_is_left_out() {
    let mut st = seeded().await;
    failing_history(&mut st, |m, _| {
        (m == "attr_instance_usage").then(|| NgsiError::OperationNotSupported("count".into()))
    });
    let (s, got) = send(&st, "GET", "/q/tenants/seams", None).await;
    assert_eq!(s, StatusCode::OK, "{got}");
    assert_eq!(got["counts"]["entities"], 1, "{got}");
    assert_eq!(got["counts"].get("attrInstances"), None, "{got}");
    assert_eq!(got.get("estimated"), None, "{got}");
    assert_eq!(got.get("bytes"), None, "{got}");
}

/// A store that cannot size a tenant answers the history exactly and no
/// `bytes`; one whose exact scan and estimate both fail does the same, and
/// neither says it estimated.
#[tokio::test(flavor = "multi_thread")]
async fn a_size_that_cannot_be_measured_or_estimated_is_left_out() {
    for unsized_store in [true, false] {
        let mut st = seeded().await;
        failing_store(&mut st, move |m, _| match m {
            "tenant_bytes" if unsized_store => {
                Some(NgsiError::OperationNotSupported("tenant size".into()))
            }
            "tenant_bytes" | "tenant_bytes_estimate" => Some(store_down()),
            _ => None,
        });
        let (s, got) = send(&st, "GET", "/q/tenants/seams", None).await;
        assert_eq!(s, StatusCode::OK, "{got}");
        assert_eq!(got["estimated"], false, "{unsized_store}: {got}");
        assert!(got["counts"]["attrInstances"].is_u64(), "{got}");
        assert_eq!(got.get("bytes"), None, "{unsized_store}: {got}");
    }
}

/// Postgres keeps when a Tenant was first written, and the tenant's row
/// carries it. Skips without ANTARES_TEST_DATABASE_URL.
#[cfg(feature = "postgres")]
#[tokio::test(flavor = "multi_thread")]
async fn a_postgres_tenant_says_when_it_was_created() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = antares_sql::store::pg::connect(&url, 5)
        .await
        .expect("connect");
    let tenant = TenantId::new("seams_created").expect("tenant");
    antares_sql::store::pg::ensure_tenant(&pool, &tenant)
        .await
        .expect("tenant row");
    let st = AppState::with_store(
        "store-fault-pg".into(),
        Arc::new(antares_sql::store::any::AnyStore::Pg(
            antares_sql::store::any::PgBackend::new(pool),
        )),
        "postgres",
    );
    let (s, got) = send(&st, "GET", "/q/tenants/seams_created", None).await;
    assert_eq!(s, StatusCode::OK, "{got}");
    assert!(
        got["createdAt"]
            .as_str()
            .is_some_and(|c| c.starts_with("20")),
        "{got}"
    );
}
