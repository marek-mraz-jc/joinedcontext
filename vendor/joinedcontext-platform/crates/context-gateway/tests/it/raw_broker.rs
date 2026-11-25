//! A broker scripted request by request over a raw socket, for the answers a real broker never
//! sends on purpose: a connection that hangs up, a body cut short, one larger than the gateway
//! holds. Every connection carries one request and is closed after the answer.

use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// What the scripted broker does with one request.
pub enum Reply {
    /// A JSON body under `status`.
    Json(u16, Value),
    /// Any bytes under `status` and the media type.
    Bytes(u16, &'static str, Vec<u8>),
    /// The connection closes before any answer.
    Hangup,
    /// The answer announces a long body, sends a few bytes and closes.
    Cut,
}

/// One request as the scripted broker read it.
pub struct Req {
    pub method: String,
    pub target: String,
    pub head: String,
    pub body: String,
}

impl Req {
    /// The path from `/ngsi-ld/v1` on, without a view prefix and without the query.
    pub fn path(&self) -> &str {
        let path = self.target.split('?').next().unwrap_or_default();
        path.find("/ngsi-ld/v1").map_or(path, |at| &path[at..])
    }

    /// The decoded value of query parameter `name`.
    pub fn param(&self, name: &str) -> Option<String> {
        let query = self.target.split_once('?')?.1;
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| context_gateway::query::decode(key) == name)
            .map(|(_, value)| context_gateway::query::decode(value))
    }

    /// The decoded ids of the `id` parameter.
    pub fn ids(&self) -> Vec<String> {
        self.param("id")
            .map(|ids| ids.split(',').map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// The request's header `name`, lowercase in `head`.
    pub fn header(&self, name: &str) -> Option<String> {
        self.head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.trim().eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim().to_owned())
    }

    pub fn is(&self, method: &str, path: &str) -> bool {
        self.method == method && self.path() == path
    }
}

/// The scripted broker's address and every request it saw as `METHOD target`.
pub struct Scripted {
    pub url: String,
    pub calls: Arc<Mutex<Vec<String>>>,
}

impl Scripted {
    /// The calls that are not reads.
    pub fn writes(&self) -> Vec<String> {
        self.calls
            .lock()
            .expect("the call log")
            .iter()
            .filter(|call| !call.starts_with("GET "))
            .cloned()
            .collect()
    }

    /// How many calls start with `prefix`.
    pub fn count(&self, prefix: &str) -> usize {
        self.calls
            .lock()
            .expect("the call log")
            .iter()
            .filter(|call| call.starts_with(prefix))
            .count()
    }
}

/// Starts a broker that answers each request with what `script` makes of it; the second
/// argument counts the calls so far, this one included.
pub async fn spawn(script: impl Fn(&Req, usize) -> Reply + Send + Sync + 'static) -> Scripted {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let url = format!("http://{}", listener.local_addr().expect("an address"));
    let calls: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = Arc::clone(&calls);
    let script = Arc::new(script);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let (script, log) = (Arc::clone(&script), Arc::clone(&log));
            tokio::spawn(async move {
                let mut held = Vec::new();
                let mut chunk = [0u8; 8192];
                let split = loop {
                    if let Some(at) = held.windows(4).position(|window| window == b"\r\n\r\n") {
                        break at + 4;
                    }
                    match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => held.extend_from_slice(&chunk[..n]),
                    }
                };
                let head = String::from_utf8_lossy(&held[..split]).into_owned();
                let wanted: usize = head
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, value)| value.trim().parse().ok())
                    .unwrap_or(0);
                while held.len() < split + wanted {
                    match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => held.extend_from_slice(&chunk[..n]),
                    }
                }
                let mut line = head.lines().next().unwrap_or_default().split(' ');
                let request = Req {
                    method: line.next().unwrap_or_default().to_owned(),
                    target: line.next().unwrap_or_default().to_owned(),
                    head: head.clone(),
                    body: String::from_utf8_lossy(&held[split..]).into_owned(),
                };
                let nth = {
                    let mut log = log.lock().expect("the call log");
                    log.push(format!("{} {}", request.method, request.target));
                    log.len()
                };
                let (status, media, body, announced) = match script(&request, nth) {
                    Reply::Hangup => return,
                    Reply::Json(status, value) => (
                        status,
                        "application/json",
                        value.to_string().into_bytes(),
                        None,
                    ),
                    Reply::Bytes(status, media, body) => (status, media, body, None),
                    Reply::Cut => (200, "application/json", b"[{\"id\"".to_vec(), Some(100_000)),
                };
                let length = announced.unwrap_or(body.len());
                let answer = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: {media}\r\ncontent-length: {length}\r\nconnection: close\r\n\r\n"
                );
                let _ = socket.write_all(answer.as_bytes()).await;
                let _ = socket.write_all(&body).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    Scripted { url, calls }
}
