// SPDX-License-Identifier: EUPL-1.2
//! The ADR-0027 section 6 budget row "WebSocket notification": delivery
//! latency against the HTTP sink to a local receiver, and what an idle
//! connection keeps resident.
//!
//!     cargo bench -p antares-ws --bench fanout
//!
//! Latency: one Subscription per channel on one Car, `ROUNDS` sequential
//! changes, each timed from the write to the notification's arrival;
//! p50 / p99 of each channel, and whether the socket's p99 is within the
//! HTTP one. Resident: `IDLE` sockets opened with a bare handshake over raw
//! TCP (so the client side keeps almost nothing), the growth of this
//! process's `VmRSS` divided by `IDLE`, against 32 KiB.

use antares_ops::test_kit::TestBroker;
use antares_ws::{Hub, Limits, Sink, Ws};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::io::Write as _;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

const ROUNDS: usize = 2000;
const IDLE: usize = 2000;
const BUDGET_KIB: u64 = 32;

type Error = Box<dyn std::error::Error>;

/// One HTTP/1.1 exchange on its own connection; the status.
async fn http(addr: SocketAddr, method: &str, path: &str, body: &Value) -> Result<u16, Error> {
    let b = body.to_string();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
        b.len()
    );
    let mut s = TcpStream::connect(addr).await?;
    s.write_all(req.as_bytes()).await?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).await?;
    let text = String::from_utf8_lossy(&out);
    Ok(text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0))
}

/// A receiver that answers 204 to every POST and reports when each arrived.
async fn receiver() -> Result<(SocketAddr, tokio::sync::mpsc::UnboundedReceiver<Instant>), Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    let Ok(n) = s.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(buf.get(..n).unwrap_or_default());
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    let Some((head, body)) = text.split_once("\r\n\r\n") else {
                        continue;
                    };
                    let length = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().to_owned())
                        })
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(0);
                    if body.len() < length {
                        continue;
                    }
                    let _sent = tx.send(Instant::now());
                    raw.clear();
                    if s.write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n")
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    Ok((addr, rx))
}

fn percentile(sorted: &[Duration], p: usize) -> Duration {
    let i = sorted.len().saturating_mul(p).checked_div(100).unwrap_or(0);
    sorted
        .get(i.min(sorted.len().saturating_sub(1)))
        .copied()
        .unwrap_or_default()
}

fn vm_rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

/// Open a socket with a bare handshake and read its welcome; the stream is
/// kept, nothing else.
async fn idle(addr: SocketAddr) -> Result<TcpStream, Error> {
    let mut s = TcpStream::connect(addr).await?;
    let req =
        "GET /x/ws/ngsi-ld/v1 HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
               Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n";
    s.write_all(req.as_bytes()).await?;
    let mut buf = [0u8; 1024];
    let n = s.read(&mut buf).await?;
    if !buf
        .get(..n)
        .unwrap_or_default()
        .starts_with(b"HTTP/1.1 101")
    {
        return Err("the upgrade was refused".into());
    }
    Ok(s)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Error> {
    let limits = Limits {
        max_connections: IDLE.saturating_add(10),
        max_connections_per_tenant: IDLE.saturating_add(10),
        ..Limits::default()
    };
    let hub = Hub::new(limits);
    let broker = TestBroker::with_sinks(vec![Box::new(Sink::new(hub.clone()))], None).await?;
    let app = broker.serve_mounted(vec![antares_ops::mount(Ws::new(hub.clone()))])?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _served = axum::serve(listener, app).await;
    });

    for id in ["urn:ngsi-ld:Car:h", "urn:ngsi-ld:Car:w"] {
        let car = json!({"id": id, "type": "Car", "speed": {"type": "Property", "value": 0}});
        http(addr, "POST", "/ngsi-ld/v1/entities", &car).await?;
    }
    // one Car per channel, so a change notifies exactly one of them and
    // neither waits for the other's delivery
    let watch = |id: &str, endpoint: Value| {
        json!({"type": "Subscription", "entities": [{"type": "Car", "id": id}], "watchedAttributes": ["speed"],
               "notification": {"attributes": ["speed"], "endpoint": endpoint}})
    };
    let (rx_addr, mut arrivals) = receiver().await?;
    let endpoint = json!({"uri": format!("http://{rx_addr}/n"), "accept": "application/json"});
    let made = http(
        addr,
        "POST",
        "/ngsi-ld/v1/subscriptions",
        &watch("urn:ngsi-ld:Car:h", endpoint),
    )
    .await?;
    if made != 201 {
        return Err(format!("the HTTP Subscription: {made}").into());
    }
    let (mut ws, _) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/x/ws/ngsi-ld/v1")).await?;
    ws.next().await;
    let sub = json!({"metadata": {"x-op": "x-subscribe", "x-id": "s"}, "body": watch("urn:ngsi-ld:Car:w", json!({}))});
    ws.send(Message::Text(sub.to_string().into())).await?;
    ws.next().await;

    let (mut over_ws, mut over_http) = (Vec::new(), Vec::new());
    for v in 1..=ROUNDS {
        let body = json!({"speed": {"type": "Property", "value": v}});
        let start = Instant::now();
        http(
            addr,
            "PATCH",
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:h/attrs",
            &body,
        )
        .await?;
        let at = tokio::time::timeout(Duration::from_secs(10), arrivals.recv())
            .await?
            .ok_or("the receiver stopped")?;
        over_http.push(at.saturating_duration_since(start));

        let start = Instant::now();
        http(
            addr,
            "PATCH",
            "/ngsi-ld/v1/entities/urn:ngsi-ld:Car:w/attrs",
            &body,
        )
        .await?;
        match tokio::time::timeout(Duration::from_secs(10), ws.next()).await? {
            Some(Ok(Message::Text(_))) => over_ws.push(start.elapsed()),
            other => return Err(format!("the socket ended: {other:?}").into()),
        }
    }
    over_ws.sort();
    over_http.sort();
    let (ws99, http99) = (percentile(&over_ws, 99), percentile(&over_http, 99));
    let mut out = std::io::stdout();
    writeln!(
        out,
        "latency over {ROUNDS} changes: socket p50 {:?} p99 {ws99:?}; HTTP p50 {:?} p99 {http99:?}; socket p99 within 1.10× HTTP p99: {}",
        percentile(&over_ws, 50),
        percentile(&over_http, 50),
        // ADR-0027 section 6: the socket's one queue hop is allowed 10 %
        ws99.as_nanos().saturating_mul(10) <= http99.as_nanos().saturating_mul(11)
    )?;

    let before = vm_rss_kib();
    let mut held = Vec::with_capacity(IDLE);
    for _ in 0..IDLE {
        held.push(idle(addr).await?);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let per = vm_rss_kib()
        .saturating_sub(before)
        .checked_div(u64::try_from(IDLE)?)
        .unwrap_or(0);
    writeln!(
        out,
        "resident per idle connection: {per} KiB over {IDLE} sockets (budget {BUDGET_KIB} KiB): {}",
        if per <= BUDGET_KIB { "within" } else { "OVER" }
    )?;
    drop(held);
    Ok(())
}
