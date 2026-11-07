// SPDX-License-Identifier: EUPL-1.2
//! T-5895: a default broker stays inside its 200 MiB memory budget under the
//! load that held 394 MB of live heap before: 5 000 Entities, then reads
//! with distinct inline @contexts of 2 000 terms. Each distinct @context used
//! to become an ImplicitlyCreated row in the memory store (~390 KB of heap
//! each, 1 000 per Tenant); the byte share bounds them now.
//!
//! ~66 s of load: it runs in the all-features lanes only, not once per
//! feature in the each-feature job (30 min cap, train 20261028-0937). The
//! budget does not depend on the surfaces built in.
#![cfg(all(feature = "full", feature = "jc-policy"))]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

mod common;
use common::free_port;

struct Broker(Child);

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn http(port: u16, method: &str, path: &str, ctype: &str, body: &str) -> String {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return String::new();
    };
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nAccept: application/json\r\n\
         Content-Type: {ctype}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = s.write_all(req.as_bytes());
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

fn allocated(port: u16) -> u64 {
    let raw = http(port, "GET", "/q/health", "application/json", "");
    let body = raw.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    let health: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    health["memory"]["allocatedBytes"]
        .as_u64()
        .unwrap_or(u64::MAX)
}

#[test]
fn distinct_large_inline_contexts_stay_inside_the_memory_budget() {
    let port = free_port();
    let broker = Broker(
        Command::new(env!("CARGO_BIN_EXE_antares"))
            .env("ANTARES_HTTP_PORT", port.to_string())
            .env_remove("ANTARES_MEMORY_BUDGET")
            .env_remove("ANTARES_STORE")
            .spawn()
            .expect("spawn antares"),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !http(port, "GET", "/q/health", "application/json", "").starts_with("HTTP/1.1 200") {
        assert!(Instant::now() < deadline, "broker never got healthy");
        std::thread::sleep(Duration::from_millis(200));
    }
    for batch in 0..50 {
        let entities: Vec<serde_json::Value> = (0..100)
            .map(|j| {
                serde_json::json!({"id": format!("urn:ngsi-ld:T:{}", batch * 100 + j),
                    "type": "T", "speed": {"type": "Property", "value": j}})
            })
            .collect();
        let body = serde_json::to_string(&entities).expect("json");
        let r = http(
            port,
            "POST",
            "/ngsi-ld/v1/entityOperations/upsert",
            "application/json",
            &body,
        );
        assert!(
            r.starts_with("HTTP/1.1 201") || r.starts_with("HTTP/1.1 204"),
            "{r:.200}"
        );
    }
    // 1 000 such @contexts held ~390 MB, so 600 held ~235 MB beside the
    // Entities before the byte share (red without it)
    for k in 0..600 {
        let terms: serde_json::Map<String, serde_json::Value> = (0..2_000)
            .map(|t| {
                (
                    format!("t{k}_{t}"),
                    format!("https://example.org/c{k}/t{t}").into(),
                )
            })
            .collect();
        let query = serde_json::json!({"type": "Query", "entities": [{"type": "T"}],
            "@context": [terms]});
        let r = http(
            port,
            "POST",
            "/ngsi-ld/v1/entityOperations/query",
            "application/ld+json",
            &query.to_string(),
        );
        assert!(r.starts_with("HTTP/1.1 200"), "query {k}: {r:.300}");
    }
    let held = allocated(port);
    assert!(
        held < antares_api::bounds::MemoryBudget::DEFAULT,
        "{held} bytes allocated after the load, budget {}",
        antares_api::bounds::MemoryBudget::DEFAULT
    );
    drop(broker);
}
