// SPDX-License-Identifier: EUPL-1.2
//! `ANTARES_POLICY_SUBJECT_FROM` against the real binary (T-4875, T-6432):
//! a subject header counts only from a listed mesh identity. The drop itself
//! is unit-tested in `antares_api::policy`; this test proves `run` wires it
//! in front of the router.
#![cfg(feature = "jc")]

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

const GATEWAY: &str = "context-gateway.jc.serviceaccount.identity.linkerd.cluster.local";

fn get(port: u16, path: &str, headers: &[(&str, &str)]) -> String {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) else {
        return String::new();
    };
    let mut req = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    let _ = s.write_all(req.as_bytes());
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

#[test]
fn a_subject_from_an_unlisted_identity_counts_as_no_subject() {
    let port = free_port();
    let _broker = Broker(
        Command::new(env!("CARGO_BIN_EXE_antares"))
            .env("ANTARES_HTTP_PORT", port.to_string())
            .env("ANTARES_POLICY_SUBJECT_HEADERS", "x-jc-subject")
            .env("ANTARES_POLICY_SUBJECT_FROM", GATEWAY)
            .spawn()
            .expect("spawn antares"),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !get(port, "/q/health", &[]).starts_with("HTTP/1.1 200") {
        assert!(Instant::now() < deadline, "broker never got healthy");
        std::thread::sleep(Duration::from_millis(200));
    }
    let entities = "/ngsi-ld/v1/entities?type=T";
    let unlisted = [
        vec![("x-jc-subject", "alice")],
        vec![
            ("x-jc-subject", "alice"),
            (
                "l5d-client-id",
                "default.scratch.serviceaccount.identity.linkerd.cluster.local",
            ),
        ],
    ];
    for headers in &unlisted {
        let answer = get(port, entities, headers);
        assert!(answer.starts_with("HTTP/1.1 401"), "{headers:?}: {answer}");
    }
    let gateway = [("x-jc-subject", "alice"), ("l5d-client-id", GATEWAY)];
    let answer = get(port, entities, &gateway);
    assert!(!answer.starts_with("HTTP/1.1 401"), "{answer}");
}
