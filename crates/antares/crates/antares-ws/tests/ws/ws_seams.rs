// SPDX-License-Identifier: EUPL-1.2
//! What a client can get wrong and what a socket can run into: malformed
//! Subscriptions and updates, control and binary frames, a silent or idle
//! peer, a view that refuses or goes away, refused upgrades, the end of
//! the grace period and a takeover of an attached connection.

use crate::common::{broker, cars, connect, open, status, text, until, with, Broker, Client};
use antares_ws::{Limits, ENDPOINT_PREFIX};
use futures_util::SinkExt;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

const ROOT: &str = "/x/ws/ngsi-ld/v1";

async fn update(c: &mut Client, sub: &str, fragment: Value) -> Result<u64, String> {
    c.send(
        json!({"x-op": "x-update", "x-subscription": sub}),
        Some(fragment),
    )
    .await?;
    Ok(status(&c.result().await?))
}

async fn unsubscribe(c: &mut Client, sub: &str) -> Result<u64, String> {
    c.send(
        json!({"x-op": "x-unsubscribe", "x-subscription": sub}),
        None,
    )
    .await?;
    Ok(status(&c.result().await?))
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_a_malformed_subscription_is_answered_and_forgotten() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let own = format!("{ENDPOINT_PREFIX}{}", c.connection);
    // a pinned core context: it resolves without the network (T-5245)
    let context = "https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld";
    let cases = [
        (with(cars(), &["@context"], json!(context)), 201),
        (
            with(cars(), &["notification", "endpoint", "uri"], json!(own)),
            201,
        ),
        (with(cars(), &["notification"], json!(5)), 400),
        (with(cars(), &["notification", "endpoint"], json!(5)), 400),
        (with(cars(), &["id"], json!(5)), 400),
        // Create Subscription itself refuses it: no entities, no
        // watchedAttributes
        (json!({"type": "Subscription"}), 400),
    ];
    for (body, expected) in cases {
        let r = c.subscribe(body.clone()).await?;
        assert_eq!(status(&r), expected, "{body} -> {r}");
    }
    let fixed = with(cars(), &["id"], json!("urn:ngsi-ld:Subscription:fixed"));
    assert_eq!(status(&c.subscribe(fixed.clone()).await?), 201);
    assert_eq!(status(&c.subscribe(fixed).await?), 409);
    // the refused Create left nothing on the connection: the three made
    // are all it deletes
    let (code, all) = b
        .http
        .call("GET", "/ngsi-ld/v1/subscriptions", &[], None)
        .await?;
    assert_eq!(code, 200, "{all}");
    assert_eq!(all.as_array().map(Vec::len), Some(3), "{all}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_update_and_unsubscribe_answer_every_case() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let own = format!("{ENDPOINT_PREFIX}{}", c.connection);
    let sub = text(&c.subscribe(cars()).await?, "/body/id");
    let endpoint = |uri: &str| json!({"notification": {"endpoint": {"uri": uri}}});
    assert_eq!(
        update(&mut c, "urn:ngsi-ld:Subscription:x", json!({})).await?,
        404
    );
    assert_eq!(update(&mut c, &sub, endpoint(&own)).await?, 204);
    assert_eq!(update(&mut c, &sub, endpoint("http://x/n")).await?, 400);
    assert_eq!(
        update(&mut c, &sub, json!({"watchedAttributes": 5})).await?,
        400
    );
    // a fragment's notification replaces the whole member, endpoint too
    let shown = json!({"notification": {"showChanges": true, "endpoint": {"uri": own}}});
    assert_eq!(update(&mut c, &sub, shown).await?, 204);
    b.http.car(None, "urn:ngsi-ld:Car:u", 1).await?;
    let n = c.notification().await?;
    assert_eq!(text(&n, "/metadata/x-subscription"), sub, "{n}");

    // deleted over HTTP first: the socket's delete is 404 and forgets it
    let path = format!("/ngsi-ld/v1/subscriptions/{sub}");
    let (code, body) = b.http.call("DELETE", &path, &[], None).await?;
    assert_eq!(code, 204, "{body}");
    assert_eq!(unsubscribe(&mut c, &sub).await?, 404);
    assert_eq!(unsubscribe(&mut c, &sub).await?, 404);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_control_frames_pass_and_a_binary_frame_closes() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    for frame in [
        Message::Ping(Vec::new().into()),
        Message::Pong(Vec::new().into()),
    ] {
        c.ws_mut().send(frame).await.map_err(|e| e.to_string())?;
    }
    assert_eq!(status(&c.subscribe(cars()).await?), 201);
    c.ws_mut()
        .send(Message::Binary(vec![1u8].into()))
        .await
        .map_err(|e| e.to_string())?;
    let (code, seen) = c.closed().await?;
    assert_eq!(code, 1003, "{seen:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_an_idle_socket_closes_and_a_silent_one_is_dropped() -> Result<(), String> {
    let b = broker(Limits {
        ping_every: Duration::from_millis(100),
        idle: Duration::from_millis(400),
        ..Limits::default()
    })
    .await?;
    // reading answers the Pings, but it says nothing
    let mut idle = connect(b.addr, ROOT, &[]).await?;
    let (code, seen) = idle.closed().await?;
    assert_eq!(code, 1000, "{seen:?}");
    assert!(
        seen.iter()
            .any(|m| m.pointer("/metadata/x-reason") == Some(&json!("idle"))),
        "{seen:?}"
    );

    let b = broker(Limits {
        ping_every: Duration::from_millis(100),
        pong_within: Duration::from_millis(400),
        ..Limits::default()
    })
    .await?;
    // never reads, so never answers a Ping
    let silent = connect(b.addr, ROOT, &[]).await?;
    until(&b.hub, |h| h.connections() == 0).await?;
    drop(silent);
    Ok(())
}

/// Load view `cars` of tenant `city` allowing `operations`.
async fn view(b: &Broker, operations: &[&str]) -> Result<(), String> {
    let view = json!({
        "id": "cars", "tenant": "city", "selector": ["Car"],
        "rules": [{"types": ["Car"], "attributes": ["speed"]}],
        "operations": operations
    });
    let (code, answer) = b
        .http
        .call("PUT", "/q/views/cars", &[], Some(&view))
        .await?;
    (code == 201 || code == 204)
        .then_some(())
        .ok_or(format!("view: {code} {answer}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_a_view_refuses_what_it_does_not_allow() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    view(
        &b,
        &[
            "create_subscription",
            "retrieve_subscription",
            "update_subscription",
        ],
    )
    .await?;
    let mut c = connect(b.addr, "/x/views/cars/ws", &[]).await?;
    let sub = text(&c.subscribe(cars()).await?, "/body/id");
    let refused = unsubscribe(&mut c, &sub).await?;
    assert!(refused >= 400, "{refused}");
    // removed between two Pings: the next operation finds no view
    let (code, answer) = b.http.call("DELETE", "/q/views/cars", &[], None).await?;
    assert_eq!(code, 204, "{answer}");
    let gone = update(&mut c, &sub, json!({"q": "speed>1"})).await?;
    assert!(gone >= 400, "{gone}");
    Ok(())
}

/// The status of an upgrade of `path` sent with raw `headers`.
async fn raw_upgrade(b: &Broker, path: &str, headers: &[u8]) -> Result<String, String> {
    let mut s = tokio::net::TcpStream::connect(b.addr)
        .await
        .map_err(|e| e.to_string())?;
    let mut req = format!(
        "GET {path} HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n"
    )
    .into_bytes();
    req.extend_from_slice(headers);
    req.extend_from_slice(b"Connection: close\r\n\r\n");
    s.write_all(&req).await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let _read = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    Ok(String::from_utf8_lossy(&out)
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned())
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_an_upgrade_naming_the_tenant_wrongly_is_refused() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    view(&b, &["create_subscription"]).await?;
    let two = open(
        b.addr,
        ROOT,
        &[("NGSILD-Tenant", "a"), ("NGSILD-Tenant", "b")],
    )
    .await?;
    assert_eq!(two.err().map(|r| r.status().as_u16()), Some(400));
    let unknown = open(b.addr, "/x/views/nope/ws", &[]).await?;
    assert_eq!(unknown.err().map(|r| r.status().as_u16()), Some(404));
    let other = open(b.addr, "/x/views/cars/ws", &[("NGSILD-Tenant", "town")]).await?;
    assert_eq!(other.err().map(|r| r.status().as_u16()), Some(400));
    assert_eq!(
        raw_upgrade(&b, ROOT, b"NGSILD-Tenant: \xff\r\n").await?,
        "400"
    );
    assert_eq!(b.hub.connections(), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_a_resume_within_the_grace_keeps_the_subscriptions() -> Result<(), String> {
    let b = broker(Limits {
        // the reconnect and resume fit in the grace under CI load; 300 ms
        // did not (T-5245)
        resume_grace: Duration::from_secs(2),
        ..Limits::default()
    })
    .await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let sub = text(&c.subscribe(cars()).await?, "/body/id");
    let (id, token) = (c.connection.clone(), c.token.clone());
    c.close().await;
    until(&b.hub, |h| h.attached() == 0).await?;
    let mut again = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(status(&again.resume(&id, &token, 0).await?), 200);
    // past the grace the resumed connection still holds its Subscription
    tokio::time::sleep(Duration::from_secs(3)).await;
    b.http.car(None, "urn:ngsi-ld:Car:g", 1).await?;
    let n = again.notification().await?;
    assert_eq!(text(&n, "/metadata/x-subscription"), sub, "{n}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_a_resume_takes_over_an_attached_connection() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let mut first = connect(b.addr, ROOT, &[]).await?;
    let sub = text(&first.subscribe(cars()).await?, "/body/id");
    let mut second = connect(b.addr, ROOT, &[]).await?;
    let r = second.resume(&first.connection, &first.token, 0).await?;
    assert_eq!(status(&r), 200, "{r}");
    // the first socket ends; its connection lives on in the second
    until(&b.hub, |h| h.connections() == 1 && h.attached() == 1).await?;
    b.http.car(None, "urn:ngsi-ld:Car:t", 1).await?;
    let n = second.notification().await?;
    assert_eq!(text(&n, "/metadata/x-subscription"), sub, "{n}");
    assert!(first.next().await.is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ws_a_resume_from_another_place_is_not_found() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    view(&b, &["create_subscription", "retrieve_subscription"]).await?;
    let city = [("NGSILD-Tenant", "city")];
    let mut held = connect(b.addr, ROOT, &city).await?;
    assert_eq!(status(&held.subscribe(cars()).await?), 201);
    let (id, token) = (held.connection.clone(), held.token.clone());
    // itself, another Tenant, the same Tenant through a view
    let mut own = connect(b.addr, ROOT, &city).await?;
    let (own_id, own_token) = (own.connection.clone(), own.token.clone());
    let mut town = connect(b.addr, ROOT, &[("NGSILD-Tenant", "town")]).await?;
    let mut viewed = connect(b.addr, "/x/views/cars/ws", &[]).await?;
    assert_eq!(status(&own.resume(&own_id, &own_token, 0).await?), 404);
    assert_eq!(status(&town.resume(&id, &token, 0).await?), 404);
    assert_eq!(status(&viewed.resume(&id, &token, 0).await?), 404);
    assert_eq!(b.hub.connections(), 4);
    Ok(())
}
