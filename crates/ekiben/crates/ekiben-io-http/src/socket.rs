//! Bento's `socket` input (`internal/impl/io/input_socket.go`) as a client: `tcp` dials
//! `address` and splits what arrives with `scanner` (`lines` by default, each line at most
//! `max_buffer` bytes; another scanner reads until the peer closes, at most 64 MiB); `udp`
//! connects to `address` and scans each datagram. The peer goes through the organization's
//! outbound rule before any packet is sent. A closed or failed connection is dialled again with
//! backoff; the input is resident. `unix` sockets are refused (no host paths for authors).

use std::collections::VecDeque;
use std::time::Duration;

use async_trait::async_trait;
use ekiben_core::{
    AckResponder, Batch, ComponentConfig, CoreError, Input, Registry, StreamContext,
};
use ekiben_io_file::inputs::Scanner;
use serde_json::Value as JsonValue;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::net::{TcpStream, UdpSocket};

const MAX_STREAM_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DATAGRAM: usize = 65_535;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub fn register(registry: &Registry) {
    registry.register_input("socket", |conf, _ctx| {
        Ok(Box::new(SocketInput::from_config(conf)?))
    });
}

enum Conn {
    Tcp(BufReader<TcpStream>),
    Udp(UdpSocket),
}

pub struct SocketInput {
    udp: bool,
    host: String,
    port: u16,
    scanner: Scanner,
    max_buffer: usize,
    conn: Option<Conn>,
    pending: VecDeque<Batch>,
    backoff: Duration,
    label: String,
}

impl SocketInput {
    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("socket input config must be an object"))?;
        let udp = match obj.get("network").and_then(JsonValue::as_str).unwrap_or("") {
            "tcp" => false,
            "udp" => true,
            "unix" => {
                return Err(CoreError::config(
                    "socket input: network unix is not supported by ekiben, use tcp or udp",
                ))
            }
            other => {
                return Err(CoreError::config(format!(
                    "socket input: network must be tcp or udp, not '{other}'"
                )))
            }
        };
        let address = obj.get("address").and_then(JsonValue::as_str).unwrap_or("");
        let (host, port) = address
            .rsplit_once(':')
            .and_then(|(h, p)| Some((h.trim_start_matches('[').trim_end_matches(']'), p.parse::<u16>().ok()?)))
            .filter(|(h, p)| !h.is_empty() && *p != 0)
            .ok_or_else(|| {
                CoreError::config(format!(
                    "socket input: address must be host:port such as counters.example.org:5000, got '{address}'"
                ))
            })?;
        let max_buffer = obj
            .get("max_buffer")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1_000_000)
            .clamp(1, MAX_STREAM_BYTES) as usize;
        Ok(Self {
            udp,
            host: host.to_string(),
            port,
            scanner: Scanner::lines_by_default(obj)?,
            max_buffer,
            conn: None,
            pending: VecDeque::new(),
            backoff: Duration::from_millis(100),
            label: conf.label.clone(),
        })
    }

    async fn dial(&self, ctx: &StreamContext) -> Result<Conn, CoreError> {
        let addrs = crate::outbound::resolve_host_checked(&self.host, self.port, &ctx.org).await?;
        if self.udp {
            let bind = if addrs[0].is_ipv4() {
                "0.0.0.0:0"
            } else {
                "[::]:0"
            };
            let sock = UdpSocket::bind(bind)
                .await
                .map_err(|e| CoreError::other(format!("socket input: udp bind: {e}")))?;
            sock.connect(addrs[0])
                .await
                .map_err(|e| CoreError::other(format!("socket input: udp connect: {e}")))?;
            Ok(Conn::Udp(sock))
        } else {
            let tcp = crate::outbound::connect_checked(&addrs, CONNECT_TIMEOUT).await?;
            Ok(Conn::Tcp(BufReader::new(tcp)))
        }
    }

    /// The next messages from the connection; `None` when the peer closed it.
    async fn read(&mut self) -> Result<Option<Batch>, CoreError> {
        let io = |e: std::io::Error| CoreError::other(format!("socket input: {e}"));
        let lines = self.scanner.is_lines();
        let max = self.max_buffer;
        match self.conn.as_mut() {
            None => Ok(None),
            Some(Conn::Udp(sock)) => {
                let mut buf = vec![0u8; MAX_DATAGRAM];
                let n = sock.recv(&mut buf).await.map_err(io)?;
                buf.truncate(n);
                Ok(Some(self.scanner.scan(buf)?))
            }
            Some(Conn::Tcp(r)) if lines => {
                let mut line = Vec::new();
                let n = r
                    .take(max as u64 + 1)
                    .read_until(b'\n', &mut line)
                    .await
                    .map_err(io)?;
                if n == 0 {
                    return Ok(None);
                }
                if line.len() > max && !line.ends_with(b"\n") {
                    return Err(CoreError::other(format!(
                        "socket input: a line is over max_buffer ({max} bytes)"
                    )));
                }
                Ok(Some(self.scanner.scan(line)?))
            }
            Some(Conn::Tcp(r)) => {
                let mut all = Vec::new();
                r.take(MAX_STREAM_BYTES + 1)
                    .read_to_end(&mut all)
                    .await
                    .map_err(io)?;
                if all.len() as u64 > MAX_STREAM_BYTES {
                    return Err(CoreError::other(format!(
                        "socket input: the peer sent over {MAX_STREAM_BYTES} bytes before closing"
                    )));
                }
                self.conn = None;
                if all.is_empty() {
                    return Ok(None);
                }
                Ok(Some(self.scanner.scan(all)?))
            }
        }
    }
}

#[async_trait]
impl Input for SocketInput {
    fn input_type(&self) -> &'static str {
        "socket"
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        self.conn = Some(self.dial(ctx).await?);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.conn.is_some()
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        loop {
            if let Some(b) = self.pending.pop_front() {
                return Ok((b, AckResponder::noop()));
            }
            if self.conn.is_none() {
                match self.dial(ctx).await {
                    Ok(c) => {
                        self.conn = Some(c);
                        self.backoff = Duration::from_millis(100);
                    }
                    Err(e @ (CoreError::AccessDenied { .. } | CoreError::Config(_))) => {
                        return Err(e)
                    }
                    Err(e) => {
                        tracing::warn!(
                            "socket input: dial failed, retrying in {:?}: {e}",
                            self.backoff
                        );
                        tokio::time::sleep(self.backoff).await;
                        self.backoff = (self.backoff * 2).min(Duration::from_secs(30));
                        continue;
                    }
                }
            }
            match self.read().await {
                Ok(Some(b)) if b.is_empty() => {}
                Ok(Some(b)) => self.pending.push_back(b),
                Ok(None) => self.conn = None,
                Err(e) => {
                    self.conn = None;
                    return Err(e);
                }
            }
        }
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        self.conn = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekiben_core::{OrgId, OrgQuotas, ResourceManager, StreamMetrics};
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    fn ctx(allow_private_ips: bool) -> StreamContext {
        let resources = ResourceManager::new();
        let org = resources.set_org_quotas(
            &OrgId::new("org-socket"),
            OrgQuotas {
                allow_private_ips,
                ..Default::default()
            },
        );
        let (_tx, rx) = tokio::sync::watch::channel(false);
        StreamContext {
            stream_id: "socket-test".into(),
            org,
            resources,
            registry: Arc::new(Registry::new()),
            blobl_env: ekiben_bloblang::Environment::new(),
            metrics: StreamMetrics::new("socket-test", "org-socket"),
            label: String::new(),
            path: "root.input".into(),
            file_root: None,
            env_vars: Arc::new(HashMap::new()),
            shutdown_rx: rx,
        }
    }

    fn texts(b: &Batch) -> Vec<String> {
        b.iter()
            .map(|p| String::from_utf8_lossy(&p.as_bytes_ref()).to_string())
            .collect()
    }

    #[tokio::test]
    async fn tcp_lines_arrive_as_messages_and_a_closed_peer_is_dialled_again() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            for payload in [&b"a\nb\n"[..], &b"c\n"[..]] {
                let (mut s, _) = l.accept().await.unwrap();
                s.write_all(payload).await.unwrap();
            }
        });
        let ctx = ctx(true);
        let mut i = input(json!({"network": "tcp", "address": addr.to_string()})).unwrap();
        let mut got = Vec::new();
        while got.len() < 3 {
            got.extend(texts(&i.read_batch(&ctx).await.unwrap().0));
        }
        assert_eq!(got, ["a", "b", "c"]);
    }

    #[tokio::test]
    async fn a_line_over_max_buffer_is_refused() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            s.write_all(b"0123456789\n").await.unwrap();
        });
        let mut i =
            input(json!({"network": "tcp", "address": addr.to_string(), "max_buffer": 4})).unwrap();
        let Err(e) = i.read_batch(&ctx(true)).await else {
            panic!("refused")
        };
        assert!(e.to_string().contains("max_buffer"), "{e}");
    }

    #[tokio::test]
    async fn each_udp_datagram_is_scanned() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let ctx = ctx(true);
        let mut i =
            input(json!({"network": "udp", "address": server.local_addr().unwrap().to_string()}))
                .unwrap();
        i.connect(&ctx).await.unwrap();
        let Some(Conn::Udp(sock)) = &i.conn else {
            panic!("udp")
        };
        let me = sock.local_addr().unwrap();
        server.send_to(b"x=1\ny=2", me).await.unwrap();
        assert_eq!(texts(&i.read_batch(&ctx).await.unwrap().0), ["x=1", "y=2"]);
    }

    #[tokio::test]
    async fn a_private_peer_is_refused_before_connecting() {
        for network in ["tcp", "udp"] {
            let mut i = input(json!({"network": network, "address": "127.0.0.1:9"})).unwrap();
            let Err(e) = i.read_batch(&ctx(false)).await else {
                panic!("refused")
            };
            assert!(
                matches!(e, CoreError::AccessDenied { .. }),
                "{network}: {e}"
            );
        }
    }

    fn input(v: JsonValue) -> Result<SocketInput, CoreError> {
        SocketInput::from_config(&ComponentConfig {
            component_type: "socket".into(),
            config: v,
            ..Default::default()
        })
    }

    #[test]
    fn unix_sockets_and_bad_addresses_are_refused() {
        assert!(input(json!({"network": "unix", "address": "/tmp/x"})).is_err());
        assert!(input(json!({"network": "tcp", "address": "nohost"})).is_err());
        assert!(input(json!({"network": "tcp", "address": "h:0"})).is_err());
        let i = input(json!({"network": "tcp", "address": "[::1]:5000"})).unwrap();
        assert_eq!((i.host.as_str(), i.port), ("::1", 5000));
    }
}
