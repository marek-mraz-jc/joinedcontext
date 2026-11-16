//! Broker connections: one TCP (or TLS) stream per broker, each opened to the addresses the
//! organization's outbound rule checked, then SASL, then requests one at a time.
//!
//! The seed brokers and every broker a Metadata answer advertises go through the same
//! `connect`, so a cluster cannot steer the runner past the outbound rule by advertising another
//! host. A response bigger than `broker_read_max_bytes` is refused before it is read.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use ekiben_core::redact::SecretString;
use ekiben_core::{CoreError, OrgContext};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::scram;
use crate::wire::{error_name, frame, Dec, Enc, WireError};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub const METADATA: i16 = 3;
pub const FETCH: i16 = 1;
pub const LIST_OFFSETS: i16 = 2;
pub const OFFSET_COMMIT: i16 = 8;
pub const OFFSET_FETCH: i16 = 9;
pub const FIND_COORDINATOR: i16 = 10;
pub const JOIN_GROUP: i16 = 11;
pub const HEARTBEAT: i16 = 12;
pub const LEAVE_GROUP: i16 = 13;
pub const SYNC_GROUP: i16 = 14;
const SASL_HANDSHAKE: i16 = 17;
const SASL_AUTHENTICATE: i16 = 36;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mechanism {
    Plain,
    ScramSha256,
    ScramSha512,
}

impl Mechanism {
    pub fn name(self) -> &'static str {
        match self {
            Mechanism::Plain => "PLAIN",
            Mechanism::ScramSha256 => "SCRAM-SHA-256",
            Mechanism::ScramSha512 => "SCRAM-SHA-512",
        }
    }
}

pub struct Sasl {
    pub mechanism: Mechanism,
    pub username: String,
    pub password: SecretString,
}

/// What every connection of one input shares.
pub struct ConnSettings {
    pub client_id: String,
    pub tls: Option<Arc<rustls::ClientConfig>>,
    pub sasl: Option<Sasl>,
    pub max_response: usize,
    pub request_timeout: Duration,
    pub org: Arc<OrgContext>,
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> Io for T {}

pub struct Conn {
    io: Box<dyn Io>,
    correlation: i32,
}

pub fn failed(what: impl std::fmt::Display) -> CoreError {
    CoreError::other(format!("kafka_franz input: {what}"))
}

fn wire(e: WireError) -> CoreError {
    failed(e)
}

/// The TLS settings: the system roots plus `root_cas`; no way to skip verification.
pub fn tls_config(root_cas: &[Vec<u8>]) -> Result<Arc<rustls::ClientConfig>, CoreError> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    for der in root_cas {
        roots
            .add(rustls_pki_types::CertificateDer::from(der.clone()))
            .map_err(|e| CoreError::config(format!("kafka_franz input: tls.root_cas: {e}")))?;
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(failed)?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(config))
}

impl Conn {
    /// Opens a connection to `host:port` through the outbound rule, with TLS and SASL as set.
    pub async fn connect(host: &str, port: u16, s: &ConnSettings) -> Result<Self, CoreError> {
        let addrs = ekiben_io_http::outbound::resolve_host_checked(host, port, &s.org).await?;
        let tcp = ekiben_io_http::outbound::connect_checked(&addrs, CONNECT_TIMEOUT)
            .await
            .map_err(|e| failed(format!("broker {host}:{port}: {e}")))?;
        let io: Box<dyn Io> = match &s.tls {
            None => Box::new(tcp),
            Some(cfg) => {
                let bare = host.trim_start_matches('[').trim_end_matches(']');
                let name =
                    rustls_pki_types::ServerName::try_from(bare.to_string()).map_err(|e| {
                        CoreError::config(format!("kafka_franz input: broker {host}: {e}"))
                    })?;
                let tls = tokio::time::timeout(
                    CONNECT_TIMEOUT,
                    tokio_rustls::TlsConnector::from(cfg.clone()).connect(name, tcp),
                )
                .await
                .map_err(|_| failed(format!("broker {host}:{port}: TLS handshake timed out")))?
                .map_err(|e| failed(format!("broker {host}:{port}: TLS handshake failed: {e}")))?;
                Box::new(tls)
            }
        };
        let mut conn = Conn { io, correlation: 0 };
        if let Some(sasl) = &s.sasl {
            conn.authenticate(sasl, s).await?;
        }
        Ok(conn)
    }

    /// Sends one request and reads its answer (the correlation id checked and stripped).
    pub async fn call(
        &mut self,
        api_key: i16,
        version: i16,
        body: &[u8],
        s: &ConnSettings,
        timeout: Duration,
    ) -> Result<Dec, CoreError> {
        self.correlation = self.correlation.wrapping_add(1);
        let req = frame(api_key, version, self.correlation, &s.client_id, body);
        let exchange = async {
            self.io.write_all(&req).await?;
            self.io.flush().await?;
            let size = self.io.read_i32().await?;
            Ok::<i32, std::io::Error>(size)
        };
        let size = tokio::time::timeout(timeout, exchange)
            .await
            .map_err(|_| failed("the broker did not answer in time"))?
            .map_err(|e| failed(format!("connection: {e}")))?;
        let size = usize::try_from(size).map_err(|_| failed("a response size is negative"))?;
        if size > s.max_response {
            return Err(failed(format!(
                "a response of {size} bytes is over broker_read_max_bytes ({} bytes)",
                s.max_response
            )));
        }
        if size < 4 {
            return Err(wire(WireError("a response has no header")));
        }
        let mut buf = vec![0u8; size];
        tokio::time::timeout(timeout, self.io.read_exact(&mut buf))
            .await
            .map_err(|_| failed("the broker did not answer in time"))?
            .map_err(|e| failed(format!("connection: {e}")))?;
        let mut d = Dec(Bytes::from(buf));
        if d.i32().map_err(wire)? != self.correlation {
            return Err(failed("a response answers another request"));
        }
        Ok(d)
    }

    async fn authenticate(&mut self, sasl: &Sasl, s: &ConnSettings) -> Result<(), CoreError> {
        let mut e = Enc::default();
        e.str(sasl.mechanism.name());
        let mut d = self
            .call(SASL_HANDSHAKE, 1, &e.0, s, s.request_timeout)
            .await?;
        let code = d.i16().map_err(wire)?;
        if code != 0 {
            let offered = d.array(|d| d.str()).unwrap_or_default().join(", ");
            return Err(CoreError::config(format!(
                "kafka_franz input: the broker does not offer SASL {} ({}); it offers: {offered}",
                sasl.mechanism.name(),
                error_name(code)
            )));
        }
        match sasl.mechanism {
            Mechanism::Plain => {
                let msg = format!("\0{}\0{}", sasl.username, sasl.password.expose());
                self.sasl_step(msg.as_bytes(), s).await?;
            }
            Mechanism::ScramSha256 | Mechanism::ScramSha512 => {
                let client = scram::Client::new(sasl.mechanism, &sasl.username);
                let first = self.sasl_step(client.first().as_bytes(), s).await?;
                let (final_msg, check) = client
                    .final_message(&first, sasl.password.expose())
                    .map_err(|e| failed(format!("SASL {}: {e}", sasl.mechanism.name())))?;
                let last = self.sasl_step(final_msg.as_bytes(), s).await?;
                check
                    .verify(&last)
                    .map_err(|e| failed(format!("SASL {}: {e}", sasl.mechanism.name())))?;
            }
        }
        Ok(())
    }

    async fn sasl_step(&mut self, msg: &[u8], s: &ConnSettings) -> Result<Vec<u8>, CoreError> {
        let mut e = Enc::default();
        e.bytes(msg);
        let mut d = self
            .call(SASL_AUTHENTICATE, 0, &e.0, s, s.request_timeout)
            .await?;
        let code = d.i16().map_err(wire)?;
        let message = d.null_str().map_err(wire)?.unwrap_or_default();
        if code != 0 {
            return Err(CoreError::config(format!(
                "kafka_franz input: SASL authentication failed ({}): {message}; check sasl.username and the password secret",
                error_name(code)
            )));
        }
        Ok(d.bytes().map_err(wire)?.to_vec())
    }
}

/// A broker as Metadata advertises it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Broker {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Debug, Default)]
pub struct Metadata {
    pub brokers: HashMap<i32, Broker>,
    /// topic -> (partition, leader) for every partition without an error.
    pub topics: HashMap<String, Vec<(i32, i32)>>,
    /// Topics the broker answered with an error, and its name.
    pub errors: Vec<(String, &'static str)>,
}

pub fn decode_metadata(mut d: Dec) -> Result<Metadata, WireError> {
    let mut m = Metadata::default();
    d.i32()?; // throttle
    for (id, host, port) in d.array(|d| {
        let id = d.i32()?;
        let host = d.str()?;
        let port = d.i32()?;
        d.null_str()?; // rack
        Ok((id, host, port))
    })? {
        let port = u16::try_from(port).map_err(|_| WireError("a broker port is out of range"))?;
        m.brokers.insert(id, Broker { host, port });
    }
    d.null_str()?; // cluster id
    d.i32()?; // controller
    for (code, name, parts) in d.array(|d| {
        let code = d.i16()?;
        let name = d.str()?;
        d.bool()?; // internal
        let parts = d.array(|d| {
            let code = d.i16()?;
            let index = d.i32()?;
            let leader = d.i32()?;
            d.array(|d| d.i32())?; // replicas
            d.array(|d| d.i32())?; // isr
            Ok((code, index, leader))
        })?;
        Ok((code, name, parts))
    })? {
        if code != 0 {
            m.errors.push((name, error_name(code)));
            continue;
        }
        let ok = parts
            .into_iter()
            .filter(|(c, _, leader)| *c == 0 && *leader >= 0)
            .map(|(_, p, leader)| (p, leader))
            .collect();
        m.topics.insert(name, ok);
    }
    Ok(m)
}

/// Every connection of one input: the seeds, the brokers by node id, the metadata last read.
pub struct Cluster {
    pub s: ConnSettings,
    pub seeds: Vec<(String, u16)>,
    pub meta: Metadata,
    conns: HashMap<i32, Conn>,
}

impl Cluster {
    pub fn new(s: ConnSettings, seeds: Vec<(String, u16)>) -> Self {
        Self {
            s,
            seeds,
            meta: Metadata::default(),
            conns: HashMap::new(),
        }
    }

    /// Reads metadata for `topics` (None: every topic) from the first seed that answers.
    pub async fn refresh(&mut self, topics: Option<&[String]>) -> Result<(), CoreError> {
        let mut e = Enc::default();
        match topics {
            None => e.i32(-1),
            Some(t) => e.array(t, |e, t| {
                e.str(t);
            }),
        };
        e.bool(false); // never create a topic by reading it
        let mut last = failed("no seed broker");
        let seeds = self.seeds.clone();
        for (host, port) in &seeds {
            let answer = async {
                let mut c = Conn::connect(host, *port, &self.s).await?;
                let d = c
                    .call(METADATA, 4, &e.0, &self.s, self.s.request_timeout)
                    .await?;
                decode_metadata(d).map_err(wire)
            };
            match answer.await {
                Ok(m) => {
                    // a broker whose address changed is dialled again
                    self.conns
                        .retain(|id, _| m.brokers.get(id) == self.meta.brokers.get(id));
                    self.meta = m;
                    return Ok(());
                }
                Err(e @ (CoreError::AccessDenied { .. } | CoreError::Config(_))) => return Err(e),
                Err(e) => {
                    tracing::warn!("kafka_franz input: seed {host}:{port}: {e}");
                    last = e;
                }
            }
        }
        Err(last)
    }

    /// Calls broker `node`, connecting first when needed; a failed call drops the connection.
    pub async fn call(
        &mut self,
        node: i32,
        api_key: i16,
        version: i16,
        body: &[u8],
        timeout: Duration,
    ) -> Result<Dec, CoreError> {
        if !self.conns.contains_key(&node) {
            let b = self
                .meta
                .brokers
                .get(&node)
                .cloned()
                .ok_or_else(|| failed(format!("broker {node} is not in the metadata")))?;
            let c = Conn::connect(&b.host, b.port, &self.s).await?;
            self.conns.insert(node, c);
        }
        let Some(conn) = self.conns.get_mut(&node) else {
            return Err(failed(format!("broker {node} is not connected")));
        };
        let r = conn.call(api_key, version, body, &self.s, timeout).await;
        if r.is_err() {
            self.conns.remove(&node);
        }
        r
    }

    /// Opens a connection of its own to a broker host (the coordinator's heartbeats use one).
    pub async fn dial(&self, b: &Broker) -> Result<Conn, CoreError> {
        Conn::connect(&b.host, b.port, &self.s).await
    }

    pub fn drop_conns(&mut self) {
        self.conns.clear();
    }
}
