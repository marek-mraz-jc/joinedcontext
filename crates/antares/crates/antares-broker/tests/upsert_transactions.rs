// SPDX-License-Identifier: EUPL-1.2
//! T-4085 against the real binary on Postgres: how many transactions a Batch
//! Upsert through a view opens, read off `antares_pg_transaction_begin_seconds`
//! before and after. A Vehicle with two Relationships, as the HSL stream
//! writes one, used to cost five (pre-image, one read per target, the write,
//! the history); the pre-image and every target now share one read, so it is
//! three however many Relationships or members the batch carries.
//!
//! Skips loudly without ANTARES_TEST_DATABASE_URL (same rule as
//! store_combos.rs).
#![cfg(all(unix, feature = "views", feature = "jc-policy"))]

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

const TENANT: &str = "t4085";
const VIEW: &str = "t4085-hfp";

fn http(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> String {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return String::new();
    };
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    match body {
        Some(b) => req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        )),
        None => req.push_str("Content-Length: 0\r\n\r\n"),
    }
    let _ = s.write_all(req.as_bytes());
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

/// Every transaction the broker has begun, summed over the histogram's series.
fn begun(port: u16) -> u64 {
    let raw = http(port, "GET", "/q/metrics", &[], None);
    let total: f64 = raw
        .lines()
        .filter(|l| l.starts_with("antares_pg_transaction_begin_seconds_count"))
        .filter_map(|l| l.rsplit(' ').next()?.parse::<f64>().ok())
        .sum();
    assert!(total > 0.0, "no transaction metric: {raw}");
    total as u64
}

fn upsert(port: u16, members: &str) -> String {
    http(
        port,
        "POST",
        &format!("/x/views/{VIEW}/ngsi-ld/v1/entityOperations/upsert"),
        &[("NGSILD-Tenant", TENANT), ("x-jc-client", "runner")],
        Some(&format!("[{members}]")),
    )
}

fn vehicle(n: u32, speed: u32) -> String {
    format!(
        r#"{{"id":"urn:ngsi-ld:Vehicle:{n}","type":"Vehicle",
            "speed":{{"type":"Property","value":{speed},"observedAt":"2026-10-12T10:{speed:02}:00Z"}},
            "refTrip":{{"type":"Relationship","object":"urn:ngsi-ld:Trip:1"}},
            "refRoute":{{"type":"Relationship","object":"urn:ngsi-ld:Route:1"}}}}"#
    )
}

#[test]
fn an_upsert_through_a_view_opens_three_transactions_however_many_targets_and_members() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL unset");
        return;
    };
    let port = free_port();
    let _broker = Broker(
        Command::new(env!("CARGO_BIN_EXE_antares"))
            .env("ANTARES_HTTP_PORT", port.to_string())
            .env("ANTARES_STORE", "postgres")
            .env("ANTARES_TEMPORAL", "postgres")
            .env("ANTARES_DATABASE_URL", url)
            .env("ANTARES_ALLOW_SHARED_LOCAL", "1")
            .env("ANTARES_TELEMETRY", "1")
            .env("ANTARES_API_SURFACES", "admin,views")
            .env("ANTARES_POLICY", "joinedcontext")
            .env("ANTARES_POLICY_SUBJECT_HEADERS", "x-jc-client")
            // No reload tick inside the measured window.
            .env("ANTARES_VIEWS_RELOAD_MS", "3600000")
            .spawn()
            .expect("spawn antares"),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !http(port, "GET", "/q/health", &[], None).starts_with("HTTP/1.1 200") {
        assert!(Instant::now() < deadline, "broker never got healthy");
        std::thread::sleep(Duration::from_millis(200));
    }
    let view = format!(
        r#"{{"id":"{VIEW}","tenant":"{TENANT}","selector":["Vehicle","Trip","Route"],
            "rules":[{{"types":["Vehicle","Trip","Route"],"attributes":["speed","refTrip","refRoute"]}}],
            "operations":["batch_upsert"]}}"#
    );
    let put = http(port, "PUT", &format!("/q/views/{VIEW}"), &[], Some(&view));
    assert!(put.starts_with("HTTP/1.1 20"), "{put}");
    let rules = format!(
        r#"{{"policies":[{{"id":"{TENANT}/runner","effect":"permit",
            "assignee":{{"kind":"serviceAccount","id":"runner"}},"operations":["5.6.8"]}}]}}"#
    );
    let put = http(
        port,
        "PUT",
        &format!("/q/policy/{TENANT}"),
        &[],
        Some(&rules),
    );
    assert!(put.starts_with("HTTP/1.1 20"), "{put}");

    // The targets, then every Vehicle once, so each upsert below replaces.
    let targets =
        r#"{"id":"urn:ngsi-ld:Trip:1","type":"Trip"},{"id":"urn:ngsi-ld:Route:1","type":"Route"}"#;
    let made = upsert(port, targets);
    assert!(made.starts_with("HTTP/1.1 20"), "{made}");
    let all = |speed| {
        (1..=5)
            .map(|n| vehicle(n, speed))
            .collect::<Vec<_>>()
            .join(",")
    };
    let made = upsert(port, &all(1));
    assert!(made.starts_with("HTTP/1.1 20"), "{made}");

    // Idle, the count stands still: the start's own jobs (migrations, the
    // first view and policy load, the sweeps' first pass) are behind us.
    let mut idle = begun(port);
    let settle = Instant::now() + Duration::from_secs(30);
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let now = begun(port);
        if now == idle {
            break;
        }
        assert!(Instant::now() < settle, "the broker never went idle");
        idle = now;
    }
    let before = begun(port);
    let one = upsert(port, &vehicle(1, 2));
    assert!(one.starts_with("HTTP/1.1 204"), "{one}");
    let after = begun(port);
    assert!(
        after - before <= 3,
        "one Vehicle with two Relationships opened {} transactions",
        after - before
    );

    let before = begun(port);
    let five = upsert(port, &all(3));
    assert!(five.starts_with("HTTP/1.1 204"), "{five}");
    let after = begun(port);
    assert!(
        after - before <= 3,
        "five Vehicles opened {} transactions: the history is one, the reads one",
        after - before
    );

    // A target the view does not show still refuses the member (T-3786).
    let absent = vehicle(1, 4).replace("urn:ngsi-ld:Trip:1", "urn:ngsi-ld:Trip:absent");
    let refused = upsert(port, &absent);
    assert!(refused.starts_with("HTTP/1.1 207"), "{refused}");
}
