// SPDX-License-Identifier: EUPL-1.2
//! Resource subscriptions through `subscriptions/listen` (MCP section 5,
//! D-7; MC7, MC17). The stream caps are per process and per (Tenant,
//! subject): the tests that listen in Tenant `city` take [`CITY`] in turn.

use crate::common::{endpoint, fixture, fixture_with, post_raw, prop, MODERN};
use crate::engines::NoSubscriptions;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout, Instant};
use tower::ServiceExt;

/// Held by every test that listens in Tenant `city`, so their streams never
/// meet the per-subject cap together.
static CITY: Mutex<()> = Mutex::const_new(());

/// An open `subscriptions/listen` response, read event by event.
struct Listening {
    status: StatusCode,
    body: Body,
    buffer: String,
}

impl Listening {
    /// The next JSON-RPC message, or `None` when none comes within `wait`.
    async fn next(&mut self, wait: Duration) -> Option<Value> {
        let deadline = Instant::now().checked_add(wait)?;
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let event: String = self.buffer.drain(..end.saturating_add(2)).collect();
                let data: String = event
                    .lines()
                    .filter_map(|l| l.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect();
                match serde_json::from_str(&data) {
                    Ok(v) => return Some(v),
                    Err(_) => continue,
                }
            }
            let frame = timeout(
                deadline.saturating_duration_since(Instant::now()),
                self.body.frame(),
            );
            let bytes = frame.await.ok()??.ok()?.into_data().ok()?;
            self.buffer.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
}

/// Listen on `view` for `uris`.
async fn listen(app: &Router, view: &str, uris: &[&str]) -> Listening {
    let body = json!({"jsonrpc": "2.0", "id": 7, "method": "subscriptions/listen", "params": {
        "notifications": {"resourceSubscriptions": uris, "toolsListChanged": true},
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": MODERN,
            "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {}
        }
    }})
    .to_string();
    let req = Request::builder()
        .method(Method::POST)
        .uri(endpoint(view))
        .header("Host", "localhost")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", MODERN)
        .header("Mcp-Method", "subscriptions/listen")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    Listening {
        status: resp.status(),
        body: resp.into_body(),
        buffer: String::new(),
    }
}

/// How many Subscriptions `tenant` holds.
async fn subscriptions(app: &Router, tenant: &str) -> usize {
    let req = Request::builder()
        .uri("/ngsi-ld/v1/subscriptions?limit=100")
        .header("NGSILD-Tenant", tenant)
        .body(Body::empty())
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(http_body_util::Collected::to_bytes)
        .unwrap_or_default();
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v.as_array().map(Vec::len))
        .unwrap_or_default()
}

/// Wait until `tenant` holds `n` Subscriptions.
async fn until_subscriptions(app: &Router, tenant: &str, n: usize) -> Result<(), String> {
    for _ in 0..100 {
        if subscriptions(app, tenant).await == n {
            return Ok(());
        }
        sleep(Duration::from_millis(20)).await;
    }
    Err(format!(
        "{tenant} never held {n} Subscriptions: {}",
        subscriptions(app, tenant).await
    ))
}

/// Patch `attrs` of `id` in `tenant` through the NGSI-LD root.
async fn patch(app: &Router, tenant: &str, id: &str, attrs: &Value) -> StatusCode {
    let body = attrs.to_string();
    let req = Request::builder()
        .method(Method::PATCH)
        .uri(format!("/ngsi-ld/v1/entities/{id}/attrs"))
        .header("NGSILD-Tenant", tenant)
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .unwrap_or_default();
    let Ok(resp) = app.clone().oneshot(req).await;
    resp.status()
}

fn updated(uri: &str) -> Value {
    json!({"jsonrpc": "2.0", "method": "notifications/resources/updated", "params": {"uri": uri}})
}

/// The `params` of a notification, without the stream tag rmcp adds.
fn shape(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(p) = v.pointer_mut("/params").and_then(Value::as_object_mut) {
        p.remove("_meta");
    }
    v
}

/// MC7, MC17: the acknowledgment names the view's Entity URIs only, and
/// says nothing of which exist; only a visible Entity is watched.
#[tokio::test(flavor = "multi_thread")]
async fn a_stream_watches_the_visible_entities_it_names_and_nothing_else() -> Result<(), String> {
    let _city = CITY.lock().await;
    let f = fixture().await?;
    let car = "ngsi-ld://watch/entities/urn:ngsi-ld:Car:1";
    let uris = [
        car,
        car,
        "ngsi-ld://watch/entities/urn:ngsi-ld:Car:3",
        "ngsi-ld://watch/entities/urn:ngsi-ld:Bike:b",
        "ngsi-ld://watch/entities/urn:ngsi-ld:Car:absent",
        "ngsi-ld://watch/types/Car",
        "ngsi-ld://cars/entities/urn:ngsi-ld:Car:1",
    ];
    let mut stream = listen(&f.app, "watch", &uris).await;
    assert_eq!(stream.status, StatusCode::OK);
    let ack = stream
        .next(Duration::from_secs(5))
        .await
        .unwrap_or_default();
    assert_eq!(
        ack.pointer("/params/notifications"),
        Some(&json!({"resourceSubscriptions": [
            car,
            car,
            "ngsi-ld://watch/entities/urn:ngsi-ld:Car:3",
            "ngsi-ld://watch/entities/urn:ngsi-ld:Bike:b",
            "ngsi-ld://watch/entities/urn:ngsi-ld:Car:absent"
        ]})),
        "{ack}"
    );
    until_subscriptions(&f.app, "city", 1).await?;
    let speed = json!({"speed": prop(&json!(11))});
    assert_eq!(
        patch(&f.app, "city", "urn:ngsi-ld:Car:1", &speed).await,
        StatusCode::NO_CONTENT
    );
    let event = stream
        .next(Duration::from_secs(5))
        .await
        .unwrap_or_default();
    assert_eq!(shape(&event), updated(car), "{event}");
    let hidden = json!({"speed": prop(&json!(91))});
    assert_eq!(
        patch(&f.app, "city", "urn:ngsi-ld:Car:3", &hidden).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(stream.next(Duration::from_millis(1500)).await, None);
    Ok(())
}

/// MC17, MCP 5.3: a change to a hidden Attribute is no change in a
/// watched one, so neither an event nor its timing tells of it.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_to_a_hidden_attribute_sends_no_resource_update() -> Result<(), String> {
    let _city = CITY.lock().await;
    let f = fixture().await?;
    let car = "ngsi-ld://watch/entities/urn:ngsi-ld:Car:2";
    let mut stream = listen(&f.app, "watch", &[car]).await;
    assert!(
        stream.next(Duration::from_secs(5)).await.is_some(),
        "no acknowledgment"
    );
    until_subscriptions(&f.app, "city", 1).await?;
    let secret = json!({"secret": prop(&json!("s"))});
    assert_eq!(
        patch(&f.app, "city", "urn:ngsi-ld:Car:2", &secret).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(stream.next(Duration::from_millis(1500)).await, None);
    let speed = json!({"speed": prop(&json!(2))});
    assert_eq!(
        patch(&f.app, "city", "urn:ngsi-ld:Car:2", &speed).await,
        StatusCode::NO_CONTENT
    );
    let event = stream
        .next(Duration::from_secs(5))
        .await
        .unwrap_or_default();
    assert_eq!(shape(&event), updated(car), "{event}");
    Ok(())
}

/// MCP 5.2 step 5: a closed stream deletes its internal Subscriptions.
#[tokio::test(flavor = "multi_thread")]
async fn a_closed_stream_deletes_its_internal_subscriptions() -> Result<(), String> {
    let _city = CITY.lock().await;
    let f = fixture().await?;
    let mut stream = listen(
        &f.app,
        "watch",
        &["ngsi-ld://watch/entities/urn:ngsi-ld:Car:1"],
    )
    .await;
    assert!(
        stream.next(Duration::from_secs(5)).await.is_some(),
        "no acknowledgment"
    );
    until_subscriptions(&f.app, "city", 1).await?;
    drop(stream);
    until_subscriptions(&f.app, "city", 0).await
}

/// MCP section 8: four streams per subject; the fifth is refused after
/// its acknowledgment, and a closed one frees its place.
#[tokio::test(flavor = "multi_thread")]
async fn listen_streams_are_capped_per_subject_tenant_and_process() -> Result<(), String> {
    let f = fixture().await?;
    let uri = ["ngsi-ld://writes/entities/urn:ngsi-ld:Car:w1"];
    let mut open = Vec::new();
    for _ in 0..4 {
        let mut s = listen(&f.app, "writes", &uri).await;
        assert!(
            s.next(Duration::from_secs(5)).await.is_some(),
            "no acknowledgment"
        );
        open.push(s);
    }
    until_subscriptions(&f.app, "w", 4).await?;
    let mut fifth = listen(&f.app, "writes", &uri).await;
    let _ack = fifth.next(Duration::from_secs(5)).await;
    let refused = fifth.next(Duration::from_secs(5)).await.unwrap_or_default();
    assert_eq!(
        refused.pointer("/error/message"),
        Some(&json!("this caller holds as many listen streams as it may")),
        "{refused}"
    );
    drop(open.pop());
    until_subscriptions(&f.app, "w", 3).await?;
    let mut again = listen(&f.app, "writes", &uri).await;
    assert!(again.next(Duration::from_secs(5)).await.is_some());
    until_subscriptions(&f.app, "w", 4).await
}

/// A URI whose id is no Entity id is acknowledged and never watched.
#[tokio::test(flavor = "multi_thread")]
async fn a_uri_that_names_no_entity_id_is_never_watched() -> Result<(), String> {
    let _city = CITY.lock().await;
    let f = fixture().await?;
    let car = "ngsi-ld://watch/entities/urn:ngsi-ld:Car:1";
    let uris = [
        car,
        "ngsi-ld://watch/entities/urn:ngsi-ld:Car:%zz",
        "ngsi-ld://watch/entities/Car:1",
    ];
    let mut stream = listen(&f.app, "watch", &uris).await;
    assert!(
        stream.next(Duration::from_secs(5)).await.is_some(),
        "no acknowledgment"
    );
    until_subscriptions(&f.app, "city", 1).await?;
    sleep(Duration::from_millis(200)).await;
    assert_eq!(subscriptions(&f.app, "city").await, 1);
    Ok(())
}

/// The internal Subscription is the caller's operation: one the engine
/// denies is never made, and the stream stays silent.
#[tokio::test(flavor = "multi_thread")]
async fn a_watch_the_engine_denies_makes_no_subscription() -> Result<(), String> {
    let _city = CITY.lock().await;
    let f = fixture_with(Some(Arc::new(NoSubscriptions))).await?;
    let car = "ngsi-ld://watch/entities/urn:ngsi-ld:Car:1";
    let mut stream = listen(&f.app, "watch", &[car]).await;
    assert!(
        stream.next(Duration::from_secs(5)).await.is_some(),
        "no acknowledgment"
    );
    let speed = json!({"speed": prop(&json!(12))});
    assert_eq!(
        patch(&f.app, "city", "urn:ngsi-ld:Car:1", &speed).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(stream.next(Duration::from_millis(1500)).await, None);
    assert_eq!(subscriptions(&f.app, "city").await, 0);
    Ok(())
}

/// A view that does not let its caller make a Subscription watches
/// nothing, and its acknowledgment says so.
#[tokio::test(flavor = "multi_thread")]
async fn a_view_without_subscriptions_acknowledges_no_uri() -> Result<(), String> {
    let _city = CITY.lock().await;
    let f = fixture().await?;
    let mut stream = listen(
        &f.app,
        "cars",
        &["ngsi-ld://cars/entities/urn:ngsi-ld:Car:1"],
    )
    .await;
    let ack = stream
        .next(Duration::from_secs(5))
        .await
        .unwrap_or_default();
    assert_eq!(
        ack.pointer("/params/notifications"),
        Some(&json!({})),
        "{ack}"
    );
    assert_eq!(subscriptions(&f.app, "city").await, 0);
    Ok(())
}

/// MCP 5.3: nobody names a stream's endpoint, over HTTP or a tool.
#[tokio::test(flavor = "multi_thread")]
async fn a_subscription_naming_a_stream_it_did_not_open_is_refused() -> Result<(), String> {
    let f = fixture().await?;
    let body = json!({"type": "Subscription", "entities": [{"type": "Car"}],
                      "notification": {"endpoint": {"uri": "x-antares-mcp:0123", "accept": "application/json"}}})
    .to_string();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/ngsi-ld/v1/subscriptions")
        .header("NGSILD-Tenant", "city")
        .header("Content-Type", "application/json")
        .header("Content-Length", body.len())
        .body(Body::from(body))
        .unwrap_or_default();
    let Ok(resp) = f.app.clone().oneshot(req).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(subscriptions(&f.app, "city").await, 0);
    Ok(())
}

/// D-8: a legacy client's `resources/subscribe` is not a method, and makes
/// no Subscription.
#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_client_gets_no_resource_subscription() -> Result<(), String> {
    let f = fixture().await?;
    let subscribe = json!({"jsonrpc": "2.0", "id": 1, "method": "resources/subscribe",
                           "params": {"uri": "ngsi-ld://watch/entities/urn:ngsi-ld:Car:1"}})
    .to_string();
    let header = [("MCP-Protocol-Version", "2025-06-18")];
    let a = post_raw(&f.app, &endpoint("watch"), &header, &subscribe).await;
    assert_eq!(a.error().get("code"), Some(&json!(-32601)), "{}", a.text);
    assert_eq!(subscriptions(&f.app, "city").await, 0);
    Ok(())
}
