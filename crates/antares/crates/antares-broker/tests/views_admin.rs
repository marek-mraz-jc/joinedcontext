// SPDX-License-Identifier: EUPL-1.2
//! ADR-0023 rule 10 against the real binary: a `views` build serves
//! `/q/views`, validates before it stores, lists every view in `/q/health`,
//! and accepts the reload interval only because it carries views.
#![cfg(feature = "views")]

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

fn health(port: u16) -> serde_json::Value {
    let raw = http(port, "GET", "/q/health", None);
    let body = raw.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    serde_json::from_str(body).unwrap_or(serde_json::Value::Null)
}

#[test]
fn a_views_build_serves_q_views_and_reports_each_view() {
    let port = free_port();
    let _broker = Broker(
        Command::new(env!("CARGO_BIN_EXE_antares"))
            .env("ANTARES_HTTP_PORT", port.to_string())
            .env("ANTARES_VIEWS_RELOAD_MS", "250")
            .spawn()
            .expect("spawn antares"),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !http(port, "GET", "/q/health", None).starts_with("HTTP/1.1 200") {
        assert!(Instant::now() < deadline, "broker never got healthy");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(health(port)["views"], serde_json::json!({}));

    let good = r#"{"id":"cars","tenant":"city","selector":["Car"],
        "rules":[{"types":["Car"],"attributes":["speed"]}],
        "operations":["query_entities"]}"#;
    let put = http(port, "PUT", "/q/views/cars", Some(good));
    assert!(put.starts_with("HTTP/1.1 201"), "{put}");
    let bad = good.replace("query_entities", "drop_tables");
    let put = http(port, "PUT", "/q/views/bad", Some(&bad));
    assert!(put.starts_with("HTTP/1.1 400"), "{put}");

    let h = health(port);
    assert_eq!(h["views"]["cars"]["state"], "loaded", "{h}");
    assert!(
        h["views"].get("bad").is_none(),
        "a refused view is not stored: {h}"
    );
    let del = http(port, "DELETE", "/q/views/cars", None);
    assert!(del.starts_with("HTTP/1.1 204"), "{del}");
    assert_eq!(health(port)["views"], serde_json::json!({}));
}
