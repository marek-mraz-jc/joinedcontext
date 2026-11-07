// SPDX-License-Identifier: EUPL-1.2
//! T-4773 moved `notify.rs` into `notify/{matching,payload,throttle,interval,
//! delivery,csource,retry}.rs` and the router assembly out of `lib.rs` into
//! `router.rs` and `ops.rs`. These tests call the moved public functions by
//! the paths the broker binary and the surface crates use, so a move that
//! drops a re-export or cuts the pipeline between two of the new modules
//! fails here.

use antares_api::AppState;
use antares_store::Kind;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const TENANT: &str = "acme";

async fn post(st: &AppState, path: &str, body: &Value) -> StatusCode {
    let body = body.to_string();
    let req = Request::builder()
        .method("POST")
        .uri(format!("/ngsi-ld/v1/{path}"))
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .header("NGSILD-Tenant", TENANT)
        .body(Body::from(body))
        .expect("req");
    antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("resp")
        .status()
}

async fn sink() -> (String, tokio::sync::mpsc::Receiver<Value>) {
    let (tx, rx) = tokio::sync::mpsc::channel::<Value>(8);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = axum::Router::new().route(
        "/notify",
        axum::routing::post(move |body: axum::body::Bytes| {
            let tx = tx.clone();
            async move {
                let _ = tx
                    .send(serde_json::from_slice(&body).unwrap_or(Value::Null))
                    .await;
                StatusCode::OK
            }
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    (format!("http://{addr}/notify"), rx)
}

async fn next(rx: &mut tokio::sync::mpsc::Receiver<Value>) -> Value {
    tokio::time::timeout(
        std::time::Duration::from_secs(5 * antares_api::state::slow_factor()),
        rx.recv(),
    )
    .await
    .expect("notification within the window")
    .expect("one notification")
}

/// `notify::process_change` is the entry the broker's bus consumer calls
/// (antares-broker wiring.rs). One call crosses the drain (throttle), the
/// candidate evaluation (matching), the body (payload) and the send
/// (delivery): the subscriber receives the entity it watches.
#[tokio::test(flavor = "multi_thread")]
async fn process_change_from_outside_reaches_the_subscriber() {
    antares_jsonld::allow_private_egress(true);
    let mut st = AppState::new("module-boundaries".into());
    antares_api::wire(&mut st).await;
    let (uri, mut rx) = sink().await;
    let sub = json!({
        "id": "urn:ngsi-ld:Subscription:boundary",
        "type": "Subscription",
        "entities": [{"type": "Vehicle"}],
        "notification": {"endpoint": {"uri": uri, "accept": "application/json"}}
    });
    assert_eq!(post(&st, "subscriptions", &sub).await, StatusCode::CREATED);
    let id = "urn:ngsi-ld:Vehicle:boundary1";
    let entity = json!({"id": id, "type": "Vehicle",
                        "speed": {"type": "Property", "value": 7}});
    assert_eq!(post(&st, "entities", &entity).await, StatusCode::CREATED);
    // the write's own hook notifies once
    assert_eq!(next(&mut rx).await["data"][0]["id"], id);

    let tenant = antares_model::TenantId::new(TENANT).expect("tenant");
    let stored = st
        .store
        .get(&tenant, Kind::Entity, id)
        .await
        .expect("get")
        .expect("the entity is stored");
    antares_api::notify::process_change(&st, TENANT, None, Some(stored)).await;
    let n = next(&mut rx).await;
    assert_eq!(
        n["subscriptionId"], "urn:ngsi-ld:Subscription:boundary",
        "{n}"
    );
    assert_eq!(n["data"][0]["id"], id, "{n}");
}

/// `trim_trailing_slash` is applied by antares-broker in front of the router
/// (it must run before routing). From outside the crate it still cuts the
/// slash and keeps the query.
#[test]
fn trim_trailing_slash_from_outside_keeps_the_query() {
    let req = Request::builder()
        .uri("/ngsi-ld/v1/entities/?type=Vehicle")
        .body(())
        .expect("req");
    let out = antares_api::trim_trailing_slash(req);
    assert_eq!(out.uri().path(), "/ngsi-ld/v1/entities");
    assert_eq!(out.uri().query(), Some("type=Vehicle"));
}
