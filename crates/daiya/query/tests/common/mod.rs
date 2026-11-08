//! Shared test helpers: a raw-TCP HTTP mock on 127.0.0.1 and a guard that may reach it.

#![allow(dead_code)]

use daiya_model::{InstanceId, OrgId, Subject};
use daiya_query::{GuardConfig, OutboundGuard, Query, QueryCtx};
use ipnet::IpNet;
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const ORG: OrgId = OrgId::from_u128(42);

/// One canned answer.
#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Wait this long before answering.
    pub delay: Duration,
    /// Announce this content-length instead of the body's (a lie cuts the body short).
    pub content_length: Option<Option<usize>>,
}

impl Reply {
    pub fn json(status: u16, body: &Value) -> Self {
        Self::raw(status, &body.to_string()).header("content-type", "application/json")
    }
    pub fn raw(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
            delay: Duration::ZERO,
            content_length: None,
        }
    }
    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
    pub fn delay(mut self, d: Duration) -> Self {
        self.delay = d;
        self
    }
    /// No content-length: the body runs until the connection closes.
    pub fn streamed(mut self) -> Self {
        self.content_length = Some(None);
        self
    }
    pub fn claim_length(mut self, n: usize) -> Self {
        self.content_length = Some(Some(n));
        self
    }
}

pub struct Mock {
    pub base: String,
    pub port: u16,
    /// Every request head (request line and headers), in order.
    pub seen: Arc<Mutex<Vec<String>>>,
}

impl Mock {
    /// Request target (path and query) of the n-th request, percent-decoded.
    pub fn target(&self, n: usize) -> String {
        let head = self.seen.lock()[n].clone();
        let raw = head.split_whitespace().nth(1).unwrap_or("").to_owned();
        percent_encoding::percent_decode_str(&raw)
            .decode_utf8_lossy()
            .into_owned()
    }
    pub fn head(&self, n: usize) -> String {
        self.seen.lock()[n].clone()
    }
    pub fn count(&self) -> usize {
        self.seen.lock().len()
    }
}

/// Serves the first route whose prefix the request target starts with; 404 otherwise.
pub async fn mock(routes: Vec<(&'static str, Reply)>) -> Mock {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let routes = Arc::new(routes);
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let log = log.clone();
            let routes = routes.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let target = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                log.lock().push(head);
                let reply = routes
                    .iter()
                    .find(|(prefix, _)| target.starts_with(prefix))
                    .map_or_else(|| Reply::raw(404, "not found"), |(_, r)| r.clone());
                tokio::time::sleep(reply.delay).await;
                let mut out = format!("HTTP/1.1 {} X\r\nconnection: close\r\n", reply.status);
                for (k, v) in &reply.headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                match reply.content_length {
                    None => out.push_str(&format!("content-length: {}\r\n", reply.body.len())),
                    Some(Some(n)) => out.push_str(&format!("content-length: {n}\r\n")),
                    Some(None) => {}
                }
                out.push_str("\r\n");
                let _ = sock.write_all(out.as_bytes()).await;
                let _ = sock.write_all(&reply.body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    Mock {
        base: format!("http://127.0.0.1:{port}"),
        port,
        seen,
    }
}

/// A port nothing listens on.
pub async fn closed_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

/// A guard that reaches 127.0.0.1 over http (the operator opened the loopback network).
pub fn loopback_guard() -> OutboundGuard {
    OutboundGuard::new(GuardConfig {
        allow_hosts: vec!["127.0.0.1".into()],
        allow_http: true,
        allowed_networks: vec!["127.0.0.0/8".parse::<IpNet>().unwrap()],
        ..GuardConfig::default()
    })
}

pub fn ctx(subject: Subject) -> QueryCtx {
    QueryCtx::new(
        ORG,
        InstanceId::from_u128(1),
        subject,
        1_700_000_000_000,
        1_700_000_600_000,
    )
}

pub fn q(model: Value) -> Vec<Query> {
    vec![Query {
        ref_id: "A".into(),
        hide: false,
        model,
    }]
}
