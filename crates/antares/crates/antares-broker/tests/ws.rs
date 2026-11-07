// SPDX-License-Identifier: EUPL-1.2
//! The WebSocket surface in the real binary (ADR-0026), built only with the
//! `ws` feature: one process serves a socket end to end over its own accept
//! loop, and in the bus=nats cell the api pod holds the socket while the
//! worker pod matches and notifies, the notification crossing over core
//! NATS request/reply (rule 5; env-gated on BOTH ANTARES_TEST_DATABASE_URL
//! and ANTARES_TEST_NATS_URL).
//!
//! The client is the few lines of RFC 6455 these tests need (a handshake,
//! masked text frames out, unmasked frames in), so the broker gains no
//! dependency, development ones included: its tree is the vanilla gate's.
#![cfg(feature = "ws")]

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod common;
use common::free_port;
#[path = "common/ws_client.rs"]
mod ws_client;
use ws_client::Socket;

struct Broker(Child);

impl Drop for Broker {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _waited = self.0.wait();
    }
}

fn start(port: u16, env: &[(&str, &str)]) -> Result<Broker, String> {
    Command::new(env!("CARGO_BIN_EXE_antares"))
        .env("ANTARES_HTTP_PORT", port.to_string())
        .env("ANTARES_API_SURFACES", "admin,ws")
        .envs(env.iter().copied())
        .spawn()
        .map(Broker)
        .map_err(|e| e.to_string())
}

/// The environment of a pod of the split fleet.
fn split<'a>(roles: &'a str, db: &'a str, nats: &'a str) -> [(&'a str, &'a str); 5] {
    [
        ("ANTARES_STORE", "postgres"),
        ("ANTARES_DATABASE_URL", db),
        ("ANTARES_BUS", "nats"),
        ("ANTARES_NATS_URL", nats),
        ("ANTARES_ROLES", roles),
    ]
}

/// One HTTP/1.1 exchange; the raw answer, empty when nothing listens.
fn http(port: u16, method: &str, path: &str, tenant: &str, body: Option<&str>) -> String {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return String::new();
    };
    let b = body.unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nNGSILD-Tenant: {tenant}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
        b.len()
    );
    let mut out = String::new();
    if s.write_all(req.as_bytes()).is_ok() {
        let _read = s.read_to_string(&mut out);
    }
    out
}

fn healthy(port: u16) -> Result<(), String> {
    for _ in 0..150 {
        if http(port, "GET", "/q/health", "default", None).starts_with("HTTP/1.1 200") {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!("the broker on :{port} never got healthy"))
}

fn run_id() -> Result<u128, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis())
}

/// One process, the default store and bus: the accept loop hands the
/// upgraded stream to the surface, and a Subscription made over the socket
/// is notified over it. A bound set in the environment is a known config
/// key (unknown keys are fatal).
#[test]
fn ws_the_broker_binary_serves_a_socket_end_to_end() -> Result<(), String> {
    let port = free_port();
    let _broker = start(port, &[("ANTARES_WS_QUEUE", "64")])?;
    healthy(port)?;
    let run = run_id()?;
    notified(port, &format!("ws{run}"), run)
}

/// A bound that is not a positive whole number stops the broker at startup
/// instead of falling back to a default the operator did not choose.
#[test]
fn ws_an_invalid_bound_stops_the_broker() -> Result<(), String> {
    let mut broker = start(free_port(), &[("ANTARES_WS_MAX_SUBSCRIPTIONS", "0")])?;
    for _ in 0..150 {
        if let Some(status) = broker.0.try_wait().map_err(|e| e.to_string())? {
            assert!(!status.success(), "{status}");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("the broker kept running on ANTARES_WS_MAX_SUBSCRIPTIONS=0".into())
}

#[test]
fn ws_cross_pod_delivery_reaches_the_holding_pod() -> Result<(), String> {
    let (Ok(db), Ok(nats)) = (
        std::env::var("ANTARES_TEST_DATABASE_URL"),
        std::env::var("ANTARES_TEST_NATS_URL"),
    ) else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL / ANTARES_TEST_NATS_URL not set");
        return Ok(());
    };
    let run = run_id()?;
    let (api_port, worker_port) = (free_port(), free_port());
    let _api = start(api_port, &split("api", &db, &nats))?;
    let _worker = start(worker_port, &split("matcher,notifier", &db, &nats))?;
    healthy(api_port)?;
    healthy(worker_port)?;
    notified(api_port, &format!("ws{run}"), run)
}

/// Open a socket on `port` in `tenant`, subscribe to `WsPod`, and write a
/// new `WsPod` each round until one is notified over the socket (30 s): a
/// worker pod learns the Subscription through the KV mirror, and until it
/// does a change matches nothing.
fn notified(port: u16, tenant: &str, run: u128) -> Result<(), String> {
    let mut ws = Socket::open(
        port,
        "/x/ws/ngsi-ld/v1",
        &[("NGSILD-Tenant".to_owned(), tenant.to_owned())],
        Duration::from_secs(2),
    )?;
    let welcome = ws.next().ok_or("no welcome")?;
    assert_eq!(welcome.pointer("/metadata/x-op"), Some(&json!("x-welcome")));
    ws.send(&json!({"metadata": {"x-op": "x-subscribe", "x-id": "s"},
                    "body": {"type": "Subscription", "entities": [{"type": "WsPod"}]}}))?;
    let made = ws.next().ok_or("no x-result")?;
    assert_eq!(
        made.pointer("/metadata/x-status"),
        Some(&json!(201)),
        "{made}"
    );
    let sub_id = made
        .pointer("/body/id")
        .and_then(Value::as_str)
        .ok_or(format!("no id in {made}"))?
        .to_owned();
    for round in 0..15 {
        let id = format!("urn:ngsi-ld:WsPod:{run}-{round}");
        let body = json!({"id": id, "type": "WsPod", "n": {"type": "Property", "value": round}});
        let made = http(
            port,
            "POST",
            "/ngsi-ld/v1/entities",
            tenant,
            Some(&body.to_string()),
        );
        assert!(made.starts_with("HTTP/1.1 201"), "{made}");
        while let Some(m) = ws.next() {
            if m.pointer("/metadata/x-op") == Some(&json!("x-notification")) {
                assert_eq!(
                    m.pointer("/metadata/x-subscription"),
                    Some(&json!(sub_id)),
                    "{m}"
                );
                assert_eq!(m.pointer("/metadata/x-seq"), Some(&json!(1)), "{m}");
                assert!(
                    m.pointer("/body/data/0/id")
                        .and_then(Value::as_str)
                        .is_some_and(|i| i.starts_with("urn:ngsi-ld:WsPod:")),
                    "{m}"
                );
                return Ok(());
            }
        }
    }
    Err("no notification reached the socket".into())
}
