// SPDX-License-Identifier: EUPL-1.2
//! T-5847: the binary lays `antares_api::compression` over the API it
//! serves, so a client asking for gzip gets gzip and one asking for nothing
//! gets plain JSON. The layer's own behaviour is proven in
//! crates/antares-api/tests/response_compression_t5847.rs.

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

/// The response head, lower-cased, of a GET sent with `extra` headers.
fn head(port: u16, path: &str, extra: &str) -> String {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return String::new();
    };
    let req = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n{extra}\r\n");
    let _ = s.write_all(req.as_bytes());
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let end = out
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(out.len());
    String::from_utf8_lossy(out.get(..end).unwrap_or_default()).to_ascii_lowercase()
}

#[test]
fn the_served_api_answers_gzip_when_asked() {
    let port = free_port();
    let _broker = Broker(
        Command::new(env!("CARGO_BIN_EXE_antares"))
            .env("ANTARES_HTTP_PORT", port.to_string())
            .spawn()
            .expect("spawn antares"),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !head(port, "/q/health", "").starts_with("http/1.1 200") {
        assert!(Instant::now() < deadline, "broker never got healthy");
        std::thread::sleep(Duration::from_millis(200));
    }
    // A list streams with no known size, so it passes the 1 KiB minimum
    // (T-5849) even empty; a problem document is under it and stays plain.
    let path = "/ngsi-ld/v1/entities?type=Kpi";
    let packed = head(port, path, "Accept-Encoding: gzip\r\n");
    assert!(packed.starts_with("http/1.1 200"), "{packed}");
    assert!(packed.contains("\r\ncontent-encoding: gzip"), "{packed}");
    assert!(packed.contains("\r\nvary: accept-encoding"), "{packed}");
    let plain = head(port, path, "");
    assert!(plain.starts_with("http/1.1 200"), "{plain}");
    assert!(!plain.contains("content-encoding"), "{plain}");
}
