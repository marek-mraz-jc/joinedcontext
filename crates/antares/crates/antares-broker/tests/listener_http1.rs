// SPDX-License-Identifier: EUPL-1.2
//! F-R2ANT2-005: the API listener speaks HTTP/1.1 alone. The connection
//! bounds (header read timeout, one permit per connection) are HTTP/1
//! settings; an h2c prior-knowledge connection would carry many streams
//! under one permit and no header deadline.

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

/// Everything the server sends back for `bytes`, within two seconds.
fn answer(port: u16, bytes: &[u8]) -> Vec<u8> {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return Vec::new();
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.write_all(bytes);
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    out
}

#[test]
fn an_h2c_prior_knowledge_connection_is_not_served() {
    let port = free_port();
    let _broker = Broker(
        Command::new(env!("CARGO_BIN_EXE_antares"))
            .env("ANTARES_HTTP_PORT", port.to_string())
            .spawn()
            .expect("spawn antares"),
    );
    let health = b"GET /q/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n";
    let deadline = Instant::now() + Duration::from_secs(30);
    while !answer(port, health).starts_with(b"HTTP/1.1 200") {
        assert!(Instant::now() < deadline, "broker never got healthy");
        std::thread::sleep(Duration::from_millis(200));
    }
    // RFC 9113 3.4: the client preface, then an empty SETTINGS frame. An h2
    // server answers with its own SETTINGS frame (type 0x04 at byte 3).
    let mut preface = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec();
    preface.extend_from_slice(&[0, 0, 0, 4, 0, 0, 0, 0, 0]);
    let out = answer(port, &preface);
    assert_ne!(out.get(3), Some(&4), "served as HTTP/2: {out:?}");
}
