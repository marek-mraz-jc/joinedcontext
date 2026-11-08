//! `websocket` input (Bento 1.22 `internal/impl/io/input_websocket.go`), a client that connects
//! to a websocket server and reads every text or binary message as a one-part batch.
//!
//! Bento's fields are kept: `url`, `headers` (interpolated), `open_message` / `open_messages`,
//! `open_message_type` (`binary` default, or `text`), `auto_replay_nacks` (default `true`),
//! `tls`, `basic_auth`, `oauth` (OAuth 1 HMAC-SHA1). Pings are answered by tokio-websockets.
//!
//! ekiben differences, documented in the crate README:
//! - The connection follows the outbound rule (`crate::outbound`): one DNS resolution, every
//!   address checked, the TCP connection made to a checked address; TLS (`wss`) is done by
//!   ekiben over that connection with the system roots (plus `tls.root_cas`).
//! - Refused: `proxy_url`, `jwt`, `tls.skip_cert_verify`, `tls.root_cas_file`,
//!   `tls.client_certs` (a proxy or a file would step outside the outbound rule or file root).
//! - `max_message_bytes` (ekiben extension; default 16 MiB, at most 64 MiB): a larger message
//!   closes the connection.
//! - A lost connection is reconnected inside `read_batch` with a capped backoff (1 s to 30 s);
//!   replayed nacks are held in a bounded queue (`MAX_REPLAY_BATCHES`).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use axum::http::{HeaderName, HeaderValue, Uri};
use base64::Engine as _;
use bytes::Bytes;
use ekiben_bloblang::FieldExpression;
use ekiben_core::{
    redact_url_str, AckResponder, Batch, ComponentConfig, CoreError, Input, Part, PartsView,
    Registry, SecretString, StreamContext,
};
use futures::{SinkExt, StreamExt};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::Value as JsonValue;
use sha1::Sha1;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_websockets::{ClientBuilder, Limits, Message, WebSocketStream};
use url::Url;

use crate::http_server::shutdown_signal;
use crate::outbound;

/// Default `max_message_bytes`.
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// Largest `max_message_bytes` accepted.
pub const MAX_MESSAGE_BYTES_LIMIT: usize = 64 * 1024 * 1024;
/// Nacked batches held for replay; past it a nacked batch is dropped (and logged).
pub const MAX_REPLAY_BATCHES: usize = 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

pub fn register(registry: &Registry) {
    registry.register_input("websocket", |conf, ctx| {
        Ok(Box::new(WebsocketInput::from_config(conf, ctx)?))
    });
}

/// A byte stream tokio-websockets can run over: plain TCP or TLS over TCP.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> Io for T {}

type Conn = WebSocketStream<Box<dyn Io>>;

#[derive(Clone)]
struct OAuth1 {
    consumer_key: String,
    consumer_secret: SecretString,
    access_token: String,
    access_token_secret: SecretString,
}

/// The parsed config of one `websocket` input.
pub struct WebsocketConf {
    pub url: String,
    headers: Vec<(String, FieldExpression)>,
    open_messages: Vec<Vec<u8>>,
    open_message_type: String,
    pub auto_replay_nacks: bool,
    basic_auth: Option<(String, SecretString)>,
    oauth: Option<OAuth1>,
    root_cas: Vec<Vec<u8>>,
    pub max_message_bytes: usize,
}

impl std::fmt::Debug for WebsocketConf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebsocketConf")
            .field("url", &redact_url_str(&self.url))
            .field(
                "headers",
                &self.headers.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .field("open_messages", &self.open_messages.len())
            .field("open_message_type", &self.open_message_type)
            .field("auto_replay_nacks", &self.auto_replay_nacks)
            .field(
                "basic_auth",
                &self.basic_auth.as_ref().map(|(u, _)| (u, "REDACTED")),
            )
            .field(
                "oauth",
                &self.oauth.as_ref().map(|o| (&o.consumer_key, "REDACTED")),
            )
            .field("max_message_bytes", &self.max_message_bytes)
            .finish()
    }
}

fn obj<'a>(
    map: &'a serde_json::Map<String, JsonValue>,
    key: &str,
) -> Option<&'a serde_json::Map<String, JsonValue>> {
    map.get(key).and_then(JsonValue::as_object)
}

fn enabled(map: Option<&serde_json::Map<String, JsonValue>>) -> bool {
    map.and_then(|m| m.get("enabled"))
        .and_then(JsonValue::as_bool)
        .unwrap_or(false)
}

fn s<'a>(map: &'a serde_json::Map<String, JsonValue>, key: &str) -> &'a str {
    map.get(key).and_then(JsonValue::as_str).unwrap_or_default()
}

/// The DER certificates of a PEM text.
fn pem_certificates(pem: &str) -> Result<Vec<Vec<u8>>, CoreError> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut out = Vec::new();
    let mut rest = pem;
    while let Some(start) = rest.find(BEGIN) {
        let after = rest.get(start + BEGIN.len()..).unwrap_or_default();
        let end = after
            .find(END)
            .ok_or_else(|| CoreError::config("tls.root_cas: unterminated certificate"))?;
        let b64: String = after
            .get(..end)
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let der = base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .map_err(|_| CoreError::config("tls.root_cas: a certificate is not valid base64"))?;
        out.push(der);
        rest = after.get(end + END.len()..).unwrap_or_default();
    }
    if out.is_empty() {
        return Err(CoreError::config("tls.root_cas: no PEM certificate found"));
    }
    Ok(out)
}

impl WebsocketConf {
    pub fn from_json(
        val: &JsonValue,
        env: &ekiben_bloblang::Environment,
    ) -> Result<Self, CoreError> {
        let empty = serde_json::Map::new();
        let map = val.as_object().unwrap_or(&empty);
        let url = s(map, "url").to_string();
        if url.is_empty() {
            return Err(CoreError::config("websocket.url is required"));
        }
        let parsed = Url::parse(&url).map_err(|e| {
            CoreError::config(format!("websocket.url '{}': {e}", redact_url_str(&url)))
        })?;
        if !matches!(parsed.scheme(), "ws" | "wss") {
            return Err(CoreError::config(format!(
                "websocket.url: scheme '{}' is not ws or wss",
                parsed.scheme()
            )));
        }
        if !s(map, "proxy_url").is_empty() {
            return Err(CoreError::config(
                "websocket.proxy_url is not supported by ekiben: the connection follows the outbound rule",
            ));
        }
        if enabled(obj(map, "jwt")) {
            return Err(CoreError::config(
                "websocket.jwt is not supported by ekiben",
            ));
        }
        let mut headers = Vec::new();
        if let Some(h) = obj(map, "headers") {
            for (k, v) in h {
                if HeaderName::from_bytes(k.as_bytes()).is_err() {
                    return Err(CoreError::config(format!(
                        "websocket.headers: '{k}' is not a valid header name"
                    )));
                }
                let src = v
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string());
                let expr = env
                    .compile_field(&src)
                    .map_err(|e| CoreError::config(format!("websocket.headers.{k}: {e}")))?;
                headers.push((k.clone(), expr));
            }
        }
        let list: Vec<Vec<u8>> = map
            .get("open_messages")
            .and_then(JsonValue::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(JsonValue::as_str)
                    .map(|m| m.as_bytes().to_vec())
                    .collect()
            })
            .unwrap_or_default();
        let single = s(map, "open_message");
        if !single.is_empty() && !list.is_empty() {
            return Err(CoreError::config(
                "both open_message and open_messages cannot be set",
            ));
        }
        let open_messages = if !list.is_empty() {
            list
        } else if !single.is_empty() {
            vec![single.as_bytes().to_vec()]
        } else {
            Vec::new()
        };
        let open_message_type = map
            .get("open_message_type")
            .and_then(JsonValue::as_str)
            .unwrap_or("binary")
            .to_string();
        let auto_replay_nacks = map
            .get("auto_replay_nacks")
            .and_then(JsonValue::as_bool)
            .unwrap_or(true);

        let mut root_cas = Vec::new();
        if let Some(tls) = obj(map, "tls") {
            if tls.get("skip_cert_verify").and_then(JsonValue::as_bool) == Some(true) {
                return Err(CoreError::config(
                    "websocket.tls.skip_cert_verify is not supported by ekiben",
                ));
            }
            if !s(tls, "root_cas_file").is_empty() {
                return Err(CoreError::config(
                    "websocket.tls.root_cas_file is not supported by ekiben; put the PEM in tls.root_cas",
                ));
            }
            if tls
                .get("client_certs")
                .and_then(JsonValue::as_array)
                .is_some_and(|a| !a.is_empty())
            {
                return Err(CoreError::config(
                    "websocket.tls.client_certs is not supported by ekiben",
                ));
            }
            let pem = s(tls, "root_cas");
            if !pem.is_empty() {
                root_cas = pem_certificates(pem)?;
            }
        }

        let basic = obj(map, "basic_auth");
        let basic_auth = if enabled(basic) {
            let b = basic.unwrap_or(&empty);
            Some((
                s(b, "username").to_string(),
                SecretString::new(s(b, "password")),
            ))
        } else {
            None
        };
        let oa = obj(map, "oauth");
        let oauth = if enabled(oa) {
            let o = oa.unwrap_or(&empty);
            Some(OAuth1 {
                consumer_key: s(o, "consumer_key").to_string(),
                consumer_secret: SecretString::new(s(o, "consumer_secret")),
                access_token: s(o, "access_token").to_string(),
                access_token_secret: SecretString::new(s(o, "access_token_secret")),
            })
        } else {
            None
        };
        let max_message_bytes = match map.get("max_message_bytes") {
            None | Some(JsonValue::Null) => DEFAULT_MAX_MESSAGE_BYTES,
            Some(v) => v
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| {
                    CoreError::config("websocket.max_message_bytes: expected a number")
                })?,
        };
        if max_message_bytes == 0 || max_message_bytes > MAX_MESSAGE_BYTES_LIMIT {
            return Err(CoreError::config(format!(
                "websocket.max_message_bytes must be between 1 and {MAX_MESSAGE_BYTES_LIMIT}"
            )));
        }
        Ok(Self {
            url,
            headers,
            open_messages,
            open_message_type,
            auto_replay_nacks,
            basic_auth,
            oauth,
            root_cas,
            max_message_bytes,
        })
    }
}

/// RFC 3986 percent-encoding of OAuth 1 (unreserved characters kept).
fn oauth_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The OAuth 1 `Authorization` header of a `GET url` (HMAC-SHA1, RFC 5849).
pub fn oauth1_header(
    url: &Url,
    consumer_key: &str,
    consumer_secret: &str,
    token: &str,
    token_secret: &str,
    nonce: &str,
    timestamp: u64,
) -> String {
    let mut params: Vec<(String, String)> = vec![
        ("oauth_consumer_key".into(), consumer_key.into()),
        ("oauth_nonce".into(), nonce.into()),
        ("oauth_signature_method".into(), "HMAC-SHA1".into()),
        ("oauth_timestamp".into(), timestamp.to_string()),
        ("oauth_token".into(), token.into()),
        ("oauth_version".into(), "1.0".into()),
    ];
    for (k, v) in url.query_pairs() {
        params.push((k.into_owned(), v.into_owned()));
    }
    let mut encoded: Vec<(String, String)> = params
        .iter()
        .map(|(k, v)| (oauth_encode(k), oauth_encode(v)))
        .collect();
    encoded.sort();
    let param_str = encoded
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let mut base_url = url.clone();
    base_url.set_query(None);
    base_url.set_fragment(None);
    let base = format!(
        "GET&{}&{}",
        oauth_encode(base_url.as_str()),
        oauth_encode(&param_str)
    );
    let key = format!(
        "{}&{}",
        oauth_encode(consumer_secret),
        oauth_encode(token_secret)
    );
    let signature = match <Hmac<Sha1>>::new_from_slice(key.as_bytes()) {
        Ok(mut mac) => {
            mac.update(base.as_bytes());
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
        }
        Err(_) => String::new(),
    };
    format!(
        "OAuth oauth_consumer_key=\"{}\", oauth_nonce=\"{}\", oauth_signature=\"{}\", oauth_signature_method=\"HMAC-SHA1\", oauth_timestamp=\"{}\", oauth_token=\"{}\", oauth_version=\"1.0\"",
        oauth_encode(consumer_key),
        oauth_encode(nonce),
        oauth_encode(&signature),
        timestamp,
        oauth_encode(token)
    )
}

async fn tls_connect(
    tcp: tokio::net::TcpStream,
    host: &str,
    extra_roots: &[Vec<u8>],
) -> Result<Box<dyn Io>, CoreError> {
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        let _ = roots.add(cert);
    }
    for der in extra_roots {
        roots
            .add(rustls_pki_types::CertificateDer::from(der.clone()))
            .map_err(|e| CoreError::config(format!("tls.root_cas: {e}")))?;
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| CoreError::other(format!("tls: {e}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let name = rustls_pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| CoreError::config(format!("tls: bad server name: {e}")))?;
    let stream = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .map_err(|e| CoreError::other(format!("tls handshake failed: {e}")))?;
    Ok(Box::new(stream))
}

/// The `websocket` input.
pub struct WebsocketInput {
    conf: Arc<WebsocketConf>,
    label: String,
    conn: Option<Conn>,
    closed: bool,
    backoff: Duration,
    replay: Arc<Mutex<VecDeque<Batch>>>,
}

impl WebsocketInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        Ok(Self {
            conf: Arc::new(WebsocketConf::from_json(&conf.config, &ctx.blobl_env)?),
            label: conf.label.clone(),
            conn: None,
            closed: false,
            backoff: BACKOFF_MIN,
            replay: Arc::new(Mutex::new(VecDeque::new())),
        })
    }

    async fn dial(&self, ctx: &StreamContext) -> Result<Conn, CoreError> {
        let conf = &self.conf;
        let msg_type_text = match conf.open_message_type.as_str() {
            "binary" => false,
            "text" => true,
            other => {
                return Err(CoreError::config(format!(
                    "unrecognised open_message_type: {other}"
                )))
            }
        };
        let url =
            Url::parse(&conf.url).map_err(|e| CoreError::config(format!("websocket.url: {e}")))?;
        let addrs = outbound::resolve_checked(&url, &ctx.org).await?;
        let tcp = outbound::connect_checked(&addrs, CONNECT_TIMEOUT).await?;
        let host = url
            .host_str()
            .unwrap_or_default()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        let io: Box<dyn Io> = if url.scheme() == "wss" {
            tls_connect(tcp, &host, &conf.root_cas).await?
        } else {
            Box::new(tcp)
        };

        let uri: Uri = url
            .as_str()
            .parse()
            .map_err(|_| CoreError::config("websocket.url is not a valid URI"))?;
        let mut builder = ClientBuilder::from_uri(uri)
            .limits(Limits::default().max_payload_len(Some(conf.max_message_bytes)));

        // Interpolated as Bento does, against one empty message.
        let empty = [Part::new(Bytes::new())];
        let view = PartsView(&empty);
        for (k, expr) in &conf.headers {
            let value = expr.resolve_string(0, &view).map_err(|e| {
                CoreError::config(format!(
                    "failed string interpolation on header \"{k}\": {e}"
                ))
            })?;
            let name = HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| CoreError::config(format!("bad header name '{k}'")))?;
            let val = HeaderValue::from_str(&value)
                .map_err(|_| CoreError::config(format!("bad value for header '{k}'")))?;
            builder = builder
                .add_header(name, val)
                .map_err(|e| CoreError::config(format!("header '{k}': {e}")))?;
        }
        if let Some((user, pass)) = &conf.basic_auth {
            let token = base64::engine::general_purpose::STANDARD
                .encode(format!("{user}:{}", pass.expose()));
            let val = HeaderValue::from_str(&format!("Basic {token}"))
                .map_err(|_| CoreError::config("basic_auth: invalid credentials"))?;
            builder = builder
                .add_header(axum::http::header::AUTHORIZATION, val)
                .map_err(|e| CoreError::config(format!("basic_auth: {e}")))?;
        }
        if let Some(o) = &conf.oauth {
            let nonce = uuid::Uuid::new_v4().simple().to_string();
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default();
            let header = oauth1_header(
                &url,
                &o.consumer_key,
                o.consumer_secret.expose(),
                &o.access_token,
                o.access_token_secret.expose(),
                &nonce,
                ts,
            );
            let val = HeaderValue::from_str(&header)
                .map_err(|_| CoreError::config("oauth: invalid header"))?;
            builder = builder
                .add_header(axum::http::header::AUTHORIZATION, val)
                .map_err(|e| CoreError::config(format!("oauth: {e}")))?;
        }

        let (mut ws, _resp) = tokio::time::timeout(CONNECT_TIMEOUT, builder.connect_on(io))
            .await
            .map_err(|_| CoreError::other("websocket handshake timed out"))?
            .map_err(|e| CoreError::other(format!("websocket handshake failed: {e}")))?;

        for m in &conf.open_messages {
            let msg = if msg_type_text {
                Message::text(String::from_utf8_lossy(m).into_owned())
            } else {
                Message::binary(m.clone())
            };
            ws.send(msg)
                .await
                .map_err(|e| CoreError::other(format!("websocket open message: {e}")))?;
        }
        Ok(ws)
    }

    fn ack_for(&self, batch: &Batch) -> AckResponder {
        if !self.conf.auto_replay_nacks {
            return AckResponder::noop();
        }
        let replay = self.replay.clone();
        let copy = batch.clone();
        AckResponder::from_fn(move |res| {
            if res.is_some() {
                let mut q = replay.lock().unwrap_or_else(PoisonError::into_inner);
                if q.len() < MAX_REPLAY_BATCHES {
                    q.push_back(copy);
                } else {
                    tracing::warn!("websocket: replay queue is full; a nacked message is dropped");
                }
            }
        })
    }

    fn pop_replay(&self) -> Option<Batch> {
        self.replay
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }
}

#[async_trait]
impl Input for WebsocketInput {
    fn input_type(&self) -> &'static str {
        "websocket"
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        if self.conn.is_some() {
            return Ok(());
        }
        self.closed = false;
        let ws = self.dial(ctx).await?;
        self.conn = Some(ws);
        self.backoff = BACKOFF_MIN;
        Ok(())
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        loop {
            if self.closed {
                return Err(CoreError::NotConnected);
            }
            if let Some(batch) = self.pop_replay() {
                let ack = self.ack_for(&batch);
                return Ok((batch, ack));
            }
            let shutdown = ctx.shutdown_rx.clone();
            if *shutdown.borrow() {
                return Err(CoreError::EndOfInput);
            }
            let Some(conn) = self.conn.as_mut() else {
                // The runner ends a stream on a read error, so the reconnect happens here.
                match self.dial(ctx).await {
                    Ok(ws) => {
                        self.conn = Some(ws);
                        self.backoff = BACKOFF_MIN;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "websocket: reconnect to {} failed: {e}",
                            redact_url_str(&self.conf.url)
                        );
                        let wait = self.backoff;
                        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {}
                            _ = shutdown_signal(shutdown) => return Err(CoreError::EndOfInput),
                        }
                    }
                }
                continue;
            };
            let next = tokio::select! {
                m = conn.next() => m,
                _ = shutdown_signal(shutdown) => return Err(CoreError::EndOfInput),
            };
            match next {
                Some(Ok(msg)) if msg.is_text() || msg.is_binary() => {
                    let data = Bytes::copy_from_slice(&msg.as_payload()[..]);
                    let batch = vec![Part::new(data)];
                    let ack = self.ack_for(&batch);
                    return Ok((batch, ack));
                }
                Some(Ok(msg)) if msg.is_close() => {
                    self.conn = None;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    tracing::warn!("websocket: connection lost: {e}");
                    self.conn = None;
                }
                None => {
                    self.conn = None;
                }
            }
        }
    }

    fn is_connected(&self) -> bool {
        self.conn.is_some()
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        self.closed = true;
        if let Some(mut ws) = self.conn.take() {
            let _ = tokio::time::timeout(Duration::from_secs(1), ws.close()).await;
        }
        Ok(())
    }
}
