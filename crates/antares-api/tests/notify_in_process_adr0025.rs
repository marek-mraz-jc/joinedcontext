// SPDX-License-Identifier: EUPL-1.2
//! ADR-0025 rule 7 at the API seam: a notification whose endpoint the
//! registered `ForwardTransport` takes is handed to it in process, on the
//! first attempt and on a dead-letter replay, with the sending Subscription's
//! Tenant; an endpoint it does not take goes out on the wire as before, and a
//! transport that overrides neither method takes nothing. The receiving
//! Tenant of a 5.8.1.4 copy is read from the broker's own inbound index.
//! (antares-icsr tests the real transport; this file covers the seam in
//! antares-api, whose coverage floor counts only its own binaries, T-4250.)

use antares_api::{AppState, BuiltForward, ForwardFuture, ForwardTransport, NotifyFuture};
use antares_model::TenantId;
use antares_store::Kind;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;

/// No socket listens here: a delivery that reached the wire would fail.
const TAKEN: &str = "http://127.0.0.1:9/in-process/notify";

/// Takes the notifications sent to [`TAKEN`] and records them; refuses them
/// while `refuse` is set.
#[derive(Default)]
struct Inbox {
    seen: Mutex<Vec<(String, String, Value)>>,
    refuse: AtomicBool,
}

impl ForwardTransport for Inbox {
    fn forward<'a>(&'a self, _req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        None
    }

    fn notifies(&self, uri: &str) -> bool {
        uri == TAKEN
    }

    fn notify<'a>(
        &'a self,
        tenant: &'a TenantId,
        uri: &'a str,
        notification: &'a Value,
    ) -> NotifyFuture<'a> {
        let refused = self.refuse.load(Ordering::SeqCst);
        if !refused {
            self.seen.lock().expect("lock").push((
                tenant.as_str().to_owned(),
                uri.to_owned(),
                notification.clone(),
            ));
        }
        Box::pin(std::future::ready(if refused {
            Err("no export grant".to_owned())
        } else {
            Ok(())
        }))
    }
}

/// A transport that overrides neither `notifies` nor `notify`.
struct ForwardsOnly;

impl ForwardTransport for ForwardsOnly {
    fn forward<'a>(&'a self, _req: BuiltForward<'a>) -> Option<ForwardFuture<'a>> {
        None
    }
}

async fn state(transport: Arc<dyn ForwardTransport>) -> AppState {
    antares_jsonld::allow_private_egress(true);
    let mut st = AppState::new("in-process".into());
    antares_api::wire(&mut st).await;
    st.with_forward_transport(transport).expect("one transport")
}

async fn call(st: &AppState, method: &str, path: &str, body: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .header("NGSILD-Tenant", "acme")
        .body(Body::from(body.to_owned()))
        .expect("req");
    let resp = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("resp");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn subscribe_and_create(st: &AppState, uri: &str, kind: &str) {
    let sub = json!({"type": "Subscription", "entities": [{"type": kind}],
                     "notification": {"endpoint": {"uri": uri}}});
    let (s, b) = call(st, "POST", "/ngsi-ld/v1/subscriptions", &sub.to_string()).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let entity = json!({"id": format!("urn:ngsi-ld:{kind}:1"), "type": kind,
                        "a": {"type": "Property", "value": 1}});
    let (s, b) = call(st, "POST", "/ngsi-ld/v1/entities", &entity.to_string()).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
}

fn letter(uri: &str) -> Value {
    json!({
        "id": "urn:ngsi-ld:DeadLetter:1", "type": "DeadLetter", "subscriptionId": "urn:s:1",
        "uri": uri, "binding": "http", "timeoutMs": 2000, "attempts": 2,
        "firstError": "no export grant", "lastError": "no export grant",
        "firstAt": "2026-01-01T00:00:00Z", "lastAt": "2026-01-01T00:00:01Z",
        "headers": [["Content-Type", "application/json"], ["NGSILD-Tenant", "acme"]],
        "payload": {"id": "urn:ngsi-ld:Notification:1", "type": "Notification",
                    "subscriptionId": "urn:s:1", "data": [{"id": "urn:ngsi-ld:Room:1", "type": "Room"}]},
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_notification_the_transport_takes_reaches_it_with_the_tenant_and_no_socket() {
    let inbox = Arc::new(Inbox::default());
    let st = state(inbox.clone()).await;
    subscribe_and_create(&st, TAKEN, "Taken").await;
    let wait = Duration::from_secs(5 * antares_api::state::slow_factor());
    let deadline = tokio::time::Instant::now() + wait;
    while inbox.seen.lock().expect("lock").is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no in-process delivery within {wait:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (tenant, uri, body) = inbox.seen.lock().expect("lock")[0].clone();
    assert_eq!((tenant.as_str(), uri.as_str()), ("acme", TAKEN));
    assert_eq!(body["type"], "Notification");
    assert_eq!(body["data"][0]["id"], "urn:ngsi-ld:Taken:1");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_letter_goes_in_process_and_a_refusal_keeps_it() {
    let inbox = Arc::new(Inbox::default());
    let st = state(inbox.clone()).await;
    let acme = TenantId::new("acme").expect("tenant");
    let id = "urn:ngsi-ld:DeadLetter:1";
    assert!(st
        .store
        .create(&acme, Kind::DeadLetter, id, letter(TAKEN))
        .await
        .expect("seed"));
    let replay = format!("/q/dead-letters/{id}/replay?tenant=acme");

    inbox.refuse.store(true, Ordering::SeqCst);
    let (s, b) = call(&st, "POST", &replay, "").await;
    assert_eq!(s, StatusCode::BAD_GATEWAY, "{b}");
    assert!(
        st.store
            .get(&acme, Kind::DeadLetter, id)
            .await
            .expect("get")
            .is_some(),
        "a refused replay keeps the letter"
    );

    inbox.refuse.store(false, Ordering::SeqCst);
    let (s, b) = call(&st, "POST", &replay, "").await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    let seen = inbox.seen.lock().expect("lock").clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!((seen[0].0.as_str(), seen[0].1.as_str()), ("acme", TAKEN));
    assert_eq!(seen[0].2["data"][0]["id"], "urn:ngsi-ld:Room:1");
    assert!(
        st.store
            .get(&acme, Kind::DeadLetter, id)
            .await
            .expect("get")
            .is_none(),
        "a delivered letter is gone"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_transport_that_takes_nothing_leaves_the_notification_on_the_wire() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Value>(4);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let uri = format!("http://{}/notify", listener.local_addr().expect("addr"));
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
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });

    let st = state(Arc::new(ForwardsOnly)).await;
    subscribe_and_create(&st, &uri, "Wired").await;
    let wait = Duration::from_secs(5 * antares_api::state::slow_factor());
    let got = tokio::time::timeout(wait, rx.recv())
        .await
        .expect("on the wire")
        .expect("one");
    assert_eq!(got["data"][0]["id"], "urn:ngsi-ld:Wired:1");

    // and asked directly, the default refuses rather than drops
    let acme = TenantId::new("acme").expect("tenant");
    assert!(!ForwardsOnly.notifies(&uri));
    assert!(ForwardsOnly.notify(&acme, &uri, &got).await.is_err());
}

#[tokio::test]
async fn the_receiver_of_a_copy_is_read_from_the_inbound_index_only() {
    let st = state(Arc::new(ForwardsOnly)).await;
    assert_eq!(
        st.notification_receiver("urn:ngsi-ld:Subscription:unknown")
            .await,
        None
    );
    let index = TenantId::new_internal("distsub-index").expect("index tenant");
    let doc = json!({"tenant": "acme", "own": "urn:ngsi-ld:Subscription:own"});
    assert!(st
        .store
        .create(&index, Kind::DistSub, "urn:remote:copy", doc)
        .await
        .expect("seed"));
    assert_eq!(
        st.notification_receiver("urn:remote:copy").await.as_deref(),
        Some("acme")
    );
}
