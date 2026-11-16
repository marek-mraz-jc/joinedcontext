#![forbid(unsafe_code)]
//! `ekiben-io-amqp`: Bento's `amqp_0_9` input (`internal/impl/amqp09/input.go`) on lapin.
//!
//! Each URL's broker goes through the organization's outbound rule (ekiben-io-http
//! `outbound::resolve_host_checked`) and the connection is opened to the checked address, never
//! re-resolved. TLS (`amqps://` or `tls.enabled`) always verifies the broker's certificate
//! against the system roots plus `tls.root_cas`; `skip_cert_verify`, renegotiation, client
//! certificates and file paths are refused. The user and password ride in the URL, which an
//! author interpolates from a secret: ekiben keeps the URLs as `SecretString` and names a URL by
//! its index in every error. The input is resident: a lost connection is dialled again with
//! backoff (100 ms to 30 s).
//!
//! Each delivery is one message, acked to the broker once the pipeline acks it; a failed message
//! is nacked and requeued, or rejected (dropped, or dead-lettered by the broker) when its error
//! matches one of `nack_reject_patterns`. `auto_ack` acks on receipt.
//! ponytail: lapin reads a whole message before ekiben sees its size; the broker's own
//! `max_message_size` (RabbitMQ: 16 MiB since 4.0) bounds it, and a message over
//! [`MAX_MESSAGE_BYTES`] is rejected unread by the pipeline.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use ekiben_core::redact::SecretString;
use ekiben_core::{
    lint_fields, AckResponder, Batch, ComponentConfig, CoreError, Input, Part, Registry,
    StreamContext,
};
use futures::StreamExt;
use lapin::message::Delivery;
use lapin::options::{
    BasicAckOptions, BasicConsumeOptions, BasicNackOptions, BasicQosOptions, BasicRejectOptions,
    QueueBindOptions, QueueDeclareOptions,
};
use lapin::tcp::{AsyncTcpStream, TLSConfig};
use lapin::types::{AMQPValue, FieldTable};
use lapin::uri::{AMQPScheme, AMQPUri};
use lapin::{Connection, ConnectionProperties, Consumer};
use regex::Regex;
use serde_json::Value as JsonValue;

/// The largest message the input hands to the pipeline.
pub const MAX_MESSAGE_BYTES: usize = 64 << 20;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `read_batch` waits for a delivery before it lets the runner look at shutdown.
const READ_WAIT: Duration = Duration::from_secs(1);

const FIELDS: &[&str] = &[
    "urls",
    "queue",
    "queue_declare",
    "bindings_declare",
    "consumer_tag",
    "auto_ack",
    "nack_reject_patterns",
    "prefetch_count",
    "prefetch_size",
    "tls",
];
const TLS_FIELDS: &[&str] = &[
    "enabled",
    "skip_cert_verify",
    "enable_renegotiation",
    "root_cas",
    "root_cas_file",
    "client_certs",
];

pub fn register_all(registry: &Registry) {
    registry.register_input("amqp_0_9", |conf, _ctx| {
        Ok(Box::new(AmqpInput::from_config(conf)?))
    });
    registry.register_lint("input", "amqp_0_9", lint);
}

pub fn lint(conf: &JsonValue) -> Vec<String> {
    match parse(conf) {
        Ok(_) => Vec::new(),
        Err(e) => vec![e],
    }
}

struct Settings {
    urls: Vec<SecretString>,
    queue: String,
    /// `(durable, auto_delete)` when `queue_declare.enabled`.
    declare: Option<(bool, bool)>,
    bindings: Vec<(String, String)>,
    consumer_tag: String,
    auto_ack: bool,
    reject: Vec<Regex>,
    prefetch_count: u16,
    tls: bool,
    root_cas: Option<String>,
}

fn str_field<'a>(obj: &'a serde_json::Map<String, JsonValue>, k: &str) -> &'a str {
    obj.get(k).and_then(JsonValue::as_str).unwrap_or("")
}

fn bool_field(obj: &serde_json::Map<String, JsonValue>, k: &str, default: bool) -> bool {
    obj.get(k).and_then(JsonValue::as_bool).unwrap_or(default)
}

fn uint<T: TryFrom<u64>>(
    obj: &serde_json::Map<String, JsonValue>,
    k: &str,
    default: T,
) -> Result<T, String> {
    match obj.get(k) {
        None | Some(JsonValue::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|n| T::try_from(n).ok())
            .ok_or_else(|| format!("amqp_0_9 input: {k} must be a whole number, e.g. 10")),
    }
}

fn parse(conf: &JsonValue) -> Result<Settings, String> {
    let errs = lint_fields(conf, FIELDS, &["urls", "queue"]);
    if !errs.is_empty() {
        return Err(format!("amqp_0_9 input: {}", errs.join("; ")));
    }
    let obj = conf
        .as_object()
        .ok_or("amqp_0_9 input: expected an object")?;
    let mut urls = Vec::new();
    for (i, u) in obj
        .get("urls")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let u = u
            .as_str()
            .ok_or_else(|| format!("amqp_0_9 input: urls[{i}] must be a string"))?;
        // Bento expands a comma-separated item into several URLs.
        for part in u.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            part.parse::<AMQPUri>().map_err(|_| {
                format!(
                    "amqp_0_9 input: urls[{i}] is not an AMQP URL; expected amqp://user:password@host:5672/vhost"
                )
            })?;
            urls.push(SecretString::new(part));
        }
    }
    if urls.is_empty() {
        return Err("amqp_0_9 input: urls needs at least one URL, e.g. amqp://user:${AMQP_PASSWORD}@rabbitmq.example.org:5672/".into());
    }
    let queue = str_field(obj, "queue").to_string();
    if queue.is_empty() {
        return Err("amqp_0_9 input: queue must name the queue to consume, e.g. sensors".into());
    }
    let declare = obj
        .get("queue_declare")
        .and_then(JsonValue::as_object)
        .filter(|d| bool_field(d, "enabled", false))
        .map(|d| {
            (
                bool_field(d, "durable", true),
                bool_field(d, "auto_delete", false),
            )
        });
    let mut bindings = Vec::new();
    for (i, b) in obj
        .get("bindings_declare")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let b = b
            .as_object()
            .ok_or_else(|| format!("amqp_0_9 input: bindings_declare[{i}] must be an object"))?;
        let exchange = str_field(b, "exchange");
        if exchange.is_empty() {
            return Err(format!(
                "amqp_0_9 input: bindings_declare[{i}].exchange must name an exchange, e.g. amq.topic"
            ));
        }
        bindings.push((exchange.to_string(), str_field(b, "key").to_string()));
    }
    let mut reject = Vec::new();
    for (i, p) in obj
        .get("nack_reject_patterns")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let re = p
            .as_str()
            .and_then(|p| regex::RegexBuilder::new(p).size_limit(1 << 20).build().ok())
            .ok_or_else(|| {
                format!("amqp_0_9 input: nack_reject_patterns[{i}] is not a regular expression, e.g. ^invalid")
            })?;
        reject.push(re);
    }
    if uint(obj, "prefetch_size", 0u32)? != 0 {
        return Err("amqp_0_9 input: prefetch_size is not supported (RabbitMQ does not implement it); bound the pending messages with prefetch_count".into());
    }
    let (tls, root_cas) = match obj.get("tls").and_then(JsonValue::as_object) {
        None => (false, None),
        Some(t) => {
            let unknown: Vec<&String> = t
                .keys()
                .filter(|k| !TLS_FIELDS.contains(&k.as_str()))
                .collect();
            if let Some(k) = unknown.first() {
                return Err(format!("amqp_0_9 input: tls.{k} is not a field of tls"));
            }
            if bool_field(t, "skip_cert_verify", false) {
                return Err("amqp_0_9 input: tls.skip_cert_verify: true is refused: ekiben always verifies the broker's certificate; give its CA in tls.root_cas".into());
            }
            if bool_field(t, "enable_renegotiation", false) {
                return Err("amqp_0_9 input: tls.enable_renegotiation: true is refused: TLS renegotiation is not supported".into());
            }
            if !str_field(t, "root_cas_file").is_empty() {
                return Err("amqp_0_9 input: tls.root_cas_file is not supported by ekiben; give the CA's PEM in tls.root_cas".into());
            }
            if t.get("client_certs")
                .and_then(JsonValue::as_array)
                .is_some_and(|c| !c.is_empty())
            {
                return Err("amqp_0_9 input: tls.client_certs is not supported by ekiben; authenticate with the URL's user and password from a secret".into());
            }
            let cas = str_field(t, "root_cas");
            (
                bool_field(t, "enabled", false),
                (!cas.is_empty()).then(|| cas.to_string()),
            )
        }
    };
    Ok(Settings {
        urls,
        queue,
        declare,
        bindings,
        consumer_tag: str_field(obj, "consumer_tag").to_string(),
        auto_ack: bool_field(obj, "auto_ack", false),
        reject,
        prefetch_count: uint(obj, "prefetch_count", 10u16)?,
        tls,
        root_cas,
    })
}

struct Live {
    conn: Connection,
    consumer: Consumer,
}

pub struct AmqpInput {
    s: Settings,
    live: Option<Live>,
    backoff: Duration,
    label: String,
}

impl AmqpInput {
    pub fn from_config(conf: &ComponentConfig) -> Result<Self, CoreError> {
        Ok(Self {
            s: parse(&conf.config).map_err(CoreError::config)?,
            live: None,
            backoff: Duration::from_millis(100),
            label: conf.label.clone(),
        })
    }

    async fn dial(&self, ctx: &StreamContext) -> Result<Live, CoreError> {
        let mut last = CoreError::other("amqp_0_9 input: no URL");
        for (i, url) in self.s.urls.iter().enumerate() {
            match self.dial_one(url, ctx).await {
                Ok(l) => return Ok(l),
                Err(e @ (CoreError::AccessDenied { .. } | CoreError::Config(_))) => return Err(e),
                Err(e) => {
                    tracing::warn!("amqp_0_9 input: urls[{i}]: {e}");
                    last = e;
                }
            }
        }
        Err(last)
    }

    async fn dial_one(&self, url: &SecretString, ctx: &StreamContext) -> Result<Live, CoreError> {
        let mut uri: AMQPUri = url
            .expose()
            .parse()
            .map_err(|_| CoreError::config("amqp_0_9 input: a URL does not parse"))?;
        let addrs = ekiben_io_http::outbound::resolve_host_checked(
            &uri.authority.host,
            uri.authority.port,
            &ctx.org,
        )
        .await?;
        let tls = self.s.tls || uri.scheme == AMQPScheme::AMQPS;
        if tls {
            uri.scheme = AMQPScheme::AMQPS;
        }
        let failed =
            |what: &str, e: lapin::Error| CoreError::other(format!("amqp_0_9 input: {what}: {e}"));
        let runtime = lapin::runtime::default_runtime().map_err(|e| failed("runtime", e))?;
        let root_cas = self.s.root_cas.clone();
        // Opens the TCP connection to the checked addresses and, with TLS, verifies the
        // broker's certificate for the URL's host.
        let connect = async move |uri: AMQPUri, runtime| {
            let stream = AsyncTcpStream::connect(&runtime, addrs.clone()).await?;
            if !tls {
                return Ok(stream);
            }
            let config = TLSConfig {
                identity: None,
                cert_chain: root_cas.as_deref(),
            };
            match stream.into_tls(&uri.authority.host, config).await? {
                AsyncTcpStream::Plain(_) => Err(lapin::Error::from(std::io::Error::other(
                    "this build has no TLS backend; refusing to talk to the broker in the clear",
                ))),
                s => Ok(s),
            }
        };
        let conn = tokio::time::timeout(
            CONNECT_TIMEOUT,
            Connection::connector(uri, runtime, connect, ConnectionProperties::default()),
        )
        .await
        .map_err(|_| {
            CoreError::other("amqp_0_9 input: the broker did not complete the handshake in 10 s")
        })?
        .map_err(|e| failed("connect", e))?;
        let channel = conn
            .create_channel()
            .await
            .map_err(|e| failed("channel", e))?;
        channel
            .basic_qos(self.s.prefetch_count, BasicQosOptions::default())
            .await
            .map_err(|e| failed("qos", e))?;
        if let Some((durable, auto_delete)) = self.s.declare {
            channel
                .queue_declare(
                    self.s.queue.clone().into(),
                    QueueDeclareOptions {
                        durable,
                        auto_delete,
                        ..QueueDeclareOptions::default()
                    },
                    FieldTable::default(),
                )
                .await
                .map_err(|e| failed("queue declare", e))?;
        }
        for (exchange, key) in &self.s.bindings {
            channel
                .queue_bind(
                    self.s.queue.clone().into(),
                    exchange.clone().into(),
                    key.clone().into(),
                    QueueBindOptions::default(),
                    FieldTable::default(),
                )
                .await
                .map_err(|e| failed("queue bind", e))?;
        }
        let consumer = channel
            .basic_consume(
                self.s.queue.clone().into(),
                self.s.consumer_tag.clone().into(),
                BasicConsumeOptions {
                    no_ack: self.s.auto_ack,
                    ..BasicConsumeOptions::default()
                },
                FieldTable::default(),
            )
            .await
            .map_err(|e| failed("consume", e))?;
        Ok(Live { conn, consumer })
    }

    fn to_batch(&self, d: Delivery, consumer_tag: &str) -> (Batch, AckResponder) {
        let acker = d.acker.clone();
        let ack = if self.s.auto_ack {
            AckResponder::noop()
        } else {
            let reject = self.s.reject.clone();
            AckResponder::from_fn(move |err| {
                let Ok(rt) = tokio::runtime::Handle::try_current() else {
                    return;
                };
                rt.spawn(async move {
                    let sent = match err {
                        None => acker.ack(BasicAckOptions::default()).await,
                        Some(e) if reject.iter().any(|r| r.is_match(&e.message)) => {
                            acker.reject(BasicRejectOptions { requeue: false }).await
                        }
                        Some(_) => {
                            acker
                                .nack(BasicNackOptions {
                                    multiple: false,
                                    requeue: true,
                                })
                                .await
                        }
                    };
                    if let Err(e) = sent {
                        tracing::warn!("amqp_0_9 input: acknowledging a message failed: {e}");
                    }
                });
            })
        };
        let mut part = Part::new(d.data);
        for (k, v) in metadata(&d.properties) {
            part.meta_set_str(k, v);
        }
        part.meta_set_str("amqp_consumer_tag", consumer_tag.to_string());
        part.meta_set_str("amqp_delivery_tag", d.delivery_tag.to_string());
        part.meta_set_str("amqp_redelivered", d.redelivered.to_string());
        part.meta_set_str("amqp_exchange", d.exchange.to_string());
        part.meta_set_str("amqp_routing_key", d.routing_key.to_string());
        (vec![part], ack)
    }
}

/// Bento's metadata of a delivery's properties and headers.
fn metadata(p: &lapin::BasicProperties) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let mut put = |k: &str, v: Option<String>| {
        if let Some(v) = v {
            m.insert(k.to_string(), v);
        }
    };
    fn text<T: ToString>(v: &Option<T>) -> Option<String> {
        v.as_ref().map(ToString::to_string)
    }
    put("amqp_content_type", text(p.content_type()));
    put("amqp_content_encoding", text(p.content_encoding()));
    put("amqp_delivery_mode", text(p.delivery_mode()));
    put("amqp_priority", text(p.priority()));
    put("amqp_correlation_id", text(p.correlation_id()));
    put("amqp_reply_to", text(p.reply_to()));
    put("amqp_expiration", text(p.expiration()));
    put("amqp_message_id", text(p.message_id()));
    put("amqp_timestamp", text(p.timestamp()));
    put("amqp_type", text(p.kind()));
    put("amqp_user_id", text(p.user_id()));
    put("amqp_app_id", text(p.app_id()));
    if let Some(headers) = p.headers() {
        for (k, v) in headers.inner() {
            let v = match v {
                AMQPValue::LongString(s) => Some(s.to_string()),
                AMQPValue::ShortString(s) => Some(s.to_string()),
                AMQPValue::Boolean(b) => Some(b.to_string()),
                AMQPValue::ShortShortInt(n) => Some(n.to_string()),
                AMQPValue::ShortShortUInt(n) => Some(n.to_string()),
                AMQPValue::ShortInt(n) => Some(n.to_string()),
                AMQPValue::ShortUInt(n) => Some(n.to_string()),
                AMQPValue::LongInt(n) => Some(n.to_string()),
                AMQPValue::LongUInt(n) => Some(n.to_string()),
                AMQPValue::LongLongInt(n) => Some(n.to_string()),
                AMQPValue::Float(n) => Some(n.to_string()),
                AMQPValue::Double(n) => Some(n.to_string()),
                AMQPValue::Timestamp(n) => Some(n.to_string()),
                _ => None,
            };
            put(k.as_str(), v);
        }
    }
    m
}

#[async_trait]
impl Input for AmqpInput {
    fn input_type(&self) -> &'static str {
        "amqp_0_9"
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn is_connected(&self) -> bool {
        self.live.is_some()
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        loop {
            let Some(live) = self.live.as_mut() else {
                match self.dial(ctx).await {
                    Ok(l) => {
                        self.live = Some(l);
                        self.backoff = Duration::from_millis(100);
                    }
                    Err(e @ (CoreError::AccessDenied { .. } | CoreError::Config(_))) => {
                        return Err(e)
                    }
                    Err(e) => {
                        tracing::warn!(
                            "amqp_0_9 input: connect failed, retrying in {:?}: {e}",
                            self.backoff
                        );
                        tokio::time::sleep(self.backoff).await;
                        self.backoff = (self.backoff * 2).min(Duration::from_secs(30));
                    }
                }
                continue;
            };
            let next = match tokio::time::timeout(READ_WAIT, live.consumer.next()).await {
                Err(_) => return Err(CoreError::Timeout),
                Ok(n) => n,
            };
            match next {
                Some(Ok(d)) if d.data.len() > MAX_MESSAGE_BYTES => {
                    tracing::warn!(
                        "amqp_0_9 input: a message of {} bytes is over the {MAX_MESSAGE_BYTES} byte limit and is rejected",
                        d.data.len()
                    );
                    if !self.s.auto_ack {
                        let _ = d.acker.reject(BasicRejectOptions { requeue: false }).await;
                    }
                }
                Some(Ok(d)) => {
                    let tag = live.consumer.tag().to_string();
                    return Ok(self.to_batch(d, &tag));
                }
                Some(Err(e)) => {
                    tracing::warn!("amqp_0_9 input: the connection failed: {e}");
                    self.live = None;
                }
                None => self.live = None,
            }
        }
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        if let Some(live) = self.live.take() {
            let _ = live.conn.close(200, "ekiben stream stopped".into()).await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
