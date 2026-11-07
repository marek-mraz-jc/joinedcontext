// SPDX-License-Identifier: EUPL-1.2
//! T-3628 against the real binary: a `views` build serves
//! `/q/schemas/{tenant}` only with `ANTARES_SCHEMA_MAX_BYTES` named and lists
//! each Tenant's schema in `/q/health`; a vanilla build has no such route.

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

fn http(port: u16, method: &str, path: &str, body: Option<&str>) -> String {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return String::new();
    };
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
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

fn start(max: Option<&str>) -> (Broker, u16) {
    let port = free_port();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_antares"));
    cmd.env("ANTARES_HTTP_PORT", port.to_string())
        .env_remove("ANTARES_SCHEMA_MAX_BYTES");
    if let Some(max) = max {
        cmd.env("ANTARES_SCHEMA_MAX_BYTES", max);
    }
    let broker = Broker(cmd.spawn().expect("spawn antares"));
    let deadline = Instant::now() + Duration::from_secs(30);
    while !http(port, "GET", "/q/health", None).starts_with("HTTP/1.1 200") {
        assert!(Instant::now() < deadline, "broker never got healthy");
        std::thread::sleep(Duration::from_millis(200));
    }
    (broker, port)
}

#[cfg(feature = "views")]
fn health(port: u16) -> serde_json::Value {
    let raw = http(port, "GET", "/q/health", None);
    let body = raw.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    serde_json::from_str(body).unwrap_or(serde_json::Value::Null)
}

#[cfg(feature = "views")]
#[test]
fn a_views_build_takes_a_schema_within_the_named_bound() {
    let (_broker, port) = start(Some("64"));
    assert_eq!(health(port)["schemas"], serde_json::json!({}));
    let put = http(port, "PUT", "/q/schemas/city", Some(r#"{"name": "city"}"#));
    assert!(put.starts_with("HTTP/1.1 201"), "{put}");
    let big = format!("\"{}\"", "a".repeat(63));
    let put = http(port, "PUT", "/q/schemas/city", Some(&big));
    assert!(put.starts_with("HTTP/1.1 413"), "{put}");
    assert_eq!(health(port)["schemas"]["city"]["bytes"], 16);
    let get = http(port, "GET", "/q/schemas/city", None);
    assert!(get.ends_with(r#"{"name": "city"}"#), "{get}");
}

#[cfg(feature = "views")]
#[test]
fn without_the_bound_the_broker_takes_no_schema() {
    let (_broker, port) = start(None);
    let put = http(port, "PUT", "/q/schemas/city", Some("{}"));
    assert!(put.starts_with("HTTP/1.1 503"), "{put}");
    assert!(put.contains("ANTARES_SCHEMA_MAX_BYTES is not set"), "{put}");
    let get = http(port, "GET", "/q/schemas/city", None);
    assert!(get.starts_with("HTTP/1.1 404"), "nothing stored: {get}");
}

#[test]
fn a_bound_that_is_no_positive_byte_count_stops_the_start() {
    for bad in ["0", "-1", "64k"] {
        let port = free_port();
        let out = Command::new(env!("CARGO_BIN_EXE_antares"))
            .env("ANTARES_HTTP_PORT", port.to_string())
            .env("ANTARES_SCHEMA_MAX_BYTES", bad)
            .output()
            .expect("run antares");
        assert!(!out.status.success(), "{bad} started");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("ANTARES_SCHEMA_MAX_BYTES"), "{bad}: {err}");
    }
}

#[cfg(not(feature = "views"))]
#[test]
fn a_vanilla_build_has_no_schema_route() {
    let (_broker, port) = start(None);
    let put = http(port, "PUT", "/q/schemas/city", Some("{}"));
    assert!(put.starts_with("HTTP/1.1 404"), "{put}");
}
