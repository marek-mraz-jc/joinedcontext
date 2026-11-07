// SPDX-License-Identifier: EUPL-1.2
//! The 5.5.10 Tenant wall remembers the Tenants the store said exist
//! (T-5500): a request naming one it holds goes to the operation without a
//! store round trip, a purge forgets the name at once, and a "does not
//! exist" is never remembered, since the next write may create the Tenant.

use crate::common;
use antares_api::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

async fn state() -> (AppState, Arc<common::Double>) {
    let mut st = AppState::new("antares-test".into());
    antares_api::wire(&mut st).await;
    let double = Arc::new(common::Double::flaky_list(st.store.clone(), 0));
    st.store = double.clone();
    (st, double)
}

async fn send(
    st: &AppState,
    method: &str,
    path: &str,
    tenant: Option<&str>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(path);
    if let Some(t) = tenant {
        b = b.header("NGSILD-Tenant", t);
    }
    let req = if method == "POST" {
        let body = json!({"id": "urn:ngsi-ld:Room:1", "type": "Room",
            "temperature": {"type": "Property", "value": 21}})
        .to_string();
        b.header("Content-Type", "application/json")
            .header("Content-Length", body.len())
            .body(Body::from(body))
    } else {
        b.body(Body::empty())
    }
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

const READ: &str = "/ngsi-ld/v1/entities?type=Room";

#[tokio::test]
async fn a_known_tenant_is_probed_once() {
    let (st, double) = state().await;
    let (s, _) = send(&st, "POST", "/ngsi-ld/v1/entities", Some("known")).await;
    assert_eq!(s, StatusCode::CREATED);
    for _ in 0..10 {
        let (s, body) = send(&st, "GET", READ, Some("known")).await;
        assert_eq!(s, StatusCode::OK, "{body}");
    }
    assert_eq!(double.tenant_probes(), 1);
}

#[tokio::test]
async fn a_purged_tenant_is_nonexistent_at_once() {
    let (st, _) = state().await;
    let (s, _) = send(&st, "POST", "/ngsi-ld/v1/entities", Some("purged")).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = send(&st, "GET", READ, Some("purged")).await;
    assert_eq!(s, StatusCode::OK);
    let (s, body) = send(&st, "DELETE", "/q/tenants/purged", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{body}");
    let (s, body) = send(&st, "GET", READ, Some("purged")).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(
        body["type"],
        "https://uri.etsi.org/ngsi-ld/errors/NonexistentTenant"
    );
}

#[tokio::test]
async fn an_unknown_tenant_is_never_cached() {
    let (st, double) = state().await;
    let (s, body) = send(&st, "GET", READ, Some("later")).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
    let (s, _) = send(&st, "POST", "/ngsi-ld/v1/entities", Some("later")).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, body) = send(&st, "GET", READ, Some("later")).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(double.tenant_probes(), 2);
}

#[tokio::test]
async fn a_reserved_tenant_is_refused_before_the_memory() {
    let (st, double) = state().await;
    let (s, _) = send(&st, "GET", READ, Some("snap-index")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(double.tenant_probes(), 0);
}
