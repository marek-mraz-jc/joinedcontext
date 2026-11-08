//! E6: the `notification` mode of the `ngsi_ld` input, through `dispatch` and the router.

mod common;

use std::time::Duration;

use axum::http::{HeaderMap, HeaderValue};
use bytes::Bytes;
use common::ctx;
use ekiben_core::{BatchError, ComponentConfig, CoreError, Input, StreamContext};
use ekiben_io_ngsild::input::NgsiLdInput;
use ekiben_io_ngsild::notification::{dispatch, router, IngestAnswer};
use serde_json::{json, Value as JsonValue};

const TOKEN: &str = "s3cret-notify-token";

fn input(config: JsonValue, sctx: &StreamContext) -> Result<NgsiLdInput, CoreError> {
    NgsiLdInput::from_config(
        &ComponentConfig {
            label: "notified".to_string(),
            component_type: "ngsi_ld".to_string(),
            config,
            processors: Vec::new(),
        },
        sctx,
    )
}

async fn listening(
    org: &str,
    extra: JsonValue,
) -> (NgsiLdInput, StreamContext, tokio::sync::watch::Sender<bool>) {
    let (sctx, _p, stop) = ctx(org, true);
    let mut c = json!({"mode": "notification", "path": "/notify", "auth": {"bearer": TOKEN}});
    if let (Some(m), JsonValue::Object(e)) = (c.as_object_mut(), extra) {
        for (k, v) in e {
            m.insert(k, v);
        }
    }
    let mut i = input(c, &sctx).expect("builds");
    i.connect(&sctx).await.expect("connects");
    (i, sctx, stop)
}

fn headers(token: Option<&str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("content-type", HeaderValue::from_static("application/json"));
    if let Some(t) = token {
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {t}")) {
            h.insert("authorization", v);
        }
    }
    h
}

fn notification(id: &str, entities: usize) -> Bytes {
    let data: Vec<JsonValue> = (0..entities)
        .map(|i| json!({"id": format!("urn:ngsi-ld:Vehicle:{i}"), "type": "Vehicle", "speed": {"type": "Property", "value": i}}))
        .collect();
    Bytes::from(
        json!({"id": id, "type": "Notification", "subscriptionId": "urn:ngsi-ld:Subscription:pl-1", "notifiedAt": "2026-11-08T10:00:00.000Z", "data": data})
            .to_string(),
    )
}

fn stream_of(sctx: &StreamContext) -> (String, String) {
    (sctx.org.org_id.0.clone(), sctx.stream_id.clone())
}

#[tokio::test]
async fn a_notification_is_one_batch_answered_after_the_ack_and_never_twice() {
    let (mut i, sctx, _stop) = listening("org-n1", json!({})).await;
    let (org, stream) = stream_of(&sctx);
    let o = org.clone();
    let s = stream.clone();
    let call = tokio::spawn(async move {
        dispatch(
            &o,
            &s,
            "/notify",
            &headers(Some(TOKEN)),
            notification("urn:n:1", 2),
        )
        .await
    });
    let (batch, ack) = i.read_batch(&sctx).await.expect("delivered");
    assert_eq!(batch.len(), 2);
    assert_eq!(
        batch[0].meta_get_str("ngsild_subscription_id"),
        "urn:ngsi-ld:Subscription:pl-1"
    );
    assert_eq!(batch[0].meta_get_str("ngsild_notification_id"), "urn:n:1");
    assert_eq!(
        batch[0].meta_get_str("ngsild_notified_at"),
        "2026-11-08T10:00:00.000Z"
    );
    assert!(!call.is_finished(), "the broker waits for the output's ack");
    ack.ack();
    let answer = call.await.expect("joined");
    assert_eq!(answer.status, 204);

    // Redelivery of the written Notification: 204, nothing emitted again.
    let again = dispatch(
        &org,
        &stream,
        "/notify",
        &headers(Some(TOKEN)),
        notification("urn:n:1", 2),
    )
    .await;
    assert_eq!(again.status, 204);
    let none = tokio::time::timeout(Duration::from_millis(100), i.read_batch(&sctx)).await;
    assert!(
        none.is_err(),
        "a redelivered Notification is not emitted twice"
    );
}

#[tokio::test]
async fn a_missing_or_wrong_token_is_401() {
    let (_i, sctx, _stop) = listening("org-n2", json!({})).await;
    let (org, stream) = stream_of(&sctx);
    for h in [
        headers(None),
        headers(Some("wrong")),
        headers(Some("s3cret-notify-toke")),
    ] {
        let a = dispatch(&org, &stream, "/notify", &h, notification("urn:n:2", 1)).await;
        assert_eq!(a.status, 401);
        assert!(!a.body.contains(TOKEN));
    }
}

#[tokio::test]
async fn unknown_routes_are_404_alike() {
    let (_i, sctx, _stop) = listening("org-n3", json!({})).await;
    let (org, stream) = stream_of(&sctx);
    let foreign = dispatch(
        "org-other",
        &stream,
        "/notify",
        &headers(Some(TOKEN)),
        notification("urn:n:3", 1),
    )
    .await;
    let missing = dispatch(
        &org,
        &stream,
        "/elsewhere",
        &headers(Some(TOKEN)),
        notification("urn:n:3", 1),
    )
    .await;
    assert_eq!(foreign.status, 404);
    assert_eq!(
        foreign, missing,
        "a foreign organization's route looks like no route"
    );
}

#[tokio::test]
async fn bad_bodies_are_refused_and_nothing_is_emitted() {
    let (mut i, sctx, _stop) = listening("org-n4", json!({"max_body_bytes": "1KiB"})).await;
    let (org, stream) = stream_of(&sctx);
    let big = dispatch(
        &org,
        &stream,
        "/notify",
        &headers(Some(TOKEN)),
        notification("urn:n:4", 50),
    )
    .await;
    assert_eq!(big.status, 413);
    let mut text = headers(Some(TOKEN));
    text.insert("content-type", HeaderValue::from_static("text/plain"));
    let wrong_type = dispatch(&org, &stream, "/notify", &text, notification("urn:n:4", 1)).await;
    assert_eq!(wrong_type.status, 415);
    for body in [
        r#"{"type":"Notification","id":"x","notifiedAt":"2026-01-01T00:00:00Z","data":[]}"#,
        r#"{"type":"Notification","id":"x","subscriptionId":"s","notifiedAt":"2026-01-01T00:00:00Z","data":["x"]}"#,
        "not json",
    ] {
        let a = dispatch(
            &org,
            &stream,
            "/notify",
            &headers(Some(TOKEN)),
            Bytes::from(body),
        )
        .await;
        assert_eq!(a.status, 400, "{body}");
    }
    let none = tokio::time::timeout(Duration::from_millis(100), i.read_batch(&sctx)).await;
    assert!(none.is_err());
}

#[tokio::test]
async fn a_nack_is_502_and_the_redelivery_is_emitted_again() {
    let (mut i, sctx, _stop) = listening("org-n5", json!({})).await;
    let (org, stream) = stream_of(&sctx);
    let (o, s) = (org.clone(), stream.clone());
    let call = tokio::spawn(async move {
        dispatch(
            &o,
            &s,
            "/notify",
            &headers(Some(TOKEN)),
            notification("urn:n:5", 1),
        )
        .await
    });
    let (batch, ack) = i.read_batch(&sctx).await.expect("delivered");
    ack.nack(BatchError::new(batch, "the output refused it"));
    let a = call.await.expect("joined");
    assert_eq!(
        a,
        IngestAnswer {
            status: 502,
            body: "Server error".into(),
            retry_after: None
        }
    );

    let (o, s) = (org.clone(), stream.clone());
    let call = tokio::spawn(async move {
        dispatch(
            &o,
            &s,
            "/notify",
            &headers(Some(TOKEN)),
            notification("urn:n:5", 1),
        )
        .await
    });
    let (_batch, ack) = i.read_batch(&sctx).await.expect("delivered again");
    ack.ack();
    assert_eq!(call.await.expect("joined").status, 204);
}

#[tokio::test]
async fn no_ack_in_time_is_408_and_a_late_ack_still_counts() {
    let (mut i, sctx, _stop) = listening("org-n6", json!({"timeout": "100ms"})).await;
    let (org, stream) = stream_of(&sctx);
    let (o, s) = (org.clone(), stream.clone());
    let call = tokio::spawn(async move {
        dispatch(
            &o,
            &s,
            "/notify",
            &headers(Some(TOKEN)),
            notification("urn:n:6", 1),
        )
        .await
    });
    let (_batch, ack) = i.read_batch(&sctx).await.expect("delivered");
    let a = call.await.expect("joined");
    assert_eq!(a.status, 408);
    ack.ack();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let again = dispatch(
        &org,
        &stream,
        "/notify",
        &headers(Some(TOKEN)),
        notification("urn:n:6", 1),
    )
    .await;
    assert_eq!(again.status, 204);
}

#[tokio::test]
async fn a_full_input_is_503_with_retry_after() {
    let (_i, sctx, _stop) = listening("org-n7", json!({"max_in_flight": 1, "timeout": "2s"})).await;
    let (org, stream) = stream_of(&sctx);
    let (o, s) = (org.clone(), stream.clone());
    let first = tokio::spawn(async move {
        dispatch(
            &o,
            &s,
            "/notify",
            &headers(Some(TOKEN)),
            notification("urn:n:7a", 1),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let busy = dispatch(
        &org,
        &stream,
        "/notify",
        &headers(Some(TOKEN)),
        notification("urn:n:7b", 1),
    )
    .await;
    assert_eq!(busy.status, 503);
    assert_eq!(busy.retry_after, Some(1));
    let same = dispatch(
        &org,
        &stream,
        "/notify",
        &headers(Some(TOKEN)),
        notification("urn:n:7a", 1),
    )
    .await;
    assert_eq!(
        same.status, 503,
        "the same Notification in flight is not emitted twice"
    );
    first.abort();
}

#[tokio::test]
async fn unauthenticated_notifications_need_public_true() {
    let (sctx, _p, _stop) = ctx("org-n8", true);
    for c in [
        json!({"mode": "notification", "path": "/notify"}),
        json!({"mode": "notification", "path": "/notify", "auth": {}}),
        json!({"mode": "notification", "path": "/notify", "auth": {"public": true, "bearer": "x"}}),
        json!({"mode": "notification", "path": "no-slash", "auth": {"public": true}}),
        json!({"mode": "notification", "path": "/a/../b", "auth": {"public": true}}),
        json!({"mode": "notification", "url": "http://x/ngsi-ld/v1", "auth": {"public": true}}),
    ] {
        assert!(input(c.clone(), &sctx).is_err(), "{c} must be refused");
    }
    let (mut i, sctx, _stop) = listening("org-n8b", json!({"auth": {"public": true}})).await;
    let (org, stream) = stream_of(&sctx);
    let call = tokio::spawn(async move {
        dispatch(
            &org,
            &stream,
            "/notify",
            &headers(None),
            notification("urn:n:8", 1),
        )
        .await
    });
    let (_b, ack) = i.read_batch(&sctx).await.expect("delivered");
    ack.ack();
    assert_eq!(call.await.expect("joined").status, 204);
}

#[tokio::test]
async fn close_unregisters_the_route() {
    let (mut i, sctx, _stop) = listening("org-n9", json!({})).await;
    let (org, stream) = stream_of(&sctx);
    i.close().await.expect("closes");
    let a = dispatch(
        &org,
        &stream,
        "/notify",
        &headers(Some(TOKEN)),
        notification("urn:n:9", 1),
    )
    .await;
    assert_eq!(a.status, 404);
}

#[tokio::test]
async fn the_router_serves_a_route_over_http() {
    let (mut i, sctx, _stop) = listening("org-n10", json!({})).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router()).await;
    });
    let url = format!(
        "http://{addr}/{}/{}/notify",
        sctx.org.org_id.0, sctx.stream_id
    );
    let call = tokio::spawn(async move {
        reqwest::Client::new()
            .post(url)
            .header("content-type", "application/ld+json")
            .bearer_auth(TOKEN)
            .body(notification("urn:n:10", 1))
            .send()
            .await
            .map(|r| r.status().as_u16())
    });
    let (_b, ack) = i.read_batch(&sctx).await.expect("delivered");
    ack.ack();
    assert_eq!(call.await.expect("joined").expect("answered"), 204);
}
