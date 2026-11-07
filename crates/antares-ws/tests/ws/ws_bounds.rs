// SPDX-License-Identifier: EUPL-1.2
//! Rules 6 and 9: what a socket may be opened from, how many there are, and
//! what one client message may weigh.

use crate::common::{broker, cars, connect, open, status, text, until, Client};
use antares_ws::Limits;
use futures_util::SinkExt;
use serde_json::json;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};
use tokio_tungstenite::tungstenite::protocol::frame::Frame;
use tokio_tungstenite::tungstenite::Message;

const ROOT: &str = "/x/ws/ngsi-ld/v1";

/// An Origin outside `allowed_origins` is refused with 403 before the
/// upgrade; a listed one and a client without an Origin are admitted.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_foreign_origin_is_refused() -> Result<(), String> {
    let b = broker(Limits {
        allowed_origins: vec!["https://portal.example".into()],
        ..Limits::default()
    })
    .await?;
    for foreign in [
        "https://evil.example",
        "https://portal.example.evil",
        "null",
    ] {
        let refused = open(b.addr, ROOT, &[("Origin", foreign)])
            .await?
            .err()
            .ok_or(format!("{foreign} was admitted"))?;
        assert_eq!(refused.status(), 403, "{foreign}");
    }
    let _listed = connect(b.addr, ROOT, &[("Origin", "https://portal.example")]).await?;
    let _none = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(b.hub.connections(), 2);
    Ok(())
}

/// The upgrade answers 503 past `max_connections_per_tenant` in one tenant
/// and past `max_connections` in the process; a closed socket frees its
/// place.
#[tokio::test(flavor = "multi_thread")]
async fn ws_connections_are_capped_per_tenant_and_per_process() -> Result<(), String> {
    let b = broker(Limits {
        max_connections: 3,
        max_connections_per_tenant: 2,
        ..Limits::default()
    })
    .await?;
    let refused = |t: &'static str| async move {
        open(b.addr, ROOT, &[("NGSILD-Tenant", t)])
            .await
            .map(|r| r.err().map(|r| r.status().as_u16()))
    };
    let a1 = connect(b.addr, ROOT, &[("NGSILD-Tenant", "a")]).await?;
    let _a2 = connect(b.addr, ROOT, &[("NGSILD-Tenant", "a")]).await?;
    assert_eq!(refused("a").await?, Some(503), "the third in tenant a");
    let _b1 = connect(b.addr, ROOT, &[("NGSILD-Tenant", "b")]).await?;
    assert_eq!(refused("b").await?, Some(503), "the fourth in the process");
    a1.close().await;
    until(&b.hub, |h| h.connections() <= 2).await?;
    connect(b.addr, ROOT, &[("NGSILD-Tenant", "a")]).await?;
    Ok(())
}

/// The socket ends within [`WAIT`], with a close frame or without one.
async fn ends(c: &mut Client) -> Result<Option<u16>, String> {
    match c.closed().await {
        Ok((code, _)) => Ok(Some(code)),
        Err(e) if e.starts_with("not closed") => Err(e),
        Err(_) => Ok(None),
    }
}

/// A text message of `bytes` bytes sent in fragments of `frame` bytes.
async fn fragmented(c: &mut Client, bytes: usize, frame: usize) -> Result<(), String> {
    let text = "x".repeat(bytes).into_bytes();
    let mut parts = text.chunks(frame).peekable();
    let mut first = true;
    while let Some(part) = parts.next() {
        let opcode = if first {
            OpCode::Data(Data::Text)
        } else {
            OpCode::Data(Data::Continue)
        };
        first = false;
        let f = Frame::message(part.to_vec(), opcode, parts.peek().is_none());
        c.ws_mut()
            .send(Message::Frame(f))
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// One message past 64 KiB (in fragments), one frame past 16 KiB, and a
/// message nested past depth 64 each end the connection; a deep message
/// closes with 1009, and a message just under the caps is answered.
#[tokio::test(flavor = "multi_thread")]
async fn ws_an_oversized_or_too_deep_client_message_closes_the_connection() -> Result<(), String> {
    let b = broker(Limits::default()).await?;

    let mut c = connect(b.addr, ROOT, &[]).await?;
    fragmented(&mut c, 64 * 1024 - 1, 8 * 1024).await?;
    let answer = c.result().await?;
    assert_eq!(status(&answer), 400, "under the caps: {answer}");

    let mut c = connect(b.addr, ROOT, &[]).await?;
    fragmented(&mut c, 64 * 1024 + 1, 8 * 1024).await?;
    assert_eq!(ends(&mut c).await?, Some(1009), "a message past 64 KiB");

    let mut c = connect(b.addr, ROOT, &[]).await?;
    fragmented(&mut c, 16 * 1024 + 1, 32 * 1024).await?;
    assert_eq!(ends(&mut c).await?, Some(1009), "a frame past 16 KiB");

    let mut c = connect(b.addr, ROOT, &[]).await?;
    let deep = format!("{}{}", "[".repeat(65), "]".repeat(65));
    c.send(
        json!({"x-op": "x-subscribe", "x-id": "d"}),
        Some(serde_json::from_str(&deep).map_err(|e| e.to_string())?),
    )
    .await?;
    assert_eq!(ends(&mut c).await?, Some(1009));

    until(&b.hub, |h| h.connections() <= 1).await?;
    Ok(())
}

/// W9: a client that sends one frame past its burst gets `x-closing`
/// `rate` and the close code 1008; every frame within the burst is
/// answered, and another client of the tenant is not touched.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_client_past_its_message_rate_is_closed_with_1008() -> Result<(), String> {
    let b = broker(Limits {
        messages_per_sec: 1,
        message_burst: 3,
        ..Limits::default()
    })
    .await?;
    let mut other = connect(b.addr, ROOT, &[]).await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    for _ in 0..4 {
        c.send_text("{}".to_owned()).await?;
    }
    let (code, seen) = c.closed().await?;
    assert_eq!(code, 1008, "{seen:?}");
    let answered = seen.iter().filter(|m| status(m) == 400).count();
    assert_eq!(answered, 3, "the burst is answered: {seen:?}");
    assert!(
        seen.iter().any(|m| text(m, "/metadata/x-reason") == "rate"),
        "{seen:?}"
    );
    assert_eq!(status(&other.subscribe(cars()).await?), 201);
    Ok(())
}

/// Past `max_subscriptions` on one connection `x-subscribe` answers 400 and
/// creates nothing; deleting one makes room again.
#[tokio::test(flavor = "multi_thread")]
async fn ws_subscriptions_per_connection_are_capped() -> Result<(), String> {
    let b = broker(Limits {
        max_subscriptions: 2,
        ..Limits::default()
    })
    .await?;
    let mut c = connect(b.addr, ROOT, &[]).await?;
    let first = c.subscribe(cars()).await?;
    assert_eq!(status(&first), 201, "{first}");
    assert_eq!(status(&c.subscribe(cars()).await?), 201);
    let third = c.subscribe(cars()).await?;
    assert_eq!(status(&third), 400, "{third}");
    let (code, listed) = b
        .http
        .call("GET", "/ngsi-ld/v1/subscriptions", &[], None)
        .await?;
    assert_eq!(code, 200);
    assert_eq!(listed.as_array().map(Vec::len), Some(2), "{listed}");
    let id = crate::common::text(&first, "/body/id");
    c.send(
        json!({"x-op": "x-unsubscribe", "x-id": "u", "x-subscription": id}),
        None,
    )
    .await?;
    assert_eq!(status(&c.result().await?), 204);
    assert_eq!(status(&c.subscribe(cars()).await?), 201);
    Ok(())
}

/// RFC 6455 section 4.2.1, which the surface checks itself since it no
/// longer uses axum's `ws` feature (T-3704): a GET that is no upgrade, a
/// version other than 13 and a missing key are refused with a problem
/// document and admit nothing.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_request_that_is_no_websocket_handshake_is_refused() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let upgrade = [("Connection", "Upgrade"), ("Upgrade", "websocket")];
    let key = ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==");
    for (headers, said) in [
        (vec![], "Upgrade: websocket"),
        (vec![upgrade[1]], "Upgrade: websocket"),
        (vec![upgrade[0]], "Upgrade: websocket"),
        (
            vec![upgrade[0], upgrade[1], ("Sec-WebSocket-Version", "8"), key],
            "Version is 13",
        ),
        (
            vec![upgrade[0], upgrade[1], ("Sec-WebSocket-Version", "13")],
            "Sec-WebSocket-Key",
        ),
    ] {
        let (code, body) = b.http.call("GET", ROOT, &headers, None).await?;
        assert_eq!(code, 400, "{headers:?}: {body}");
        assert_eq!(
            text(&body, "/type"),
            "https://uri.etsi.org/ngsi-ld/errors/BadRequestData",
            "{headers:?}"
        );
        assert!(text(&body, "/detail").contains(said), "{headers:?}: {body}");
    }
    assert_eq!(b.hub.connections(), 0);
    let _upgraded = connect(b.addr, ROOT, &[]).await?;
    assert_eq!(b.hub.connections(), 1);
    Ok(())
}

/// F-ANT3-005: a client naming a reserved or malformed Tenant is refused
/// before the hub admits a connection for it.
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_reserved_or_invalid_tenant_is_refused() -> Result<(), String> {
    let b = broker(Limits::default()).await?;
    let handshake = [
        ("Connection", "Upgrade"),
        ("Upgrade", "websocket"),
        ("Sec-WebSocket-Version", "13"),
        ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
    ];
    for tenant in ["snap-x", "a b"] {
        let mut headers = handshake.to_vec();
        headers.push(("NGSILD-Tenant", tenant));
        let (code, body) = b.http.call("GET", ROOT, &headers, None).await?;
        assert_eq!(code, 400, "{tenant}: {body}");
        assert!(
            text(&body, "/detail").contains("6.3.14"),
            "{tenant}: {body}"
        );
    }
    assert_eq!(b.hub.connections(), 0);
    Ok(())
}

/// RFC 6455 section 4.2.2 needs the connection itself: a request served
/// in process carries no `OnUpgrade`, so a well-formed handshake there is
/// refused with 426 and admits nothing (T-3704, T-3707).
#[tokio::test(flavor = "multi_thread")]
async fn ws_a_handshake_on_a_connection_that_cannot_upgrade_is_426() -> Result<(), String> {
    use tower::ServiceExt;
    let hub = antares_ws::Hub::new(Limits::default());
    let kit = antares_ops::test_kit::TestBroker::new().await?;
    let app = kit.serve_mounted(vec![antares_ops::mount(antares_ws::Ws::new(hub.clone()))])?;
    let req = axum::http::Request::get(ROOT)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(axum::body::Body::empty())
        .map_err(|e| e.to_string())?;
    let resp = app.oneshot(req).await.map_err(|e| e.to_string())?;
    assert_eq!(resp.status(), 426);
    assert_eq!(hub.connections(), 0);
    Ok(())
}
