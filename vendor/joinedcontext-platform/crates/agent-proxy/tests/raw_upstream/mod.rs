//! A plain-socket upstream, for the answers wiremock cannot give (T-7795): a connection closed
//! without a word, which is what an unreachable service looks like to the client, and a body that
//! breaks off before its `Content-Length`. Every suite that includes it uses all of it.

use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// What the upstream writes back to one request before it closes the connection: the raw bytes,
/// or nothing at all.
pub type Answer = Option<Vec<u8>>;

/// A whole JSON answer.
pub fn reply(status: u16, body: &str) -> Answer {
    Some(
        format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes(),
    )
}

/// An answer that promises `declared` bytes and sends `sent` before it closes.
pub fn broken(status: u16, declared: usize, sent: &str) -> Answer {
    Some(
        format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n{sent}"
        )
        .into_bytes(),
    )
}

/// The upstream: `answer` gets each request's path (with its query) and body.
pub struct RawUpstream {
    /// `http://127.0.0.1:<port>`, without a trailing `/`.
    pub base: String,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl RawUpstream {
    pub async fn start(answer: impl Fn(&str, &str) -> Answer + Send + Sync + 'static) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let addr = listener.local_addr().expect("an address");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (answer, log) = (Arc::new(answer), Arc::clone(&seen));
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (answer, log) = (Arc::clone(&answer), Arc::clone(&log));
                tokio::spawn(async move {
                    let Some((path, body)) = read_request(&mut socket).await else {
                        return;
                    };
                    log.lock()
                        .expect("the log")
                        .push((path.clone(), body.clone()));
                    if let Some(bytes) = answer(&path, &body) {
                        let _ = socket.write_all(&bytes).await;
                        let _ = socket.flush().await;
                    }
                });
            }
        });
        Self {
            base: format!("http://{addr}"),
            seen,
        }
    }

    /// The paths asked for, in order of arrival.
    pub fn paths(&self) -> Vec<String> {
        self.seen
            .lock()
            .expect("the log")
            .iter()
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// The bodies sent to a path ending in `suffix`.
    pub fn bodies_at(&self, suffix: &str) -> Vec<String> {
        self.seen
            .lock()
            .expect("the log")
            .iter()
            .filter(|(path, _)| path.ends_with(suffix))
            .map(|(_, body)| body.clone())
            .collect()
    }
}

/// The path and body of one HTTP/1.1 request, read to its `Content-Length`.
async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while buf.len() < head_end + length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let path = head.split_whitespace().nth(1)?.to_owned();
    Some((path, String::from_utf8_lossy(&buf[head_end..]).into_owned()))
}
