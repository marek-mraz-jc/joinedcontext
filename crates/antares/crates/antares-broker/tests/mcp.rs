// SPDX-License-Identifier: EUPL-1.2
//! The MCP surface's sink in the real binary (MCP section 5): built with
//! `mcp`, which brings `views`, the broker registers `x-antares-mcp:` before
//! the view table shares its state, so the scheme is known and refused by
//! its own rule (MC17), not dropped as an unregistered binding.
#![cfg(feature = "mcp")]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command};
use std::time::Duration;

mod common;
use common::free_port;

struct Broker(Child);

impl Drop for Broker {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _waited = self.0.wait();
    }
}

/// One HTTP/1.1 exchange; the raw answer, empty when nothing listens.
fn http(port: u16, method: &str, path: &str, body: &str) -> String {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return String::new();
    };
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut out = String::new();
    if s.write_all(req.as_bytes()).is_ok() {
        let _read = s.read_to_string(&mut out);
    }
    out
}

#[test]
fn mcp_the_broker_binary_registers_the_stream_sink() -> Result<(), String> {
    let port = free_port();
    let _broker = Command::new(env!("CARGO_BIN_EXE_antares"))
        .env("ANTARES_HTTP_PORT", port.to_string())
        .env("ANTARES_API_SURFACES", "admin,views,mcp")
        .spawn()
        .map(Broker)
        .map_err(|e| e.to_string())?;
    let healthy = (0..150).any(|_| {
        std::thread::sleep(Duration::from_millis(200));
        http(port, "GET", "/q/health", "").starts_with("HTTP/1.1 200")
    });
    assert!(healthy, "the broker on :{port} never got healthy");
    let named = r#"{"type":"Subscription","entities":[{"type":"Car"}],
        "notification":{"endpoint":{"uri":"x-antares-mcp:0123","accept":"application/json"}}}"#;
    let answer = http(port, "POST", "/ngsi-ld/v1/subscriptions", named);
    assert!(answer.starts_with("HTTP/1.1 400"), "{answer}");
    assert!(
        answer.contains("set by its own MCP stream, never named"),
        "{answer}"
    );
    Ok(())
}
