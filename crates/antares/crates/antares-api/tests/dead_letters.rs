// SPDX-License-Identifier: EUPL-1.2
//! Dead letters on the admin surface: notifications the delivery policy
//! gave up on are listed, replayed once through the same binding, or
//! deleted — always for ONE tenant named by `?tenant=`, never reachable
//! under the NGSI-LD API root.

use antares_api::AppState;
use antares_model::TenantId;
use antares_store::{CurrentStateDriverExt as _, Kind};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

async fn send(st: &AppState, method: &str, path: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        // what every HTTP/1.1 client sends for a bodyless request: the
        // bounds wall in front of this surface answers a POST without a
        // length with 6.3.4's bare 411.
        .header(axum::http::header::CONTENT_LENGTH, "0")
        .body(Body::empty())
        .expect("request");
    let res = antares_api::router(st.clone())
        .oneshot(req)
        .await
        .expect("response");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

fn state() -> AppState {
    antares_jsonld::allow_private_egress(true);
    AppState::new("antares-dead-letters".into())
}

type Seen = Arc<Mutex<Vec<(Vec<(String, String)>, Value)>>>;

/// A receiver answering `status`, recording headers and body.
async fn receiver(status: StatusCode) -> (String, Arc<AtomicUsize>, Seen) {
    let hits: Arc<AtomicUsize> = Arc::default();
    let seen: Seen = Arc::default();
    let (h, s) = (hits.clone(), seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = axum::Router::new().route(
        "/notify",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let (h, s) = (h.clone(), s.clone());
                async move {
                    h.fetch_add(1, Ordering::SeqCst);
                    let hs = headers
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                        .collect();
                    s.lock()
                        .expect("lock")
                        .push((hs, serde_json::from_slice(&body).unwrap_or(Value::Null)));
                    status
                }
            },
        ),
    );
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    (format!("http://{addr}/notify"), hits, seen)
}

fn letter(id: &str, sub: &str, uri: &str, last_at: &str) -> Value {
    json!({
        "id": id, "type": "DeadLetter", "subscriptionId": sub, "uri": uri,
        "binding": "http", "timeoutMs": 2000, "attempts": 2,
        "firstError": "HTTP 503", "lastError": "HTTP 503",
        "firstAt": "2026-01-01T00:00:00Z", "lastAt": last_at,
        "headers": [["Content-Type", "application/json"], ["NGSILD-Tenant", "acme"]],
        "payload": {"id": "urn:ngsi-ld:Notification:1", "type": "Notification",
                    "subscriptionId": sub, "data": [{"id": "urn:ngsi-ld:Room:1", "type": "Room"}]},
    })
}

/// The letter and the subscription it was written for, active and naming `uri`.
async fn seed(st: &AppState, tenant: &str, id: &str, sub: &str, uri: &str, last_at: &str) {
    let t = TenantId::new(tenant).expect("tenant");
    subscribe(st, &t, sub, uri).await;
    assert!(st
        .store
        .create(&t, Kind::DeadLetter, id, letter(id, sub, uri, last_at))
        .await
        .expect("seed"));
}

async fn subscribe(st: &AppState, t: &TenantId, sub: &str, uri: &str) {
    let doc = json!({
        "id": sub, "type": "Subscription", "entities": [{"type": "Room"}],
        "notification": {"endpoint": {"uri": uri}},
    });
    if st
        .store
        .get(t, Kind::Subscription, sub)
        .await
        .expect("get")
        .is_none()
    {
        st.store
            .create(t, Kind::Subscription, sub, doc)
            .await
            .expect("subscription");
    }
}

async fn stored(st: &AppState, tenant: &str, id: &str) -> Option<Value> {
    let t = TenantId::new(tenant).expect("tenant");
    st.store.get(&t, Kind::DeadLetter, id).await.expect("get")
}

#[tokio::test(flavor = "multi_thread")]
async fn list_is_per_tenant_newest_first_filtered_and_bounded() {
    let st = state();
    seed(
        &st,
        "acme",
        "urn:ngsi-ld:DeadLetter:1",
        "urn:s:1",
        "http://127.0.0.1:9/n",
        "2026-01-01T00:00:01Z",
    )
    .await;
    seed(
        &st,
        "acme",
        "urn:ngsi-ld:DeadLetter:3",
        "urn:s:2",
        "http://127.0.0.1:9/n",
        "2026-01-01T00:00:03Z",
    )
    .await;
    seed(
        &st,
        "acme",
        "urn:ngsi-ld:DeadLetter:2",
        "urn:s:1",
        "http://127.0.0.1:9/n",
        "2026-01-01T00:00:02Z",
    )
    .await;
    seed(
        &st,
        "other",
        "urn:ngsi-ld:DeadLetter:9",
        "urn:s:1",
        "http://127.0.0.1:9/n",
        "2026-01-01T00:00:09Z",
    )
    .await;

    let (s, b) = send(&st, "GET", "/q/dead-letters?tenant=acme").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let ids: Vec<&str> = b
        .as_array()
        .expect("array")
        .iter()
        .map(|l| l["id"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        ids,
        [
            "urn:ngsi-ld:DeadLetter:3",
            "urn:ngsi-ld:DeadLetter:2",
            "urn:ngsi-ld:DeadLetter:1"
        ]
    );
    // S4-31: the listing pages and carries no payload; the letter read
    // alone carries it
    assert!(b[0].get("payload").is_none(), "{b}");
    assert!(b[0]["payloadBytes"].as_u64().is_some_and(|n| n > 0), "{b}");
    let (_, b) = send(&st, "GET", "/q/dead-letters?tenant=acme&limit=1&offset=1").await;
    assert_eq!(b.as_array().expect("array").len(), 1);
    assert_eq!(b[0]["id"], "urn:ngsi-ld:DeadLetter:2");
    let (_, b) = send(&st, "GET", "/q/dead-letters?tenant=acme&offset=3").await;
    assert_eq!(b, json!([]), "past the end");
    let (_, b) = send(
        &st,
        "GET",
        "/q/dead-letters?tenant=acme&subscription=urn:s:1&limit=1&offset=1",
    )
    .await;
    assert_eq!(b[0]["id"], "urn:ngsi-ld:DeadLetter:1", "{b}");
    let (s, b) = send(
        &st,
        "GET",
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:2?tenant=acme",
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["payload"]["type"], "Notification", "{b}");
    let (s, _) = send(
        &st,
        "GET",
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:9?tenant=acme",
    )
    .await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "another tenant's letter is not found"
    );
    assert!(
        !b.to_string().contains("DeadLetter:9"),
        "another tenant's letter leaked: {b}"
    );

    let (_, b) = send(
        &st,
        "GET",
        "/q/dead-letters?tenant=acme&subscription=urn:s:1",
    )
    .await;
    assert_eq!(b.as_array().expect("array").len(), 2);
    let (_, b) = send(&st, "GET", "/q/dead-letters?tenant=acme&limit=1").await;
    assert_eq!(b.as_array().expect("array").len(), 1);
    assert_eq!(b[0]["id"], "urn:ngsi-ld:DeadLetter:3");
    let (_, b) = send(&st, "GET", "/q/dead-letters").await;
    assert_eq!(b, json!([]), "the default tenant holds none");
    let (_, b) = send(&st, "GET", "/q/dead-letters?tenant=never-seen").await;
    assert_eq!(b, json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_tenant_and_bad_limit_are_400() {
    let st = state();
    for path in [
        "/q/dead-letters?tenant=snap-abc",
        "/q/dead-letters?tenant=distsub-index",
        "/q/dead-letters?tenant=bad%20name",
        "/q/dead-letters?tenant=..%2F..",
        "/q/dead-letters?limit=0",
        "/q/dead-letters?limit=-1",
        "/q/dead-letters?limit=ten",
        "/q/dead-letters?offset=-1",
        "/q/dead-letters?offset=x",
    ] {
        let (s, b) = send(&st, "GET", path).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{path}: {b}");
        assert!(
            b["type"]
                .as_str()
                .is_some_and(|t| t.ends_with("BadRequestData")),
            "{b}"
        );
    }
    let (s, _) = send(&st, "DELETE", "/q/dead-letters/urn:x?tenant=snap-abc").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = send(&st, "POST", "/q/dead-letters/urn:x/replay?tenant=snap-abc").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_endpoint_userinfo_is_redacted_in_the_listing_only() {
    let st = state();
    seed(
        &st,
        "acme",
        "urn:ngsi-ld:DeadLetter:1",
        "urn:s:1",
        "http://bob:s3cret@127.0.0.1:9/n",
        "2026-01-01T00:00:01Z",
    )
    .await;
    let (_, b) = send(&st, "GET", "/q/dead-letters?tenant=acme").await;
    assert!(!b.to_string().contains("s3cret"), "{b}");
    let (_, one) = send(
        &st,
        "GET",
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:1?tenant=acme",
    )
    .await;
    assert!(
        one["id"] == "urn:ngsi-ld:DeadLetter:1" && !one.to_string().contains("s3cret"),
        "{one}"
    );
    assert!(
        b[0]["uri"]
            .as_str()
            .is_some_and(|u| u.contains("127.0.0.1:9/n")),
        "{b}"
    );
    // the stored letter keeps the URI a replay needs
    assert_eq!(
        stored(&st, "acme", "urn:ngsi-ld:DeadLetter:1")
            .await
            .expect("kept")["uri"],
        "http://bob:s3cret@127.0.0.1:9/n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn replay_delivers_the_same_request_and_deletes_the_letter() {
    let st = state();
    let (uri, hits, seen) = receiver(StatusCode::OK).await;
    seed(
        &st,
        "acme",
        "urn:ngsi-ld:DeadLetter:1",
        "urn:s:1",
        &uri,
        "2026-01-01T00:00:01Z",
    )
    .await;
    let (s, b) = send(
        &st,
        "POST",
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:1/replay?tenant=acme",
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let (headers, body) = seen.lock().expect("lock")[0].clone();
    assert!(
        headers.contains(&("content-type".into(), "application/json".into())),
        "{headers:?}"
    );
    assert!(
        headers.contains(&("ngsild-tenant".into(), "acme".into())),
        "{headers:?}"
    );
    assert_eq!(body["type"], "Notification");
    assert_eq!(body["data"][0]["id"], "urn:ngsi-ld:Room:1");
    assert!(
        stored(&st, "acme", "urn:ngsi-ld:DeadLetter:1")
            .await
            .is_none(),
        "delivered letter is gone"
    );
    let (s, _) = send(
        &st,
        "POST",
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:1/replay?tenant=acme",
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "a second replay finds nothing");
}

/// A replay that delivered but whose letter cannot be deleted answers the
/// store's error, and the letter stays to be replayed or deleted again.
#[tokio::test(flavor = "multi_thread")]
async fn a_delivered_letter_the_store_cannot_delete_answers_its_error() {
    use crate::common::faults::{store_down, Faults, StoreFaults};
    let mut st = state();
    let (uri, hits, _) = receiver(StatusCode::OK).await;
    seed(
        &st,
        "acme",
        "urn:ngsi-ld:DeadLetter:1",
        "urn:s:1",
        &uri,
        "2026-01-01T00:00:01Z",
    )
    .await;
    st.store = Arc::new(StoreFaults {
        inner: st.store.clone(),
        faults: Faults::new(|m, _| (m == "delete").then(store_down)),
    });
    let (s, b) = send(
        &st,
        "POST",
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:1/replay?tenant=acme",
    )
    .await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{b}");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "the replay was delivered");
    assert!(
        stored(&st, "acme", "urn:ngsi-ld:DeadLetter:1")
            .await
            .is_some(),
        "the letter is kept"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_replay_keeps_the_letter_and_extends_its_history() {
    let st = state();
    let (uri, hits, _) = receiver(StatusCode::SERVICE_UNAVAILABLE).await;
    seed(
        &st,
        "acme",
        "urn:ngsi-ld:DeadLetter:1",
        "urn:s:1",
        &uri,
        "2026-01-01T00:00:01Z",
    )
    .await;
    let (s, b) = send(
        &st,
        "POST",
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:1/replay?tenant=acme",
    )
    .await;
    assert_eq!(s, StatusCode::BAD_GATEWAY, "{b}");
    assert_eq!(b["detail"], "HTTP 503");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "replay is ONE attempt, never a retry loop"
    );
    let l = stored(&st, "acme", "urn:ngsi-ld:DeadLetter:1")
        .await
        .expect("kept");
    assert_eq!(l["attempts"], 3);
    assert_eq!(l["lastError"], "HTTP 503");
    assert_ne!(l["lastAt"], "2026-01-01T00:00:01Z", "lastAt moved");
    assert_eq!(l["firstAt"], "2026-01-01T00:00:00Z", "firstAt never moves");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replay_from_another_tenant_cannot_reach_the_letter() {
    let st = state();
    let (uri, hits, _) = receiver(StatusCode::OK).await;
    seed(
        &st,
        "acme",
        "urn:ngsi-ld:DeadLetter:1",
        "urn:s:1",
        &uri,
        "2026-01-01T00:00:01Z",
    )
    .await;
    for path in [
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:1/replay?tenant=other",
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:1/replay",
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:1?tenant=other",
    ] {
        let method = if path.contains("replay") {
            "POST"
        } else {
            "DELETE"
        };
        let (s, _) = send(&st, method, path).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "nothing was sent on behalf of the wrong tenant"
    );
    assert!(
        stored(&st, "acme", "urn:ngsi-ld:DeadLetter:1")
            .await
            .is_some(),
        "letter untouched"
    );
}

/// A broker that retries once, its subscription over `Vehicle` notifying
/// `uri`, and the handle of its store before `faults` wrap it.
async fn retrying_broker(
    uri: &str,
    faults: impl Fn(&str, usize) -> Option<antares_model::NgsiError> + Send + Sync + 'static,
    seed_letters: usize,
) -> (
    AppState,
    Arc<dyn antares_store::CurrentStateDriver>,
    Arc<crate::common::faults::StoreFaults>,
) {
    use crate::common::faults::{Faults, StoreFaults};
    let mut st = state();
    st.delivery = antares_notifier::DeliveryPolicy {
        attempts: 2,
        backoff: std::time::Duration::from_millis(20),
        jitter: 0.0,
        max_age: std::time::Duration::from_secs(60),
    };
    let t = TenantId::default();
    for i in 0..seed_letters {
        let id = format!("urn:ngsi-ld:DeadLetter:{i:013}-x");
        st.store
            .create(
                &t,
                Kind::DeadLetter,
                &id,
                letter(&id, "urn:s:old", uri, "2026-01-01T00:00:00Z"),
            )
            .await
            .expect("seed letter");
    }
    let inner = st.store.clone();
    let wrapped = Arc::new(StoreFaults {
        inner: inner.clone(),
        faults: Faults::new(faults),
    });
    st.store = wrapped.clone();
    antares_api::wire(&mut st).await;
    let sub = json!({"type": "Subscription", "entities": [{"type": "Vehicle"}],
        "notification": {"endpoint": {"uri": uri}}})
    .to_string();
    for (path, body) in [
        ("subscriptions", sub),
        (
            "entities",
            json!({"id": "urn:ngsi-ld:Vehicle:dl", "type": "Vehicle",
                "speed": {"type": "Property", "value": 1}})
            .to_string(),
        ),
    ] {
        let res = antares_api::router(st.clone())
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/ngsi-ld/v1/{path}"))
                    .header("Content-Type", "application/json")
                    .header("Content-Length", body.len())
                    .body(Body::from(body))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(res.status(), StatusCode::CREATED, "{path}");
    }
    (st, inner, wrapped)
}

/// Waits until `cond` holds, polling the broker's own store.
async fn until<F, Fut>(mut cond: F, what: &str)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..200 {
        if cond().await {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(cond().await, "timed out waiting for {what}");
}

/// S4-31: a letter whose cap cannot be counted, or whose oldest letters
/// cannot be read, stays parked; the cap is only skipped.
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_letter_is_kept_when_the_cap_cannot_be_checked() {
    use crate::common::faults::store_down;
    let (uri, _, _) = receiver(StatusCode::INTERNAL_SERVER_ERROR).await;
    let t = TenantId::default();

    // the count of the tenant's letters fails
    let (_st, inner, wrapped) =
        retrying_broker(&uri, |m, _| (m == "list_slice").then(store_down), 0).await;
    until(
        || async { inner.list(&t, Kind::DeadLetter).await.expect("list").len() == 1 },
        "the letter",
    )
    .await;
    assert!(wrapped.faults.calls("list_slice") >= 1);

    // the count works, the read of the oldest letters past the cap fails
    let (_st, inner, wrapped) = retrying_broker(
        &uri,
        |m, n| (m == "list_slice" && n >= 1).then(store_down),
        1_000,
    )
    .await;
    until(
        || async { inner.list(&t, Kind::DeadLetter).await.expect("list").len() == 1_001 },
        "the letter past the cap",
    )
    .await;
    until(
        || async { wrapped.faults.calls("list_slice") >= 2 },
        "the oldest-letters read",
    )
    .await;
}

/// A letter past the cap whose oldest letter the store cannot delete is
/// parked all the same: the eviction is only skipped.
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_letter_is_kept_when_the_oldest_cannot_be_evicted() {
    use crate::common::faults::store_down;
    let (uri, _, _) = receiver(StatusCode::INTERNAL_SERVER_ERROR).await;
    let t = TenantId::default();
    let (_st, inner, wrapped) =
        retrying_broker(&uri, |m, _| (m == "delete").then(store_down), 1_000).await;
    until(
        || async { wrapped.faults.calls("delete") >= 1 },
        "the eviction of the oldest letter",
    )
    .await;
    assert_eq!(
        inner.list(&t, Kind::DeadLetter).await.expect("list").len(),
        1_001,
        "the new letter is kept and the oldest stays"
    );
}

/// An exhausted retry whose letter the store refuses parks nothing; the
/// broker goes on.
#[tokio::test(flavor = "multi_thread")]
async fn an_exhausted_retry_the_store_cannot_park_leaves_no_letter() {
    use crate::common::faults::store_down;
    use std::sync::atomic::AtomicBool;
    let (uri, hits, _) = receiver(StatusCode::INTERNAL_SERVER_ERROR).await;
    let refuse = Arc::new(AtomicBool::new(false));
    let on = refuse.clone();
    let (_st, inner, wrapped) = retrying_broker(
        &uri,
        move |m, _| (m == "create" && on.load(Ordering::SeqCst)).then(store_down),
        0,
    )
    .await;
    let before = wrapped.faults.calls("create");
    refuse.store(true, Ordering::SeqCst);
    until(|| async { hits.load(Ordering::SeqCst) >= 2 }, "the retry").await;
    until(
        || async { wrapped.faults.calls("create") > before },
        "the refused letter",
    )
    .await;
    let t = TenantId::default();
    assert!(inner
        .list(&t, Kind::DeadLetter)
        .await
        .expect("list")
        .is_empty());
}

/// A retry that succeeds but whose bookkeeping cannot be written back is
/// still a delivered notification: no dead letter is parked.
#[tokio::test(flavor = "multi_thread")]
async fn a_retry_whose_writeback_fails_leaves_no_dead_letter() {
    use crate::common::faults::store_down;
    let hits: Arc<AtomicUsize> = Arc::default();
    let seen = hits.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = axum::Router::new().route(
        "/notify",
        axum::routing::post(move || {
            let seen = seen.clone();
            async move {
                if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    StatusCode::INTERNAL_SERVER_ERROR
                } else {
                    StatusCode::OK
                }
            }
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    let uri = format!("http://{addr}/notify");
    let (_st, inner, wrapped) =
        retrying_broker(&uri, |m, _| (m == "mutate_boxed").then(store_down), 0).await;
    until(|| async { hits.load(Ordering::SeqCst) >= 2 }, "the retry").await;
    until(
        || async { wrapped.faults.calls("mutate_boxed") >= 1 },
        "the bookkeeping writeback",
    )
    .await;
    let t = TenantId::default();
    assert!(inner
        .list(&t, Kind::DeadLetter)
        .await
        .expect("list")
        .is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_removes_one_letter_once() {
    let st = state();
    seed(
        &st,
        "acme",
        "urn:ngsi-ld:DeadLetter:1",
        "urn:s:1",
        "http://127.0.0.1:9/n",
        "2026-01-01T00:00:01Z",
    )
    .await;
    seed(
        &st,
        "acme",
        "urn:ngsi-ld:DeadLetter:2",
        "urn:s:1",
        "http://127.0.0.1:9/n",
        "2026-01-01T00:00:02Z",
    )
    .await;
    let (s, _) = send(
        &st,
        "DELETE",
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:1?tenant=acme",
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = send(
        &st,
        "DELETE",
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:1?tenant=acme",
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(
        stored(&st, "acme", "urn:ngsi-ld:DeadLetter:2")
            .await
            .is_some(),
        "the sibling stays"
    );
}

/// The letter's endpoint URI decides the binding, exactly as it does for a
/// fresh send: a scheme no sink serves, or a letter that cannot be read back
/// into a notification, answers 502 and puts nothing on the wire.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_binding_or_unreadable_letter_is_a_502_not_a_send() {
    let st = state();
    let (uri, hits, _) = receiver(StatusCode::OK).await;
    let t = TenantId::new("acme").expect("tenant");
    subscribe(&st, &t, "urn:s:smtp", "smtp://mail.example.org/queue").await;
    subscribe(&st, &t, "urn:s:1", &uri).await;
    let mut l = letter(
        "urn:ngsi-ld:DeadLetter:1",
        "urn:s:smtp",
        "smtp://mail.example.org/queue",
        "2026-01-01T00:00:01Z",
    );
    l["binding"] = json!("smtp");
    st.store
        .create(&t, Kind::DeadLetter, "urn:ngsi-ld:DeadLetter:1", l)
        .await
        .expect("seed");
    let mut l = letter(
        "urn:ngsi-ld:DeadLetter:2",
        "urn:s:1",
        &uri,
        "2026-01-01T00:00:01Z",
    );
    l["headers"] = json!("not a list");
    st.store
        .create(&t, Kind::DeadLetter, "urn:ngsi-ld:DeadLetter:2", l)
        .await
        .expect("seed");
    for id in ["urn:ngsi-ld:DeadLetter:1", "urn:ngsi-ld:DeadLetter:2"] {
        let (s, b) = send(
            &st,
            "POST",
            &format!("/q/dead-letters/{id}/replay?tenant=acme"),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_GATEWAY, "{b}");
        assert!(stored(&st, "acme", id).await.is_some());
    }
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

/// A replay re-sends a body built earlier: a subscription that is gone,
/// paused or names another endpoint now receives nothing, and the letter
/// stays as it was (409, no attempt counted).
#[tokio::test(flavor = "multi_thread")]
async fn replay_refuses_a_gone_paused_or_redirected_subscription() {
    for change in ["delete", "pause", "redirect"] {
        let st = state();
        let (uri, hits, _) = receiver(StatusCode::OK).await;
        seed(
            &st,
            "acme",
            "urn:ngsi-ld:DeadLetter:1",
            "urn:s:1",
            &uri,
            "2026-01-01T00:00:01Z",
        )
        .await;
        let t = TenantId::new("acme").expect("tenant");
        if change == "delete" {
            st.store
                .delete(&t, Kind::Subscription, "urn:s:1")
                .await
                .expect("delete");
        } else {
            st.store
                .mutate(&t, Kind::Subscription, "urn:s:1", |doc| {
                    if change == "pause" {
                        doc["isActive"] = json!(false);
                    } else {
                        doc["notification"]["endpoint"]["uri"] =
                            json!("http://127.0.0.1:9/elsewhere");
                    }
                    Ok::<(), antares_model::NgsiError>(())
                })
                .await
                .expect("change");
        }
        let (s, b) = send(
            &st,
            "POST",
            "/q/dead-letters/urn:ngsi-ld:DeadLetter:1/replay?tenant=acme",
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT, "{change}: {b}");
        assert_eq!(hits.load(Ordering::SeqCst), 0, "{change}: nothing sent");
        let l = stored(&st, "acme", "urn:ngsi-ld:DeadLetter:1")
            .await
            .expect("kept");
        assert_eq!(l["attempts"], 2, "{change}: a refusal is no attempt");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn replay_respects_the_egress_policy_of_the_moment() {
    // The refusing policy is set on the state itself: the env var is
    // process-global and the other tests in this binary read it too.
    let mut st = state();
    st.egress = Arc::new(antares_api::egress::Egress::new(
        antares_jsonld::EgressPolicy {
            allow_private: false,
            private_hosts: &[],
        },
    ));
    let (uri, hits, _) = receiver(StatusCode::OK).await;
    seed(
        &st,
        "acme",
        "urn:ngsi-ld:DeadLetter:1",
        "urn:s:1",
        &uri,
        "2026-01-01T00:00:01Z",
    )
    .await;
    let (s, b) = send(
        &st,
        "POST",
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:1/replay?tenant=acme",
    )
    .await;
    assert_eq!(s, StatusCode::BAD_GATEWAY, "{b}");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "a private-range endpoint is refused, not tried"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dead_letter_routes_are_not_under_the_api_root_and_health_counts_them() {
    let st = state();
    for (m, p) in [
        ("GET", "/ngsi-ld/v1/q/dead-letters"),
        ("GET", "/ngsi-ld/v1/dead-letters"),
        ("POST", "/ngsi-ld/v1/dead-letters/x/replay"),
    ] {
        let (s, _) = send(&st, m, p).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{p}");
    }
    let (s, b) = send(&st, "GET", "/q/health").await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["deadLetters"].is_u64(), "{b}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_listing_blanks_every_credential_a_letter_carries() {
    let st = state();
    let t = TenantId::new("acme").expect("tenant");
    let id = "urn:ngsi-ld:DeadLetter:9";
    let mut l = letter(
        id,
        "urn:s:9",
        "http://127.0.0.1:9/n",
        "2026-01-01T00:00:01Z",
    );
    // 5.2.22: a KeyValuePair's example content is "the HTTP Authentication
    // Header", so receiverInfo is where a subscriber's endpoint credential
    // lives. notifierInfo is opaque to every sink but the endpoint's own, so
    // the broker cannot tell which of its keys names a secret.
    l["receiverInfo"] = json!([["Authorization", "Bearer s3cret-token"]]);
    l["notifierInfo"] = json!([["MQTT-QoS", "1"], ["MQTT-Password", "hunter2"]]);
    // a letter written before the bindings moved behind the registry carries
    // the same values already rendered into headers
    l["headers"] = json!([
        ["Content-Type", "application/json"],
        ["Authorization", "Bearer legacy-token"]
    ]);
    assert!(st
        .store
        .create(&t, Kind::DeadLetter, id, l)
        .await
        .expect("seed"));

    let (_, b) = send(&st, "GET", "/q/dead-letters?tenant=acme").await;
    let shown = b.to_string();
    for secret in ["s3cret-token", "hunter2", "legacy-token"] {
        assert!(!shown.contains(secret), "{secret} leaked: {shown}");
    }
    // the keys stay: an operator has to see which parameters were set
    for key in ["Authorization", "MQTT-QoS", "Content-Type"] {
        assert!(shown.contains(key), "{key} lost: {shown}");
    }
    // the stored letter keeps what a replay has to send
    let kept = stored(&st, "acme", id).await.expect("kept");
    assert_eq!(kept["receiverInfo"][0][1], "Bearer s3cret-token");
    assert_eq!(kept["notifierInfo"][1][1], "hunter2");
    assert_eq!(kept["headers"][1][1], "Bearer legacy-token");
}

/// A DeadLetter row that is not a JSON object reaches the listing the same
/// way every other row does — the store hands back whatever it holds, and a
/// row written by an older binary, a migration or an operator with psql is
/// not this process's own `json!` literal. Redaction has to survive it: the
/// listing is the route that exists so credentials do NOT leave, and a panic
/// there takes the connection with it.
#[tokio::test]
async fn a_stored_letter_of_the_wrong_shape_does_not_take_the_listing_down() {
    let st = state();
    let t = TenantId::new("acme").expect("tenant");
    for (id, doc) in [
        ("urn:ngsi-ld:DeadLetter:string", Value::String("x".into())),
        ("urn:ngsi-ld:DeadLetter:array", Value::Array(vec![])),
        ("urn:ngsi-ld:DeadLetter:number", Value::from(7)),
        ("urn:ngsi-ld:DeadLetter:null", Value::Null),
    ] {
        assert!(st
            .store
            .create(&t, Kind::DeadLetter, id, doc)
            .await
            .expect("seed"));
    }
    seed(
        &st,
        "acme",
        "urn:ngsi-ld:DeadLetter:ok",
        "urn:s:1",
        "http://user:secret@127.0.0.1:9/n",
        "2026-01-01T00:00:01Z",
    )
    .await;

    let (s, b) = send(&st, "GET", "/q/dead-letters?tenant=acme").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let listed = b.as_array().expect("array");
    assert_eq!(listed.len(), 5, "every row is listed: {b}");
    assert!(
        !b.to_string().contains("secret"),
        "the userinfo of the well-formed row is still redacted: {b}"
    );

    // and the replay bookkeeping writes through the same shape
    let (s, _) = send(
        &st,
        "POST",
        "/q/dead-letters/urn:ngsi-ld:DeadLetter:string/replay?tenant=acme",
    )
    .await;
    assert_eq!(s, StatusCode::BAD_GATEWAY, "an unreadable letter is a 502");
}
