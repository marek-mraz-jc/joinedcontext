//! `mqtt` input (Bento 1.22 `internal/impl/mqtt/input.go`, paho, MQTT 3.1.1): subscribes to
//! `topics` at `qos` and reads every PUBLISH as a one-part batch with the metadata
//! `mqtt_duplicate`, `mqtt_qos`, `mqtt_retained`, `mqtt_topic` and `mqtt_message_id`.
//!
//! Fields: `urls` (the first that connects; schemes `tcp`/`mqtt`, `ssl`/`tls`/`mqtts`/`tcps`
//! for TLS, `ws`, `wss`), `client_id`, `dynamic_client_id_suffix` (`nanoid`), `user`,
//! `password`, `qos` (0-2), `clean_session` (default true), `keepalive` (seconds, default 30),
//! `connect_timeout` (default 30s), `tls` (`enabled`, `root_cas`).
//!
//! As the `websocket` input: every connection follows the outbound rule (`crate::outbound`) and
//! TLS is ekiben's over that connection; `tls.skip_cert_verify`, `tls.root_cas_file`,
//! `tls.client_certs` and `will` are refused. A QoS 1 message is acknowledged (PUBACK) when its
//! batch is acked; a nacked one is not, so the broker sends it again after a reconnect of a
//! persistent session. A lost connection reconnects inside `read_batch` (1 s to 30 s backoff).

use std::time::Duration;

use async_trait::async_trait;
use axum::http::{HeaderValue, Uri};
use bytes::Bytes;
use ekiben_bloblang::Value;
use ekiben_core::{
    redact_url_str, AckResponder, Batch, ComponentConfig, CoreError, Input, Part, Registry,
    SecretString, StreamContext,
};
use futures::{SinkExt, StreamExt};
use serde_json::Value as JsonValue;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_websockets::{ClientBuilder, Limits, Message, WebSocketStream};
use url::Url;

use crate::http_server::shutdown_signal;
use crate::outbound;
use crate::websocket::{pem_certificates, tls_connect, Io};

const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// Largest MQTT packet read (MQTT's own limit is 256 MiB).
pub const MAX_PACKET_BYTES: usize = 16 * 1024 * 1024;
/// Messages read ahead of the pipeline.
const READ_AHEAD: usize = 1024;

pub fn register(registry: &Registry) {
    registry.register_input("mqtt", |conf, _ctx| {
        Ok(Box::new(MqttInput::from_config(conf)?))
    });
}

/// The parsed config of one `mqtt` input.
pub struct MqttConf {
    pub urls: Vec<String>,
    pub client_id: String,
    pub user: String,
    password: SecretString,
    pub topics: Vec<String>,
    pub qos: u8,
    pub clean_session: bool,
    pub keepalive: u16,
    pub connect_timeout: Duration,
    root_cas: Vec<Vec<u8>>,
}

impl MqttConf {
    pub fn from_json(val: &JsonValue) -> Result<Self, CoreError> {
        let map = val
            .as_object()
            .ok_or_else(|| CoreError::config("mqtt: expected an object"))?;
        let str_of = |k: &str| {
            map.get(k)
                .and_then(JsonValue::as_str)
                .unwrap_or("")
                .to_string()
        };
        let mut urls = Vec::new();
        for u in map
            .get("urls")
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
        {
            // Bento splits each entry on commas.
            for part in u.as_str().unwrap_or("").split(',') {
                let part = part.trim();
                if !part.is_empty() {
                    urls.push(part.to_string());
                }
            }
        }
        if urls.is_empty() {
            return Err(CoreError::config("mqtt: at least one url is required"));
        }
        for u in &urls {
            let parsed = Url::parse(u)
                .map_err(|e| CoreError::config(format!("mqtt.urls: {}: {e}", redact_url_str(u))))?;
            scheme_of(parsed.scheme())?;
        }
        let topics: Vec<String> = map
            .get("topics")
            .and_then(JsonValue::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(JsonValue::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if topics.is_empty() || topics.iter().any(|t| t.is_empty() || t.len() > 65535) {
            return Err(CoreError::config(
                "mqtt: topics must name at least one non-empty topic",
            ));
        }
        let qos = map.get("qos").and_then(JsonValue::as_u64).unwrap_or(1);
        if qos > 2 {
            return Err(CoreError::config("mqtt: qos must be 0, 1 or 2"));
        }
        if map
            .get("will")
            .and_then(|w| w.get("enabled"))
            .and_then(JsonValue::as_bool)
            == Some(true)
        {
            return Err(CoreError::config("mqtt.will is not supported by ekiben"));
        }
        let mut client_id = str_of("client_id");
        match str_of("dynamic_client_id_suffix").as_str() {
            "" => {}
            "nanoid" => {
                client_id.push_str(&uuid::Uuid::new_v4().simple().to_string()[..21]);
            }
            other => {
                return Err(CoreError::config(format!(
                    "mqtt: unknown dynamic_client_id_suffix: {other}"
                )))
            }
        }
        let keepalive = map
            .get("keepalive")
            .and_then(JsonValue::as_u64)
            .unwrap_or(30);
        let connect_timeout = match map.get("connect_timeout").and_then(JsonValue::as_str) {
            Some(t) => ekiben_core::config::parse_duration_str(t)?,
            None => Duration::from_secs(30),
        };
        let mut root_cas = Vec::new();
        if let Some(tls) = map.get("tls").and_then(JsonValue::as_object) {
            for refused in ["skip_cert_verify", "root_cas_file", "client_certs"] {
                let set = match tls.get(refused) {
                    Some(JsonValue::Bool(b)) => *b,
                    Some(JsonValue::String(s)) => !s.is_empty(),
                    Some(JsonValue::Array(a)) => !a.is_empty(),
                    _ => false,
                };
                if set {
                    return Err(CoreError::config(format!(
                        "mqtt.tls.{refused} is not supported by ekiben"
                    )));
                }
            }
            if let Some(pem) = tls
                .get("root_cas")
                .and_then(JsonValue::as_str)
                .filter(|s| !s.is_empty())
            {
                root_cas = pem_certificates(pem)?;
            }
        }
        Ok(Self {
            urls,
            client_id,
            user: str_of("user"),
            password: SecretString::new(str_of("password")),
            topics,
            qos: qos as u8,
            clean_session: map
                .get("clean_session")
                .and_then(JsonValue::as_bool)
                .unwrap_or(true),
            keepalive: keepalive.min(u16::MAX as u64) as u16,
            connect_timeout,
            root_cas,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Transport {
    Tcp,
    Tls,
    Ws,
    Wss,
}

fn scheme_of(scheme: &str) -> Result<Transport, CoreError> {
    match scheme {
        "tcp" | "mqtt" => Ok(Transport::Tcp),
        "ssl" | "tls" | "mqtts" | "tcps" => Ok(Transport::Tls),
        "ws" => Ok(Transport::Ws),
        "wss" => Ok(Transport::Wss),
        other => Err(CoreError::config(format!(
            "mqtt: unsupported url scheme '{other}'"
        ))),
    }
}

// ---- packets (MQTT 3.1.1) ----

fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s);
}

fn packet(first: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![first];
    let mut len = body.len();
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }
    out.extend_from_slice(body);
    out
}

fn connect_packet(conf: &MqttConf) -> Vec<u8> {
    let mut body = Vec::new();
    put_str(&mut body, b"MQTT");
    body.push(4);
    let mut flags = 0u8;
    if conf.clean_session {
        flags |= 0x02;
    }
    if !conf.user.is_empty() {
        flags |= 0x80;
        if !conf.password.expose().is_empty() {
            flags |= 0x40;
        }
    }
    body.push(flags);
    body.extend_from_slice(&conf.keepalive.to_be_bytes());
    put_str(&mut body, conf.client_id.as_bytes());
    if !conf.user.is_empty() {
        put_str(&mut body, conf.user.as_bytes());
        if !conf.password.expose().is_empty() {
            put_str(&mut body, conf.password.expose().as_bytes());
        }
    }
    packet(0x10, &body)
}

fn subscribe_packet(conf: &MqttConf, id: u16) -> Vec<u8> {
    let mut body = id.to_be_bytes().to_vec();
    for t in &conf.topics {
        put_str(&mut body, t.as_bytes());
        body.push(conf.qos);
    }
    packet(0x82, &body)
}

/// One whole packet off the front of `buf`: its first byte and body, or `None` when more bytes
/// are needed.
fn take_packet(buf: &mut Vec<u8>) -> Result<Option<(u8, Vec<u8>)>, CoreError> {
    let mut len = 0usize;
    let mut mult = 1usize;
    let mut i = 1;
    loop {
        let Some(&b) = buf.get(i) else {
            return Ok(None);
        };
        len += (b & 0x7f) as usize * mult;
        i += 1;
        if b & 0x80 == 0 {
            break;
        }
        mult *= 128;
        if i > 4 {
            return Err(CoreError::other("mqtt: malformed remaining length"));
        }
    }
    if len > MAX_PACKET_BYTES {
        return Err(CoreError::other(format!(
            "mqtt: a packet of {len} bytes is over the limit"
        )));
    }
    if buf.len() < i + len {
        return Ok(None);
    }
    let first = buf[0];
    let body = buf[i..i + len].to_vec();
    buf.drain(..i + len);
    Ok(Some((first, body)))
}

/// A received PUBLISH.
#[derive(Debug, PartialEq)]
struct Publish {
    topic: String,
    qos: u8,
    dup: bool,
    retain: bool,
    id: u16,
    payload: Vec<u8>,
}

fn parse_publish(first: u8, body: &[u8]) -> Result<Publish, CoreError> {
    let qos = (first >> 1) & 0x03;
    let bad = || CoreError::other("mqtt: malformed PUBLISH");
    let tlen = u16::from_be_bytes([
        *body.first().ok_or_else(bad)?,
        *body.get(1).ok_or_else(bad)?,
    ]) as usize;
    let topic = std::str::from_utf8(body.get(2..2 + tlen).ok_or_else(bad)?)
        .map_err(|_| bad())?
        .to_string();
    let mut at = 2 + tlen;
    let mut id = 0;
    if qos > 0 {
        let b = body.get(at..at + 2).ok_or_else(bad)?;
        id = u16::from_be_bytes([b[0], b[1]]);
        at += 2;
    }
    Ok(Publish {
        topic,
        qos,
        dup: first & 0x08 != 0,
        retain: first & 0x01 != 0,
        id,
        payload: body.get(at..).unwrap_or_default().to_vec(),
    })
}

// ---- the connection ----

enum Wire {
    Stream(Box<dyn Io>),
    Ws(Box<WebSocketStream<Box<dyn Io>>>),
}

impl Wire {
    async fn send(&mut self, bytes: Vec<u8>) -> Result<(), CoreError> {
        let err = |e: String| CoreError::other(format!("mqtt: write failed: {e}"));
        match self {
            Wire::Stream(s) => {
                s.write_all(&bytes).await.map_err(|e| err(e.to_string()))?;
                s.flush().await.map_err(|e| err(e.to_string()))
            }
            Wire::Ws(ws) => ws
                .send(Message::binary(bytes))
                .await
                .map_err(|e| err(e.to_string())),
        }
    }

    /// Appends what arrives next to `buf`; `Ok(false)` when the peer closed.
    async fn recv(&mut self, buf: &mut Vec<u8>) -> Result<bool, CoreError> {
        match self {
            Wire::Stream(s) => {
                let mut chunk = [0u8; 16 * 1024];
                let n = s
                    .read(&mut chunk)
                    .await
                    .map_err(|e| CoreError::other(format!("mqtt: read failed: {e}")))?;
                buf.extend_from_slice(&chunk[..n]);
                Ok(n > 0)
            }
            Wire::Ws(ws) => loop {
                match ws.next().await {
                    Some(Ok(m)) if m.is_binary() || m.is_text() => {
                        buf.extend_from_slice(&m.as_payload()[..]);
                        return Ok(true);
                    }
                    Some(Ok(m)) if m.is_close() => return Ok(false),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        return Err(CoreError::other(format!("mqtt: read failed: {e}")))
                    }
                    None => return Ok(false),
                }
            },
        }
    }
}

async fn dial(
    conf: &MqttConf,
    url: &str,
    ctx: &StreamContext,
) -> Result<(Wire, Vec<u8>), CoreError> {
    let parsed = Url::parse(url).map_err(|e| CoreError::config(format!("mqtt.urls: {e}")))?;
    let transport = scheme_of(parsed.scheme())?;
    let host = parsed
        .host_str()
        .ok_or_else(|| CoreError::config("mqtt: a url has no host"))?
        .to_string();
    // The outbound rule reads http(s)/ws(s) addresses: check the same host and port that way.
    let port = parsed.port().unwrap_or(match transport {
        Transport::Tcp => 1883,
        Transport::Tls => 8883,
        Transport::Ws => 80,
        Transport::Wss => 443,
    });
    let probe = Url::parse(&format!("http://{host}:{port}/"))
        .map_err(|e| CoreError::config(format!("mqtt.urls: {e}")))?;
    let addrs = outbound::resolve_checked(&probe, &ctx.org).await?;
    let tcp = outbound::connect_checked(&addrs, conf.connect_timeout).await?;
    let bare = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let io: Box<dyn Io> = match transport {
        Transport::Tls | Transport::Wss => tls_connect(tcp, &bare, &conf.root_cas).await?,
        _ => Box::new(tcp),
    };
    let mut wire = match transport {
        Transport::Tcp | Transport::Tls => Wire::Stream(io),
        Transport::Ws | Transport::Wss => {
            let uri: Uri = parsed
                .as_str()
                .parse()
                .map_err(|_| CoreError::config("mqtt: a url is not a valid URI"))?;
            let builder = ClientBuilder::from_uri(uri)
                .limits(Limits::default().max_payload_len(Some(MAX_PACKET_BYTES)))
                .add_header(
                    axum::http::header::SEC_WEBSOCKET_PROTOCOL,
                    HeaderValue::from_static("mqtt"),
                )
                .map_err(|e| CoreError::config(format!("mqtt: {e}")))?;
            let (ws, _) = tokio::time::timeout(conf.connect_timeout, builder.connect_on(io))
                .await
                .map_err(|_| CoreError::other("mqtt: websocket handshake timed out"))?
                .map_err(|e| CoreError::other(format!("mqtt: websocket handshake failed: {e}")))?;
            Wire::Ws(Box::new(ws))
        }
    };
    let mut buf = Vec::new();
    let handshake = async {
        wire.send(connect_packet(conf)).await?;
        let connack = next_packet(&mut wire, &mut buf).await?;
        if connack.0 >> 4 != 2 || connack.1.len() < 2 {
            return Err(CoreError::other(
                "mqtt: the broker did not answer CONNECT with CONNACK",
            ));
        }
        if connack.1[1] != 0 {
            return Err(CoreError::other(format!(
                "mqtt: the broker refused the connection (code {})",
                connack.1[1]
            )));
        }
        wire.send(subscribe_packet(conf, 1)).await?;
        // A retained PUBLISH may arrive before the SUBACK: it is kept, in order, for the reader.
        let mut early = Vec::new();
        loop {
            let (first, body) = next_packet(&mut wire, &mut buf).await?;
            match first >> 4 {
                9 => {
                    if body.iter().skip(2).any(|&c| c == 0x80) {
                        return Err(CoreError::other("mqtt: the broker refused a subscription"));
                    }
                    early.append(&mut buf);
                    return Ok(early);
                }
                3 => early.extend(packet(first, &body)),
                _ => {}
            }
        }
    };
    let early = tokio::time::timeout(conf.connect_timeout, handshake)
        .await
        .map_err(|_| CoreError::other("mqtt: connect timed out"))??;
    Ok((wire, early))
}

async fn next_packet(wire: &mut Wire, buf: &mut Vec<u8>) -> Result<(u8, Vec<u8>), CoreError> {
    loop {
        if let Some(p) = take_packet(buf)? {
            return Ok(p);
        }
        if !wire.recv(buf).await? {
            return Err(CoreError::other("mqtt: the broker closed the connection"));
        }
    }
}

enum Event {
    Message(Publish),
    Lost(String),
}

/// Runs one connection: keepalive pings, acknowledgements, and every PUBLISH into `events`.
async fn run_connection(
    mut wire: Wire,
    mut buf: Vec<u8>,
    keepalive: u16,
    events: mpsc::Sender<Event>,
    mut acks: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let ping_every = Duration::from_secs(u64::from(keepalive.max(1)));
    let mut ticker = tokio::time::interval(ping_every);
    ticker.tick().await;
    let reason = loop {
        let next = match take_packet(&mut buf) {
            Ok(next) => next,
            Err(e) => break e.to_string(),
        };
        if let Some((first, body)) = next {
            match first >> 4 {
                3 => match parse_publish(first, &body) {
                    Ok(p) => {
                        if p.qos == 2 {
                            let pubrec = packet(0x50, &p.id.to_be_bytes());
                            if let Err(e) = wire.send(pubrec).await {
                                break e.to_string();
                            }
                        }
                        if events.send(Event::Message(p)).await.is_err() {
                            break String::new();
                        }
                    }
                    Err(e) => break e.to_string(),
                },
                // PUBREL: the QoS 2 exchange ends with PUBCOMP.
                6 => {
                    if let Err(e) = wire
                        .send(packet(0x70, body.get(..2).unwrap_or(&[0, 0])))
                        .await
                    {
                        break e.to_string();
                    }
                }
                _ => {}
            }
            continue;
        }
        tokio::select! {
            got = wire.recv(&mut buf) => match got {
                Ok(true) => {}
                Ok(false) => break "the broker closed the connection".to_string(),
                Err(e) => break e.to_string(),
            },
            ack = acks.recv() => match ack {
                Some(bytes) => if let Err(e) = wire.send(bytes).await { break e.to_string() },
                None => {
                    let _ = wire.send(vec![0xe0, 0x00]).await;
                    break String::new();
                }
            },
            _ = ticker.tick() => {
                if let Err(e) = wire.send(vec![0xc0, 0x00]).await {
                    break e.to_string();
                }
            }
        }
    };
    if !reason.is_empty() {
        let _ = events.send(Event::Lost(reason)).await;
    }
}

/// The `mqtt` input.
pub struct MqttInput {
    conf: MqttConf,
    label: String,
    events: Option<mpsc::Receiver<Event>>,
    acks: Option<mpsc::UnboundedSender<Vec<u8>>>,
    backoff: Duration,
    closed: bool,
}

impl MqttInput {
    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        Ok(Self {
            conf: MqttConf::from_json(&conf.config)?,
            label: conf.label.clone(),
            events: None,
            acks: None,
            backoff: BACKOFF_MIN,
            closed: false,
        })
    }

    async fn open(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        let mut last = CoreError::other("mqtt: no url");
        for url in &self.conf.urls {
            match dial(&self.conf, url, ctx).await {
                Ok((wire, early)) => {
                    let (etx, erx) = mpsc::channel(READ_AHEAD);
                    let (atx, arx) = mpsc::unbounded_channel();
                    tokio::spawn(run_connection(wire, early, self.conf.keepalive, etx, arx));
                    self.events = Some(erx);
                    self.acks = Some(atx);
                    self.backoff = BACKOFF_MIN;
                    return Ok(());
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

#[async_trait]
impl Input for MqttInput {
    fn input_type(&self) -> &'static str {
        "mqtt"
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        if self.events.is_some() {
            return Ok(());
        }
        self.closed = false;
        self.open(ctx).await
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        loop {
            if self.closed {
                return Err(CoreError::NotConnected);
            }
            let shutdown = ctx.shutdown_rx.clone();
            if *shutdown.borrow() {
                return Err(CoreError::EndOfInput);
            }
            let Some(events) = self.events.as_mut() else {
                if let Err(e) = self.open(ctx).await {
                    tracing::warn!("mqtt: reconnect failed: {e}");
                    let wait = self.backoff;
                    self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = shutdown_signal(shutdown) => return Err(CoreError::EndOfInput),
                    }
                }
                continue;
            };
            let next = tokio::select! {
                e = events.recv() => e,
                _ = shutdown_signal(shutdown) => return Err(CoreError::EndOfInput),
            };
            match next {
                Some(Event::Message(p)) => {
                    let mut part = Part::new(Bytes::from(p.payload));
                    part.meta_set("mqtt_duplicate", Value::Bool(p.dup));
                    part.meta_set("mqtt_qos", Value::Int(i64::from(p.qos)));
                    part.meta_set("mqtt_retained", Value::Bool(p.retain));
                    part.meta_set("mqtt_topic", Value::String(p.topic));
                    part.meta_set("mqtt_message_id", Value::Int(i64::from(p.id)));
                    let ack = match (p.qos, self.acks.clone()) {
                        (1, Some(acks)) => {
                            let puback = packet(0x40, &p.id.to_be_bytes());
                            AckResponder::from_fn(move |res| {
                                if res.is_none() {
                                    let _ = acks.send(puback);
                                }
                            })
                        }
                        _ => AckResponder::noop(),
                    };
                    return Ok((vec![part], ack));
                }
                Some(Event::Lost(why)) => {
                    tracing::warn!("mqtt: connection lost: {why}");
                    self.events = None;
                    self.acks = None;
                }
                None => {
                    self.events = None;
                    self.acks = None;
                }
            }
        }
    }

    fn is_connected(&self) -> bool {
        self.events.is_some()
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        self.closed = true;
        // Dropping the ack sender makes the connection send DISCONNECT and end.
        self.acks = None;
        self.events = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn packets_round_trip() {
        let conf = MqttConf::from_json(&json!({
            "urls": ["tcp://broker:1883"], "client_id": "c", "user": "u", "password": "p",
            "topics": ["a/+/b"], "qos": 1
        }))
        .unwrap();
        let connect = connect_packet(&conf);
        assert_eq!(connect[0], 0x10);
        assert_eq!(&connect[2..8], b"\x00\x04MQTT");
        assert_eq!(connect[9], 0x80 | 0x40 | 0x02);
        let mut buf = subscribe_packet(&conf, 7);
        buf.extend_from_slice(&[0x30]);
        let (first, body) = take_packet(&mut buf).unwrap().unwrap();
        assert_eq!(first, 0x82);
        assert_eq!(&body[..2], &[0, 7]);
        assert_eq!(buf, vec![0x30], "the next packet's partial byte stays");
        assert!(take_packet(&mut buf).unwrap().is_none());

        let mut body = Vec::new();
        put_str(&mut body, b"t/1");
        body.extend_from_slice(&5u16.to_be_bytes());
        body.extend_from_slice(b"hello");
        let publish = parse_publish(0x3b, &body).unwrap();
        assert_eq!(
            publish,
            Publish {
                topic: "t/1".into(),
                qos: 1,
                dup: true,
                retain: true,
                id: 5,
                payload: b"hello".to_vec()
            }
        );
        let long = packet(0x30, &vec![0u8; 300]);
        assert_eq!(&long[1..3], &[0xac, 0x02]);
    }

    #[test]
    fn a_config_ekiben_cannot_honour_is_refused() {
        for bad in [
            json!({"urls": [], "topics": ["a"]}),
            json!({"urls": ["udp://x:1"], "topics": ["a"]}),
            json!({"urls": ["tcp://x:1"], "topics": []}),
            json!({"urls": ["tcp://x:1"], "topics": ["a"], "qos": 3}),
            json!({"urls": ["tcp://x:1"], "topics": ["a"], "tls": {"skip_cert_verify": true}}),
            json!({"urls": ["tcp://x:1"], "topics": ["a"], "will": {"enabled": true}}),
        ] {
            assert!(MqttConf::from_json(&bad).is_err(), "{bad}");
        }
        let ok = MqttConf::from_json(
            &json!({"urls": ["wss://mqtt.hsl.fi:443/"], "topics": ["/hfp/#"], "qos": 0}),
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn an_oversized_length_is_refused() {
        let mut buf = vec![0x30, 0xff, 0xff, 0xff, 0x7f];
        assert!(take_packet(&mut buf).is_err());
        let mut buf = vec![0x30, 0xff, 0xff, 0xff, 0xff, 0x01];
        assert!(take_packet(&mut buf).is_err());
    }
}
