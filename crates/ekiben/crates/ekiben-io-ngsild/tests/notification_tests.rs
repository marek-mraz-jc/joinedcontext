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
async fn notification_limits_take_their_ceilings() {
    let (sctx, _p, _stop) = ctx("org-n8n", true);
    let c = json!({"mode": "notification", "path": "/notify", "auth": {"bearer": TOKEN},
        "max_body_bytes": "16MiB", "max_in_flight": 1024, "dedupe_cap": 1_000_000});
    assert!(input(c, &sctx).is_ok());
}

#[tokio::test]
async fn a_stop_while_waiting_for_a_notification_ends_the_read() {
    let (mut i, sctx, stop) = listening("org-n8o", json!({})).await;
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = stop.send(true);
    });
    let res = tokio::time::timeout(Duration::from_secs(5), i.read_batch(&sctx))
        .await
        .expect("the stop ends the wait");
    assert!(matches!(res, Err(CoreError::EndOfInput)));
}

#[tokio::test]
async fn a_notification_or_entity_with_an_empty_id_is_400() {
    let (_i, sctx, _stop) = listening("org-n8p", json!({})).await;
    let (org, stream) = stream_of(&sctx);
    for (body, want) in [
        (
            json!({"id": "", "type": "Notification", "subscriptionId": "urn:s", "notifiedAt": "2026-11-08T10:00:00Z", "data": []}),
            "the Notification has no id",
        ),
        (
            json!({"id": "urn:n", "type": "Notification", "subscriptionId": "urn:s", "notifiedAt": "2026-11-08T10:00:00Z", "data": [{"id": "", "type": "T"}]}),
            "data item 0 is not an entity with an id",
        ),
    ] {
        let a = dispatch(
            &org,
            &stream,
            "/notify",
            &headers(Some(TOKEN)),
            Bytes::from(body.to_string()),
        )
        .await;
        assert_eq!((a.status, a.body.as_str()), (400, want));
    }
}

#[tokio::test]
async fn a_notification_of_a_few_kib_fits_the_default_body_cap() {
    let (mut i, sctx, _stop) = listening("org-n8m", json!({})).await;
    let (org, stream) = stream_of(&sctx);
    let body = notification("urn:n:8m", 60);
    assert!(body.len() > 4096, "{}", body.len());
    let call = tokio::spawn(async move {
        dispatch(&org, &stream, "/notify", &headers(Some(TOKEN)), body).await
    });
    let (b, ack) = i.read_batch(&sctx).await.expect("delivered");
    assert_eq!(b.len(), 60);
    ack.ack();
    assert_eq!(call.await.expect("joined").status, 204);
}

#[tokio::test]
async fn notification_limits_are_refused_at_build() {
    let (sctx, _p, _stop) = ctx("org-n8c", true);
    let base = json!({"mode": "notification", "path": "/notify", "auth": {"bearer": TOKEN}});
    for (extra, want) in [
        (
            json!({"auth": {"bearer": ""}}),
            "auth needs bearer, or public: true",
        ),
        (
            json!({"auth": {"bearer": TOKEN, "public": true}}),
            "bearer or public: true, not both",
        ),
        (
            json!({"max_body_bytes": 0}),
            "max_body_bytes must be between 1 and",
        ),
        (
            json!({"max_body_bytes": "17MiB"}),
            "max_body_bytes must be between 1 and",
        ),
        (
            json!({"max_in_flight": 0}),
            "max_in_flight must be between 1 and",
        ),
        (
            json!({"max_in_flight": 1025}),
            "max_in_flight must be between 1 and",
        ),
        (json!({"timeout": "0s"}), "timeout must be above zero"),
        (
            json!({"dedupe_cap": 100_000_000}),
            "dedupe_cap may be at most",
        ),
    ] {
        let mut c = base.clone();
        if let (Some(m), JsonValue::Object(e)) = (c.as_object_mut(), extra) {
            m.extend(e);
        }
        let e = input(c.clone(), &sctx)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(e.contains(want), "{c}: {e}");
        assert!(!e.contains(TOKEN), "{e}");
    }
}

#[tokio::test]
async fn without_dedupe_an_empty_notification_is_204_and_a_repeat_is_written_again() {
    let (mut i, sctx, _stop) = listening("org-n8d", json!({"dedupe_cap": 0})).await;
    let (org, stream) = stream_of(&sctx);
    let empty = dispatch(
        &org,
        &stream,
        "/notify",
        &headers(Some(TOKEN)),
        notification("urn:n:8d0", 0),
    )
    .await;
    assert_eq!(empty.status, 204);
    for round in 0..2 {
        let (o, s) = (org.clone(), stream.clone());
        let call = tokio::spawn(async move {
            dispatch(
                &o,
                &s,
                "/notify",
                &headers(Some(TOKEN)),
                notification("urn:n:8d1", 1),
            )
            .await
        });
        let (b, ack) = i.read_batch(&sctx).await.expect("delivered");
        assert_eq!(b.len(), 1, "round {round}: emitted again without dedupe");
        ack.ack();
        assert_eq!(call.await.expect("joined").status, 204);
    }
}

#[tokio::test]
async fn a_busy_answer_carries_retry_after_on_the_wire() {
    use axum::response::IntoResponse;
    let (_i, sctx, _stop) =
        listening("org-n8e", json!({"max_in_flight": 1, "timeout": "5s"})).await;
    let (org, stream) = stream_of(&sctx);
    let (o, s) = (org.clone(), stream.clone());
    let first = tokio::spawn(async move {
        dispatch(
            &o,
            &s,
            "/notify",
            &headers(Some(TOKEN)),
            notification("urn:n:8e", 1),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let busy = dispatch(
        &org,
        &stream,
        "/notify",
        &headers(Some(TOKEN)),
        notification("urn:n:8f", 1),
    )
    .await
    .into_response();
    assert_eq!(busy.status().as_u16(), 503);
    assert_eq!(
        busy.headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("1")
    );
    first.abort();
}

#[tokio::test]
async fn closing_the_input_while_a_notification_waits_answers_503() {
    let (mut i, sctx, _stop) = listening("org-n8g", json!({"timeout": "5s"})).await;
    let (org, stream) = stream_of(&sctx);
    let call = tokio::spawn(async move {
        dispatch(
            &org,
            &stream,
            "/notify",
            &headers(Some(TOKEN)),
            notification("urn:n:8g", 1),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    i.close().await.expect("closes");
    let a = call.await.expect("joined");
    assert_eq!(a.status, 503);
    assert_eq!(
        a.body,
        "the stream stopped before the Notification was written"
    );
}

#[tokio::test]
async fn a_stopped_stream_ends_the_read_and_a_dropped_stop_signal_keeps_it_listening() {
    let (mut i, sctx, stop) = listening("org-n8h", json!({})).await;
    stop.send(true).expect("stop");
    assert!(matches!(
        i.read_batch(&sctx).await,
        Err(CoreError::EndOfInput)
    ));

    let (mut i, sctx, stop) = listening("org-n8i", json!({})).await;
    tokio::spawn(async move {
        // A signal that says "running" keeps the read waiting; then the signal goes away.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let _ = stop.send(false);
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(stop);
    });
    let (org, stream) = stream_of(&sctx);
    let call = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        dispatch(
            &org,
            &stream,
            "/notify",
            &headers(Some(TOKEN)),
            notification("urn:n:8i", 1),
        )
        .await
    });
    let (b, ack) = i.read_batch(&sctx).await.expect("still delivered");
    assert_eq!(b.len(), 1);
    ack.ack();
    assert_eq!(call.await.expect("joined").status, 204);
}

#[tokio::test]
async fn a_read_before_connect_registers_and_a_second_connect_keeps_the_route() {
    let (sctx, _p, _stop) = ctx("org-n8j", true);
    let mut i = input(
        json!({"mode": "notification", "path": "/notify", "auth": {"bearer": TOKEN}}),
        &sctx,
    )
    .expect("builds");
    let (org, stream) = stream_of(&sctx);
    let call = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        dispatch(
            &org,
            &stream,
            "/notify",
            &headers(Some(TOKEN)),
            notification("urn:n:8j", 1),
        )
        .await
    });
    let (b, ack) = i.read_batch(&sctx).await.expect("read connects");
    assert_eq!(b.len(), 1);
    ack.ack();
    assert_eq!(call.await.expect("joined").status, 204);
    i.connect(&sctx).await.expect("a second connect is a no-op");
    let (org, stream) = stream_of(&sctx);
    // The same empty Notification twice: remembered once, 204 both times.
    for _ in 0..2 {
        let a = dispatch(
            &org,
            &stream,
            "/notify",
            &headers(Some(TOKEN)),
            notification("urn:n:8k", 0),
        )
        .await;
        assert_eq!(a.status, 204);
    }
}

#[tokio::test]
async fn closing_a_replaced_registration_keeps_the_new_one() {
    let (mut first, sctx, _stop) = listening("org-n8l", json!({})).await;
    let mut second = input(
        json!({"mode": "notification", "path": "/notify", "auth": {"bearer": TOKEN}}),
        &sctx,
    )
    .expect("builds");
    second.connect(&sctx).await.expect("connects");
    first.close().await.expect("closes");
    let (org, stream) = stream_of(&sctx);
    assert!(ekiben_io_ngsild::notification::has_route(
        &org, &stream, "/notify"
    ));
    let call = tokio::spawn(async move {
        dispatch(
            &org,
            &stream,
            "/notify",
            &headers(Some(TOKEN)),
            notification("urn:n:8l", 1),
        )
        .await
    });
    let (b, ack) = second
        .read_batch(&sctx)
        .await
        .expect("the new registration reads");
    assert_eq!(b.len(), 1);
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

#[tokio::test]
async fn the_router_serves_the_root_path() {
    let (mut i, sctx, _stop) = listening("org-n10b", json!({"path": "/"})).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router()).await;
    });
    let url = format!("http://{addr}/{}/{}", sctx.org.org_id.0, sctx.stream_id);
    let call = tokio::spawn(async move {
        reqwest::Client::new()
            .post(url)
            .header("content-type", "application/json")
            .bearer_auth(TOKEN)
            .body(notification("urn:n:10b", 1))
            .send()
            .await
            .map(|r| r.status().as_u16())
    });
    let (_b, ack) = i.read_batch(&sctx).await.expect("delivered");
    ack.ack();
    assert_eq!(call.await.expect("joined").expect("answered"), 204);
}

/// G-06: the shared ingest listener serves a registered Notification route (here without its
/// bearer: 401, from the Notification door) and hands every other path, and a GET of the
/// registered one, to the `http_server` ingest, which answers 404 for a route it does not hold.
#[tokio::test]
async fn the_ingest_listener_serves_notifications_beside_http_server_inputs() {
    let (_i, sctx, _stop) = listening("org-n11", json!({})).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            ekiben_io_ngsild::notification::ingest_router()
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    let base = format!("http://{addr}/{}/{}", sctx.org.org_id.0, sctx.stream_id);
    let client = reqwest::Client::new();
    let unauthorized = client
        .post(format!("{base}/notify"))
        .header("content-type", "application/json")
        .body(notification("urn:n:11", 1))
        .send()
        .await
        .expect("answered");
    assert_eq!(unauthorized.status().as_u16(), 401);
    for request in [
        client.post(format!("{base}/other")),
        client.get(format!("{base}/notify")),
        client.post(format!("http://{addr}/")),
    ] {
        let answer = request.send().await.expect("answered");
        assert_eq!(answer.status().as_u16(), 404);
        assert_eq!(
            answer.text().await.expect("text").trim_end(),
            "404 page not found"
        );
    }
    let too_big = client
        .post(format!("{base}/notify"))
        .header("content-type", "application/json")
        .bearer_auth(TOKEN)
        .body(vec![b' '; 16 * 1024 * 1024 + 1])
        .send()
        .await
        .expect("answered");
    assert_eq!(too_big.status().as_u16(), 413);
}
