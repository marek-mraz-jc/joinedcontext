// SPDX-License-Identifier: EUPL-1.2
//! Rule M4 (surfaces-threat-model 9.6, G6) against the real binary: the view
//! root, selected with `ANTARES_API_SURFACES=admin,views`, is closed (every
//! route 404, `closed` in `/q/health`) until a view loads, opens with it and
//! closes again when it is removed. The mechanism is tested through `mount`
//! in `crates/antares-ops/tests/surfaces_fail_closed.rs`.
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

fn http(port: u16, method: &str, path: &str, tenant: Option<&str>, body: Option<&str>) -> String {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return String::new();
    };
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    if let Some(t) = tenant {
        req.push_str(&format!("NGSILD-Tenant: {t}\r\n"));
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

fn json(raw: &str) -> serde_json::Value {
    let body = raw.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    serde_json::from_str(body).unwrap_or(serde_json::Value::Null)
}

const ROUTES: [&str; 3] = [
    "/x/views/cars/ngsi-ld/v1/entities?type=Car",
    "/x/views/cars/ngsi-ld/v1/types",
    "/x/views/nowhere/ngsi-ld/v1/entities?type=Car",
];

fn assert_closed(port: u16) {
    for route in ROUTES {
        let got = http(port, "GET", route, None, None);
        assert!(got.starts_with("HTTP/1.1 404"), "{route}: {got}");
        assert_eq!(json(&got)["title"], "ResourceNotFound", "{route}: {got}");
    }
    let h = json(&http(port, "GET", "/q/health", None, None));
    assert_eq!(h["surfaces"]["views"]["state"], "closed", "{h}");
    assert_eq!(h["surfaces"]["views"]["reason"], "no view is loaded", "{h}");
}

#[test]
fn a_selected_surface_without_a_view_is_closed() {
    let port = free_port();
    let _broker = Broker(
        Command::new(env!("CARGO_BIN_EXE_antares"))
            .env("ANTARES_HTTP_PORT", port.to_string())
            .env("ANTARES_API_SURFACES", "admin,views")
            .spawn()
            .expect("spawn antares"),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !http(port, "GET", "/q/health", None, None).starts_with("HTTP/1.1 200") {
        assert!(Instant::now() < deadline, "broker never got healthy");
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_closed(port);

    let car = r#"{"id":"urn:ngsi-ld:Car:1","type":"Car","speed":{"type":"Property","value":7}}"#;
    let made = http(
        port,
        "POST",
        "/ngsi-ld/v1/entities",
        Some("city"),
        Some(car),
    );
    assert!(made.starts_with("HTTP/1.1 201"), "{made}");
    let view = r#"{"id":"cars","tenant":"city","selector":["Car"],
        "rules":[{"types":["Car"],"attributes":["speed"]}],
        "operations":["query_entities","retrieve_entity_types"]}"#;
    let put = http(port, "PUT", "/q/views/cars", None, Some(view));
    assert!(put.starts_with("HTTP/1.1 201"), "{put}");

    let got = http(port, "GET", ROUTES[0], None, None);
    assert!(got.starts_with("HTTP/1.1 200"), "{got}");
    // a list answer is chunked; the Entity is in it
    assert!(got.contains(r#""id":"urn:ngsi-ld:Car:1""#), "{got}");
    let h = json(&http(port, "GET", "/q/health", None, None));
    assert_eq!(h["surfaces"]["views"]["state"], "open", "{h}");

    let del = http(port, "DELETE", "/q/views/cars", None, None);
    assert!(del.starts_with("HTTP/1.1 204"), "{del}");
    assert_closed(port);
}
