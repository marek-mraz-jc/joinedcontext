//! What a test's Postgres sends: rows and bytes, counted on the wire between a pool and the
//! server, so a test says how much a call reads whatever SQL it runs. Included with `#[path]`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

/// The length and code of the packet that asks for TLS.
const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 4, 210, 22, 47];

#[derive(Default)]
struct Read {
    rows: AtomicU64,
    bytes: AtomicU64,
}

pub struct PgMeter {
    pub port: u16,
    read: Arc<Read>,
}

impl PgMeter {
    /// A listener on loopback that forwards to `host:port` and refuses TLS, so it can count.
    pub async fn start(host: &str, port: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("meter listens");
        let own = listener.local_addr().expect("address").port();
        let read = Arc::new(Read::default());
        let (counted, host) = (Arc::clone(&read), host.to_owned());
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                tokio::spawn(forward(client, host.clone(), port, Arc::clone(&counted)));
            }
        });
        Self { port: own, read }
    }

    /// The rows and bytes the server sent since the last call.
    pub fn take(&self) -> (u64, u64) {
        (
            self.read.rows.swap(0, Ordering::SeqCst),
            self.read.bytes.swap(0, Ordering::SeqCst),
        )
    }
}

async fn forward(client: TcpStream, host: String, port: u16, read: Arc<Read>) {
    let server = TcpStream::connect((host.as_str(), port))
        .await
        .expect("server");
    // A message is passed on in two writes: without this each waits for the other side's ack.
    for socket in [&client, &server] {
        socket.set_nodelay(true).expect("nodelay");
    }
    let (mut from_client, mut to_client) = client.into_split();
    let (mut from_server, mut to_server) = server.into_split();
    let mut first = [0u8; 8];
    if from_client.read_exact(&mut first).await.is_err() {
        return;
    }
    if first == SSL_REQUEST {
        if to_client.write_all(b"N").await.is_err() {
            return;
        }
    } else if to_server.write_all(&first).await.is_err() {
        return;
    }
    tokio::spawn(async move { tokio::io::copy(&mut from_client, &mut to_server).await });
    let mut head = [0u8; 5];
    while from_server.read_exact(&mut head).await.is_ok() {
        let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]);
        let mut body = vec![0; len.saturating_sub(4) as usize];
        if from_server.read_exact(&mut body).await.is_err() {
            return;
        }
        // Counted before it is passed on, so the caller never reads the count early.
        read.bytes.fetch_add(u64::from(len) + 1, Ordering::SeqCst);
        if head[0] == b'D' {
            read.rows.fetch_add(1, Ordering::SeqCst);
        }
        if to_client.write_all(&head).await.is_err() || to_client.write_all(&body).await.is_err() {
            return;
        }
    }
}
