// SPDX-License-Identifier: EUPL-1.2
//! MQTT notification binding — CIM 009 clause 7 (feature `mqtt`).
//!
//! 7.2: a subscription whose `notification.endpoint.uri` uses the mqtt(s)
//! scheme gets its notifications as MQTT publishes. The message is a JSON
//! object `{"metadata": {...}, "body": <Notification per 5.3.1>}`; protocol
//! parameters ride in `notifier_info` (Table 7.2-1), receiver metadata in
//! `receiver_info` (Table 7.2-2).

use crate::build_message;
use antares_jsonld::loader::EgressPolicy;
use antares_model::NgsiError;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

fn bad(m: String) -> NgsiError {
    NgsiError::BadRequestData(m)
}

use crate::redact_userinfo as redacted;

/// Parsed `mqtt[s]://[user][:pass]@host[:port]/topic[/subtopic]*` (7.2).
/// Debug prints the password as `[redacted]`: it is the subscriber's
/// credential (T-3876).
#[derive(Clone, PartialEq)]
pub struct MqttEndpoint {
    pub secure: bool,
    pub username: Option<String>,
    pub password: Option<String>,
    pub host: String,
    pub port: u16,
    pub topic: String,
}

impl std::fmt::Debug for MqttEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MqttEndpoint")
            .field("secure", &self.secure)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .field("host", &self.host)
            .field("port", &self.port)
            .field("topic", &self.topic)
            .finish()
    }
}

impl MqttEndpoint {
    /// 7.2 endpoint URI syntax. A URI that does not meet it fails the
    /// 5.2.15 restrictions, so the caller raises BadRequestData — 400 per
    /// Table 6.3.2-1. The message names only the redacted URI: the
    /// credentials 7.2 permits in the userinfo never reach the response body.
    pub fn parse(uri: &str) -> Result<Self, NgsiError> {
        let safe = redacted(uri);
        // IETF RFC 3986 3.1: scheme names are case-insensitive, and
        // `SinkRegistry::scheme_of` already lowercases to pick this sink.
        let (secure, rest) = match uri.split_once("://") {
            Some((s, r)) if s.eq_ignore_ascii_case("mqtts") => (true, r),
            Some((s, r)) if s.eq_ignore_ascii_case("mqtt") => (false, r),
            _ => return Err(bad(format!("not an mqtt(s) endpoint URI: {safe:?}"))),
        };
        let (authority, topic) = rest
            .split_once('/')
            .ok_or_else(|| bad(format!("mqtt endpoint {safe:?} has no topic")))?;
        if topic.is_empty() {
            return Err(bad(format!("mqtt endpoint {safe:?} has no topic")));
        }
        // MQTT 3.1.1 4.7.1 and 5.0 4.7.3: a PUBLISH topic holds no wildcard,
        // and 1.5.3 / 1.5.4: no U+0000 and at most 65 535 bytes. A topic
        // past those fails every delivery, so the Subscription is refused.
        if topic.contains(['+', '#', '\0']) || topic.len() > usize::from(u16::MAX) {
            return Err(bad(format!(
                "mqtt endpoint {safe:?}: the topic may hold no '+', '#' or NUL and at most 65535 bytes"
            )));
        }
        let (userinfo, hostport) = match authority.rsplit_once('@') {
            Some((u, h)) => (Some(u), h),
            None => (None, authority),
        };
        // RFC 3986 3.2.1: the userinfo is percent-encoded, so a password
        // holding `@`, `:` or `/` reaches the broker decoded.
        let decode = |part: &str| {
            percent_decoded(part).ok_or_else(|| {
                bad(format!(
                    "mqtt endpoint {safe:?}: broken percent-encoding in the userinfo"
                ))
            })
        };
        let (username, password) = match userinfo {
            None => (None, None),
            Some(u) => match u.split_once(':') {
                Some((user, pass)) => (Some(decode(user)?), Some(decode(pass)?)),
                None => (Some(decode(u)?), None),
            },
        };
        // Deliberately no IPv6-literal hosts — the binding's URI convention
        // (i.19) and the ETSI suite use hostnames; add bracket parsing when a
        // deployment needs it.
        let (host, port) = match hostport.split_once(':') {
            Some((h, p)) => (
                h.to_owned(),
                p.parse::<u16>()
                    .map_err(|_| bad(format!("invalid mqtt port in {safe:?}")))?,
            ),
            None => (hostport.to_owned(), if secure { 8883 } else { 1883 }),
        };
        if host.is_empty() {
            return Err(bad(format!("mqtt endpoint {safe:?} has no host")));
        }
        Ok(Self {
            secure,
            username,
            password,
            host,
            port,
            topic: topic.to_owned(),
        })
    }
}

/// `text` with each `%XX` decoded; `None` for a broken escape or bytes that
/// are no UTF-8.
fn percent_decoded(text: &str) -> Option<String> {
    let mut out = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(b) = bytes.next() {
        out.push(if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?
        } else {
            b
        });
    }
    String::from_utf8(out).ok()
}

/// Table 7.2-1 protocol parameters from `notification.endpoint.notifierInfo`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MqttParams {
    pub qos: u8,
    pub v5: bool,
}

impl Default for MqttParams {
    fn default() -> Self {
        Self { qos: 0, v5: true } // defaults per Table 7.2-1: QoS 0, mqtt5.0
    }
}

impl MqttParams {
    pub fn from_notifier_info<'a>(
        pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, NgsiError> {
        let mut p = Self::default();
        for (k, v) in pairs {
            match k {
                "MQTT-QoS" => {
                    p.qos = match v {
                        "0" => 0,
                        "1" => 1,
                        "2" => 2,
                        _ => return Err(bad(format!("MQTT-QoS must be 0, 1 or 2 (got {v:?})"))),
                    }
                }
                "MQTT-Version" => {
                    p.v5 = match v {
                        "mqtt5.0" => true,
                        "mqtt3.1.1" => false,
                        _ => {
                            return Err(bad(format!(
                                "MQTT-Version must be mqtt3.1.1 or mqtt5.0 (got {v:?})"
                            )))
                        }
                    }
                }
                _ => {} // unknown notifierInfo keys are not ours to police
            }
        }
        Ok(p)
    }
}

/// The key a pooled MQTT connection is shared under. Everything that changes
/// the identity the connection authenticates as (or how) must participate:
/// two subscriptions whose endpoints differ only in password must never
/// reuse one another's authenticated connection. Keys are map keys only:
/// never log them.
fn pool_key(ep: &MqttEndpoint, params: MqttParams) -> String {
    // The password participates via a hash so the plaintext never sits in a
    // map key, keyed per process: nobody outside can aim a password at
    // another one's hash, and a collision would hand one principal's
    // authenticated connection to another (F-R2ANT3-011). The username goes
    // in length-prefixed, since a decoded one may hold any character.
    static KEY: std::sync::OnceLock<std::collections::hash_map::RandomState> =
        std::sync::OnceLock::new();
    use std::hash::BuildHasher;
    let user = ep.username.as_deref().unwrap_or("");
    format!(
        "{}:{user}:{:016x}:{}@{}:{}/v{}",
        user.len(),
        KEY.get_or_init(Default::default).hash_one(&ep.password),
        ep.secure,
        ep.host,
        ep.port,
        if params.v5 { 5 } else { 3 }
    )
}

/// The client options of one MQTT endpoint, for either protocol version
/// (rumqttc has one options type per version, with the same setters): 30 s
/// keep-alive, the endpoint's credentials, TLS when it is `mqtts`.
macro_rules! options {
    ($ty:ty, $id:expr, $dial:expr, $port:expr, $ep:expr) => {{
        let mut opts = <$ty>::new($id, $dial, $port);
        opts.set_keep_alive(Duration::from_secs(30));
        if let Some(u) = &$ep.username {
            opts.set_credentials(u, $ep.password.as_deref().unwrap_or(""));
        }
        if $ep.secure {
            opts.set_transport(rumqttc::Transport::Tls(shared_tls_config()?));
        }
        opts
    }};
}

/// The mqtts trust store, built ONCE per process. Loading the platform
/// certificate store on every connect is wasted work, and rumqttc's
/// `TlsConfiguration::default()` panics when the store is unreadable — a
/// bad cert bundle must fail the one delivery, not the broker.
fn shared_tls_config() -> Result<rumqttc::TlsConfiguration, NgsiError> {
    static TLS: std::sync::OnceLock<Option<rumqttc::TlsConfiguration>> = std::sync::OnceLock::new();
    TLS.get_or_init(|| {
        // `TlsConfiguration::default()` is the only rumqttc constructor
        // that loads the platform trust store, and it panics on failure —
        // contain that so a broken cert bundle degrades to failed mqtts
        // deliveries instead of killing the process. The failure is cached:
        // a store unreadable at first use will not become readable later.
        std::panic::catch_unwind(rumqttc::TlsConfiguration::default)
            .map_err(|_| tracing::error!("mqtts: loading the platform certificate store failed"))
            .ok()
    })
    .clone()
    .ok_or_else(|| NgsiError::InternalError("mqtts trust store unavailable".into()))
}

/// Is egress to private/loopback ranges allowed? The MQTT destination is
/// client-supplied (`notification.endpoint.uri`, 7.2), so it is governed by
/// the same deployment switch, and judged by the same address classifiers,
/// as the HTTP callbacks and @context fetches: `EgressPolicy` is the one
/// copy of both. Private egress is allowed by default (dev boxes, compose
/// stacks and the conformance mocks all live there) and
/// `ANTARES_EGRESS_ALLOW_PRIVATE=false` turns the deny on for
/// internet-exposed deployments, with `ANTARES_EGRESS_PRIVATE_HOSTS` naming
/// the peers it still reaches. Read once per process.
fn egress_policy() -> EgressPolicy {
    static POLICY: std::sync::OnceLock<EgressPolicy> = std::sync::OnceLock::new();
    *POLICY.get_or_init(EgressPolicy::from_env)
}

/// Resolve the endpoint host ONCE and return the address to dial, or a
/// denial. Resolving for the check and then handing the NAME to the MQTT
/// client would leave a window in which the answer changes (DNS rebinding):
/// the connector would dial an address the policy never saw. The address
/// this returns is the address dialled, so check and connect see the same
/// answer by construction. A host that cannot be resolved is a DENIAL — a
/// destination the policy could not judge is never dialled — and the
/// resolver runs under the sink's own deadline.
async fn checked_addr(
    host: &str,
    port: u16,
    policy: EgressPolicy,
    dns_timeout: Duration,
) -> Result<std::net::SocketAddr, NgsiError> {
    let denied = || {
        NgsiError::InternalError(format!(
            "mqtt egress to {host}:{port} denied (instance metadata or private range)"
        ))
    };
    let addrs: Vec<std::net::SocketAddr> = match host
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
    {
        // an IP literal needs no resolver, and is judged by the same rules
        Ok(ip) => vec![std::net::SocketAddr::new(ip, port)],
        Err(_) => tokio::time::timeout(dns_timeout, tokio::net::lookup_host((host, port)))
            .await
            .map_err(|_| {
                NgsiError::InternalError(format!("mqtt egress: resolving {host} timed out"))
            })?
            .map_err(|e| NgsiError::InternalError(format!("mqtt egress: resolving {host}: {e}")))?
            .collect(),
    };
    addrs
        .into_iter()
        .find(|a| policy.admits(host, a.ip()))
        .ok_or_else(denied)
}

/// What to hand rumqttc as the broker address. Plain MQTT dials the checked
/// ADDRESS, which pins the resolution the policy judged (and makes the
/// event loop's own re-dial after a dropped connection reuse it instead of
/// resolving the name again, unchecked). mqtts keeps the host NAME: rumqttc
/// verifies the server certificate against the string it is given, so an
/// address there would demand an IP SAN and break certificate verification
/// against every ordinary broker certificate. For mqtts the check above
/// still gates the connect, and the certificate name check is what stops a
/// changed answer from impersonating the endpoint.
fn dial_host(ep: &MqttEndpoint, addr: std::net::SocketAddr) -> String {
    if ep.secure {
        ep.host.clone()
    } else {
        addr.ip().to_string()
    }
}

/// One pooled connection: the client plus its event-loop pump task.
enum Client {
    V3(rumqttc::AsyncClient),
    V5(rumqttc::v5::AsyncClient),
}

struct Conn {
    client: Client,
    pump: tokio::task::JoinHandle<()>,
    /// One `()` per PUBACK or PUBCOMP the broker sent: a connection is
    /// checked out by one publish at a time, so the next one answers it.
    acks: tokio::sync::mpsc::Receiver<()>,
    last_used: Instant,
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

/// MQTT delivery with a bounded per-endpoint connection pool (bounded
/// WITH eviction; timeouts fixed at construction).
pub struct MqttSink {
    pool: Mutex<HashMap<String, Conn>>,
    cap: usize,
    timeout: Duration,
}

impl Default for MqttSink {
    fn default() -> Self {
        Self::new(32, Duration::from_secs(5))
    }
}

impl MqttSink {
    pub fn new(cap: usize, timeout: Duration) -> Self {
        Self {
            pool: Mutex::new(HashMap::new()),
            cap,
            timeout,
        }
    }

    /// Deliver one notification message. `message` is the 7.2 wrapper from
    /// [`build_message`], serialized by the caller once per subscription.
    pub async fn publish_message(
        &self,
        ep: &MqttEndpoint,
        params: MqttParams,
        message: &[u8],
    ) -> Result<(), NgsiError> {
        let key = pool_key(ep, params);
        // one retry with a fresh connection: a pooled client whose broker
        // restarted fails the first publish; a dead broker fails both.
        for attempt in 0..2 {
            let mut conn = match self.checkout(&key) {
                Some(c) => c,
                None => self.connect(ep, params).await?,
            };
            // an ack a timed-out publish left behind answers nothing now
            while conn.acks.try_recv().is_ok() {}
            let (client, acks) = (&conn.client, &mut conn.acks);
            // QoS 1 and 2 are delivered once the broker acknowledged them
            // (PUBACK, PUBCOMP), not once rumqttc queued them (F-ANT3-022)
            let published = tokio::time::timeout(self.timeout, async {
                Self::publish(client, &ep.topic, params.qos, message).await?;
                if params.qos > 0 && acks.recv().await.is_none() {
                    return Err("connection lost before the broker acknowledged".to_owned());
                }
                Ok(())
            })
            .await;
            match published {
                Ok(Ok(())) if !conn.pump.is_finished() => {
                    self.checkin(key, conn);
                    return Ok(());
                }
                _ if attempt == 0 => continue, // drop conn, retry fresh
                Ok(Ok(())) => {
                    return Err(NgsiError::InternalError(
                        "mqtt connection lost during publish".into(),
                    ))
                }
                Ok(Err(e)) => return Err(NgsiError::InternalError(format!("mqtt publish: {e}"))),
                Err(_) => {
                    return Err(NgsiError::InternalError(format!(
                        "mqtt publish to {}:{} timed out",
                        ep.host, ep.port
                    )))
                }
            }
        }
        Err(NgsiError::InternalError(
            "mqtt publish exhausted its retry without a verdict".into(),
        ))
    }

    async fn publish(client: &Client, topic: &str, qos: u8, payload: &[u8]) -> Result<(), String> {
        match client {
            Client::V3(c) => {
                let qos = rumqttc::qos(qos).map_err(|e| e.to_string())?;
                c.publish(topic, qos, false, payload.to_vec())
                    .await
                    .map_err(|e| e.to_string())
            }
            Client::V5(c) => {
                let qos =
                    rumqttc::v5::mqttbytes::qos(qos).ok_or_else(|| format!("invalid QoS {qos}"))?;
                c.publish(topic, qos, false, payload.to_vec())
                    .await
                    .map_err(|e| e.to_string())
            }
        }
    }

    fn checkout(&self, key: &str) -> Option<Conn> {
        self.pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key)
    }

    fn checkin(&self, key: String, mut conn: Conn) {
        conn.last_used = Instant::now();
        let mut pool = self
            .pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pool.retain(|_, c| !c.pump.is_finished());
        pool.insert(key, conn);
        // bounded with eviction: drop the least-recently-used overflow.
        while pool.len() > self.cap {
            if let Some(oldest) = pool
                .iter()
                .min_by_key(|(_, c)| c.last_used)
                .map(|(k, _)| k.clone())
            {
                pool.remove(&oldest);
            }
        }
    }

    /// Connect and wait for ConnAck (a dead broker must fail delivery, not
    /// queue forever), then hand the event loop to a pump task.
    async fn connect(&self, ep: &MqttEndpoint, params: MqttParams) -> Result<Conn, NgsiError> {
        // A monotonic counter, not a timestamp: `Instant::now().elapsed()` is
        // ~0 for every caller, so two connects in one process would claim the
        // same client id and the broker would kick the older session.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = format!(
            "antares-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let refused = |e: String| {
            NgsiError::InternalError(format!("mqtt connect {}:{}: {e}", ep.host, ep.port))
        };
        // Egress policy first: resolve once, judge the answer, dial what was
        // judged. Nothing below opens a socket to an unchecked destination.
        let addr = checked_addr(&ep.host, ep.port, egress_policy(), self.timeout).await?;
        let dial = dial_host(ep, addr);
        if params.v5 {
            let opts = options!(rumqttc::v5::MqttOptions, id, dial, addr.port(), ep);
            let (client, mut eventloop) = rumqttc::v5::AsyncClient::new(opts, 16);
            tokio::time::timeout(self.timeout, async {
                loop {
                    match eventloop.poll().await {
                        Ok(rumqttc::v5::Event::Incoming(
                            rumqttc::v5::mqttbytes::v5::Packet::ConnAck(_),
                        )) => return Ok(()),
                        Ok(_) => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
            })
            .await
            .map_err(|_| refused("connect timeout".into()))?
            .map_err(refused)?;
            let (ack, acks) = tokio::sync::mpsc::channel(16);
            let pump = tokio::spawn(async move {
                while let Ok(event) = eventloop.poll().await {
                    if matches!(
                        event,
                        rumqttc::v5::Event::Incoming(
                            rumqttc::v5::mqttbytes::v5::Packet::PubAck(_)
                                | rumqttc::v5::mqttbytes::v5::Packet::PubComp(_)
                        )
                    ) {
                        let _ = ack.try_send(());
                    }
                }
            });
            Ok(Conn {
                client: Client::V5(client),
                pump,
                acks,
                last_used: Instant::now(),
            })
        } else {
            let opts = options!(rumqttc::MqttOptions, id, dial, addr.port(), ep);
            let (client, mut eventloop) = rumqttc::AsyncClient::new(opts, 16);
            tokio::time::timeout(self.timeout, async {
                loop {
                    match eventloop.poll().await {
                        Ok(rumqttc::Event::Incoming(rumqttc::Packet::ConnAck(_))) => return Ok(()),
                        Ok(_) => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
            })
            .await
            .map_err(|_| refused("connect timeout".into()))?
            .map_err(refused)?;
            let (ack, acks) = tokio::sync::mpsc::channel(16);
            let pump = tokio::spawn(async move {
                while let Ok(event) = eventloop.poll().await {
                    if matches!(
                        event,
                        rumqttc::Event::Incoming(
                            rumqttc::Packet::PubAck(_) | rumqttc::Packet::PubComp(_)
                        )
                    ) {
                        let _ = ack.try_send(());
                    }
                }
            });
            Ok(Conn {
                client: Client::V3(client),
                pump,
                acks,
                last_used: Instant::now(),
            })
        }
    }
}

/// One message an inbound subscription received: the topic it was
/// published on and its payload, as sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Inbound {
    pub topic: String,
    pub payload: Vec<u8>,
}

/// Subscribe to `ep.topic` and hand every message published there to `tx`,
/// for an inbound bridge Channel (ADR-0031). MQTT 5 only, at QoS 1, under the
/// shared subscription `$share/{group}/{topic}` (MQTT 5.0 4.8.2): every
/// broker replica binds the same Channels, and the server hands each message
/// to one of them, so a sample is written once whatever the replica count.
///
/// The destination is judged by the egress policy before the first connect
/// and the checked address is dialled, as for a notification. The event loop
/// runs on the returned task: it reconnects after a dropped connection and
/// subscribes again on every ConnAck (a clean start keeps no subscription).
/// A full `tx` drops the message rather than stalling the event loop's
/// keep-alive. Aborting the task closes the subscription.
///
/// # Errors
/// The egress denial, or a broker that does not ConnAck within `timeout`.
pub async fn listen(
    ep: &MqttEndpoint,
    group: &str,
    tx: tokio::sync::mpsc::Sender<Inbound>,
    timeout: Duration,
) -> Result<tokio::task::JoinHandle<()>, NgsiError> {
    use rumqttc::v5::mqttbytes::v5::Packet;
    use rumqttc::v5::mqttbytes::QoS;
    use rumqttc::v5::Event;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = format!(
        "antares-in-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let addr = checked_addr(&ep.host, ep.port, egress_policy(), timeout).await?;
    let opts = options!(
        rumqttc::v5::MqttOptions,
        id,
        dial_host(ep, addr),
        addr.port(),
        ep
    );
    let (client, mut eventloop) = rumqttc::v5::AsyncClient::new(opts, 16);
    let filter = format!("$share/{group}/{}", ep.topic);
    let refused =
        |e: String| NgsiError::InternalError(format!("mqtt connect {}:{}: {e}", ep.host, ep.port));
    tokio::time::timeout(timeout, async {
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => return Ok(()),
                Ok(_) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    })
    .await
    .map_err(|_| refused("connect timeout".into()))?
    .map_err(refused)?;
    client
        .subscribe(filter.clone(), QoS::AtLeastOnce)
        .await
        .map_err(|e| refused(e.to_string()))?;
    let (host, port) = (ep.host.clone(), ep.port);
    Ok(tokio::spawn(async move {
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::Publish(p))) => {
                    let msg = Inbound {
                        topic: String::from_utf8_lossy(&p.topic).into_owned(),
                        payload: p.payload.to_vec(),
                    };
                    match tx.try_send(msg) {
                        Ok(()) => {}
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return,
                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                            tracing::debug!(
                                "mqtt inbound {host}:{port}: queue full, message dropped"
                            );
                        }
                    }
                }
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    if client
                        .subscribe(filter.clone(), QoS::AtLeastOnce)
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("mqtt inbound {host}:{port}: {e}; reconnecting in 5 s");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }))
}

/// Clause 7: the mqtt(s) binding. `parse_endpoint` holds Table 7.2-1 (the
/// endpoint URI syntax and the notifier parameters) at subscription
/// creation; `deliver` publishes the 7.2 message — `{"metadata": …,
/// "body": <Notification>}` — on the endpoint's topic.
impl crate::NotificationSink for MqttSink {
    fn schemes(&self) -> &'static [&'static str] {
        &["mqtt", "mqtts"]
    }

    fn parse_endpoint(&self, uri: &str, notifier_info: &[(&str, &str)]) -> Result<(), NgsiError> {
        MqttEndpoint::parse(uri)?;
        MqttParams::from_notifier_info(notifier_info.iter().copied())?;
        Ok(())
    }

    fn deliver<'a>(
        &'a self,
        uri: &'a str,
        out: &'a crate::Outbound,
        _timeout: Duration,
    ) -> crate::DeliveryFuture<'a> {
        Box::pin(async move {
            // Creation validated the endpoint (7.2 / Table 7.2-1); a parse
            // failure here means a hand-edited row. The message names only
            // the redacted URI: 7.2 permits credentials in the userinfo.
            let Ok(ep) = MqttEndpoint::parse(uri) else {
                return Err(crate::DeliveryError::failed(format!(
                    "mqtt endpoint {:?} unusable",
                    redacted(uri)
                )));
            };
            let Ok(params) = MqttParams::from_notifier_info(out.notifier_pairs()) else {
                return Err(crate::DeliveryError::failed(format!(
                    "mqtt notifierInfo of {:?} unusable",
                    redacted(uri)
                )));
            };
            let msg = build_message(&out.body, &out.accept, Some(&out.link), &out.receiver_info);
            let bytes = antares_model::ordered_vec(&msg);
            self.publish_message(&ep, params, &bytes)
                .await
                // broker/socket-level failure — keep the timeout guard
                .map_err(|e| crate::DeliveryError::timeout(e.to_string()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn debug_never_prints_the_password() {
        let e = MqttEndpoint::parse("mqtt://u:s3cret@host:9001/t").expect("user+pass");
        let shown = format!("{e:?}");
        assert!(!shown.contains("s3cret"), "{shown}");
        assert!(
            shown.contains("[redacted]") && shown.contains("\"u\"") && shown.contains("host"),
            "{shown}"
        );
        let none = format!(
            "{:?}",
            MqttEndpoint::parse("mqtt://u@host/t").expect("user")
        );
        assert!(none.contains("password: None"), "{none}");
    }

    #[test]
    fn parses_endpoint_variants() {
        let e = MqttEndpoint::parse("mqtt://host/topic").expect("plain");
        assert_eq!(
            e,
            MqttEndpoint {
                secure: false,
                username: None,
                password: None,
                host: "host".into(),
                port: 1883,
                topic: "topic".into()
            }
        );
        let e = MqttEndpoint::parse("mqtt://host:8085/a/b/c").expect("port+subtopics");
        assert_eq!(e.port, 8085);
        assert_eq!(e.topic, "a/b/c");
        let e = MqttEndpoint::parse("mqtt://user@host/t").expect("user");
        assert_eq!(e.username.as_deref(), Some("user"));
        assert_eq!(e.password, None);
        let e = MqttEndpoint::parse("mqtt://u:p@host:9001/t").expect("user+pass+port");
        assert_eq!(e.username.as_deref(), Some("u"));
        assert_eq!(e.password.as_deref(), Some("p"));
        assert_eq!(e.port, 9001);
        let e = MqttEndpoint::parse("mqtts://host/t").expect("tls");
        assert!(e.secure);
        assert_eq!(e.port, 8883, "mqtts default port");
    }

    /// IETF RFC 3986 3.1: scheme names are case-insensitive.
    /// `SinkRegistry::scheme_of` lowercases before it picks this sink, so an
    /// uppercase-scheme endpoint is routed here and this parser decides —
    /// rejecting it would 400 an endpoint URI the registry says it serves.
    #[test]
    fn the_scheme_is_matched_case_insensitively() {
        assert!(
            MqttEndpoint::parse("MQTT://host/topic")
                .expect("upper")
                .port
                == 1883
        );
        let e = MqttEndpoint::parse("MQTTS://host/t").expect("upper tls");
        assert!(e.secure);
        assert_eq!(e.port, 8883);
        assert!(MqttEndpoint::parse("MqTt://host/t").is_ok());
    }

    #[test]
    fn rejects_bad_endpoints() {
        for uri in [
            "http://host/topic",
            "mqtt://host",
            "mqtt://host/",
            "mqtt:///topic",
            "mqtt://host:notaport/t",
        ] {
            assert!(MqttEndpoint::parse(uri).is_err(), "{uri} must be rejected");
        }
    }

    /// 7.2 endpoint URIs may carry credentials in the userinfo
    /// (`mqtt[s]://<username>:<password>@host`). A parse failure is answered
    /// to the client as BadRequestData (5.8.1.4, 400 per Table 6.3.2-1) and
    /// the message becomes the 5.5.3 ProblemDetails `detail`, so no password
    /// may appear in it.
    #[test]
    fn parse_errors_redact_endpoint_userinfo() {
        for uri in [
            "mqtt://user:hunter2@host",            // no topic
            "mqtt://user:hunter2@host/",           // empty topic
            "mqtt://user:hunter2@host:notaport/t", // bad port
            "mqtts://user:hunter2@/t",             // no host
            "http://user:hunter2@host/t",          // wrong scheme
        ] {
            let NgsiError::BadRequestData(msg) =
                MqttEndpoint::parse(uri).expect_err(&format!("{uri} must be rejected"))
            else {
                panic!("{uri} must be BadRequestData (400, Table 6.3.2-1)");
            };
            assert!(
                !msg.contains("hunter2"),
                "the password leaked into the 400 detail: {msg}"
            );
            assert!(
                !msg.contains("user:"),
                "the userinfo leaked into the 400 detail: {msg}"
            );
            // the detail must still be useful: scheme and host survive
            assert!(
                msg.contains("host") || uri.contains("@/"),
                "the redacted detail lost the host: {msg}"
            );
        }
    }

    #[test]
    fn notifier_info_defaults_and_validation() {
        let p = MqttParams::from_notifier_info([]).expect("defaults");
        assert_eq!(p, MqttParams { qos: 0, v5: true });
        let p = MqttParams::from_notifier_info([("MQTT-QoS", "2"), ("MQTT-Version", "mqtt3.1.1")])
            .expect("explicit");
        assert_eq!(p, MqttParams { qos: 2, v5: false });
        assert!(MqttParams::from_notifier_info([("MQTT-QoS", "3")]).is_err());
        assert!(MqttParams::from_notifier_info([("MQTT-Version", "mqtt4")]).is_err());
    }

    /// The pool key must separate sessions by credentials: same user with
    /// two different passwords = two different authenticated principals,
    /// which must never share one session. The plaintext password itself
    /// must not appear in the key.
    #[test]
    fn pool_key_separates_credentials() {
        let p = MqttParams::default();
        let a = MqttEndpoint::parse("mqtt://u:secret-one@host/t").expect("a");
        let b = MqttEndpoint::parse("mqtt://u:secret-two@host/t").expect("b");
        let c = MqttEndpoint::parse("mqtt://u:secret-one@host/t").expect("c");
        assert_ne!(
            pool_key(&a, p),
            pool_key(&b, p),
            "different passwords must not share an authenticated session"
        );
        assert_eq!(
            pool_key(&a, p),
            pool_key(&c, p),
            "identical endpoints must keep pooling"
        );
        assert!(
            !pool_key(&a, p).contains("secret-one"),
            "the plaintext password must never appear in the key"
        );
        // no password vs some password are different principals too
        let none = MqttEndpoint::parse("mqtt://u@host/t").expect("none");
        assert_ne!(pool_key(&a, p), pool_key(&none, p));
        // a decoded username may hold `:`: it never reads as another split
        let colon = MqttEndpoint::parse("mqtt://u%3Asecret-one@host/t").expect("colon");
        assert_ne!(pool_key(&a, p), pool_key(&colon, p));
    }

    /// F-R2ANT3-012: the userinfo is percent-decoded (RFC 3986 3.2.1), and a
    /// broken escape is refused.
    #[test]
    fn the_userinfo_is_percent_decoded() {
        let e = MqttEndpoint::parse("mqtt://us%3Aer:p%40ss%2Fw@host/t").expect("encoded");
        assert_eq!(e.username.as_deref(), Some("us:er"));
        assert_eq!(e.password.as_deref(), Some("p@ss/w"));
        for broken in [
            "mqtt://u:p%4@host/t",
            "mqtt://u%zz@host/t",
            "mqtt://u:%ff@host/t",
        ] {
            assert!(MqttEndpoint::parse(broken).is_err(), "{broken}");
        }
    }

    /// F-R2ANT3-007: a topic no PUBLISH may carry is refused at parse.
    #[test]
    fn a_topic_no_publish_may_carry_is_refused() {
        let long = format!("mqtt://h/{}", "a".repeat(65_536));
        for uri in [
            "mqtt://h/a/+/b",
            "mqtt://h/a/#",
            "mqtt://h/a\0b",
            long.as_str(),
        ] {
            assert!(MqttEndpoint::parse(uri).is_err(), "{:.40}", uri);
        }
        assert!(
            MqttEndpoint::parse("mqtt://h/a/b?x=1").is_ok(),
            "`?` is a topic character"
        );
    }

    /// The mqtts trust store is loaded once and the same shared rustls
    /// config is handed to every connect. (The failure path — an unreadable
    /// platform store — cannot be simulated in a unit test; the helper's
    /// error contract covers it.)
    #[test]
    fn tls_config_is_built_once_and_shared() {
        let a = shared_tls_config().expect("first load");
        let b = shared_tls_config().expect("second load");
        let (rumqttc::TlsConfiguration::Rustls(a), rumqttc::TlsConfiguration::Rustls(b)) = (a, b)
        else {
            panic!("expected the injected-rustls variant");
        };
        assert!(
            std::sync::Arc::ptr_eq(&a, &b),
            "each call built a fresh trust store instead of sharing one"
        );
    }

    /// A stub MQTT 3.1.1 broker on loopback: it accepts every CONNECT and
    /// answers a QoS 1 PUBLISH with its PUBACK only when `ack` holds.
    async fn stub_broker(ack: bool) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    while let Ok(n) = s.read(&mut buf).await {
                        let Some(packet) = buf.get(..n).filter(|p| !p.is_empty()) else {
                            return;
                        };
                        let reply: Vec<u8> = match packet.first().map(|b| b >> 4) {
                            Some(1) => vec![0x20, 0x02, 0x00, 0x00],
                            // header, one-byte length, topic length + topic, pkid
                            Some(3) if ack => {
                                let topic = usize::from(packet.get(3).copied().unwrap_or(0));
                                let at = 4usize.saturating_add(topic);
                                let pkid = packet.get(at..at.saturating_add(2)).unwrap_or(&[0, 1]);
                                [&[0x40, 0x02][..], pkid].concat()
                            }
                            _ => continue,
                        };
                        if s.write_all(&reply).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        port
    }

    /// F-ANT3-022: a QoS 1 publish is delivered once the broker sent its
    /// PUBACK; a broker that never acknowledges fails it within the timeout.
    #[tokio::test]
    async fn a_qos1_publish_waits_for_the_puback() {
        let params = MqttParams { qos: 1, v5: false };
        let sink = MqttSink::new(2, Duration::from_millis(300));
        let silent =
            MqttEndpoint::parse(&format!("mqtt://127.0.0.1:{}/t", stub_broker(false).await))
                .expect("parse");
        let started = Instant::now();
        let err = sink
            .publish_message(&silent, params, b"{}")
            .await
            .expect_err("no PUBACK, no delivery");
        assert!(err.to_string().contains("timed out"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        let acking =
            MqttEndpoint::parse(&format!("mqtt://127.0.0.1:{}/t", stub_broker(true).await))
                .expect("parse");
        sink.publish_message(&acking, params, b"{}")
            .await
            .expect("the PUBACK delivers");
    }

    /// The MQTT destination is client-supplied (`notification.endpoint.uri`,
    /// 7.2), so it is an egress target: the cloud instance-metadata range is
    /// refused before any socket is opened, whatever the private-egress
    /// switch says, and the refusal must not echo the endpoint credentials.
    #[tokio::test]
    async fn deliver_refuses_instance_metadata_endpoint() {
        let ep = MqttEndpoint::parse("mqtt://user:hunter2@169.254.169.254/t").expect("parse");
        let sink = MqttSink::new(2, Duration::from_millis(250));
        let err = sink
            .publish_message(&ep, MqttParams::default(), b"{}")
            .await
            .expect_err("the metadata range must never be dialled");
        let msg = err.to_string();
        assert!(
            msg.contains("denied"),
            "expected an egress denial, got: {msg}"
        );
        assert!(
            !msg.contains("hunter2"),
            "the endpoint password leaked into the delivery error: {msg}"
        );
    }

    /// Egress classification, restated from the HTTP side's policy: the
    /// metadata range is refused unconditionally, private ranges only when
    /// the deployment switched private egress off, and IPv4-mapped IPv6
    /// spellings are judged as their IPv4 selves.
    const OPEN: EgressPolicy = EgressPolicy {
        allow_private: true,
        private_hosts: &[],
    };
    const CLOSED: EgressPolicy = EgressPolicy {
        allow_private: false,
        private_hosts: &[],
    };

    #[tokio::test]
    async fn checked_addr_applies_the_egress_rules_to_resolved_addresses() {
        let d = Duration::from_secs(2);
        // metadata: denied with private egress ALLOWED (the default)
        // the 6to4 spelling too (IETF RFC 3056: the two segments after the
        // 2002:: prefix ARE the IPv4 destination), which proves this binding
        // goes through the shared classifier rather than its own check
        for host in [
            "169.254.169.254",
            "::ffff:169.254.169.254",
            "fd00:ec2::254",
            "2002:a9fe:a9fe::",
        ] {
            let e = checked_addr(host, 1883, OPEN, d)
                .await
                .expect_err("metadata range must be refused whatever the switch says");
            assert!(e.to_string().contains("denied"), "{host}: {e}");
        }
        // loopback and RFC 1918: allowed by default, refused when the
        // deployment turns private egress off
        for host in [
            "127.0.0.1",
            "::ffff:127.0.0.1",
            "10.1.2.3",
            "::1",
            "localhost",
        ] {
            let ok = checked_addr(host, 1883, OPEN, d)
                .await
                .unwrap_or_else(|e| panic!("{host} must be reachable by default: {e}"));
            assert_eq!(ok.port(), 1883);
            assert!(
                checked_addr(host, 1883, CLOSED, d).await.is_err(),
                "{host} must be refused with private egress off"
            );
        }
        // a public literal clears the strict policy and comes back as the
        // ADDRESS to dial — the resolution the check judged, pinned
        let a = checked_addr("93.184.216.34", 8883, CLOSED, d)
            .await
            .expect("public address allowed");
        assert_eq!(a.to_string(), "93.184.216.34:8883");
        // a name that cannot be resolved is a denial, not a pass-through
        assert!(
            checked_addr("no-such-host.invalid", 1883, OPEN, d)
                .await
                .is_err(),
            "an unresolvable destination must not be dialled"
        );
    }

    /// The address the policy judged is the address dialled — except for
    /// mqtts, where the certificate is verified against the host NAME.
    #[test]
    fn dial_host_pins_the_address_and_keeps_the_tls_name() {
        let plain = MqttEndpoint::parse("mqtt://broker.example/t").expect("plain");
        let addr = "203.0.113.7:1883".parse().expect("addr");
        assert_eq!(dial_host(&plain, addr), "203.0.113.7");
        let secure = MqttEndpoint::parse("mqtts://broker.example/t").expect("secure");
        assert_eq!(
            dial_host(&secure, addr),
            "broker.example",
            "mqtts must dial the name so the certificate name check still applies"
        );
        // IPv6 comes back unbracketed, which is what rumqttc resolves
        let v6 = "[2001:db8::1]:1883".parse().expect("v6 addr");
        assert_eq!(dial_host(&plain, v6), "2001:db8::1");
    }

    #[test]
    fn message_wrapper_shape() {
        let body = json!({"id": "urn:n:1", "type": "Notification"});
        let m = build_message(
            &body,
            "application/json",
            Some("<https://ctx>; rel=\"http://www.w3.org/ns/json-ld#context\""),
            &[("MyKey".into(), "MyValue".into())],
        );
        assert_eq!(m["body"], body);
        assert_eq!(m["metadata"]["Content-Type"], "application/json");
        assert!(m["metadata"]["Link"]
            .as_str()
            .expect("link present")
            .contains("json-ld#context"));
        assert_eq!(m["metadata"]["MyKey"], "MyValue");

        // ld+json: @context is in the body, no Link in metadata (Table 7.2-2)
        let m = build_message(&body, "application/ld+json", Some("<x>"), &[]);
        assert_eq!(m["metadata"]["Content-Type"], "application/ld+json");
        assert!(m["metadata"].get("Link").is_none());
    }

    /// Table 7.2-2 names the source of two metadata keys: `Content-Type`
    /// comes from `endpoint.accept` and `Link` from the served @context.
    /// receiverInfo entries are added "additionally", so one keyed like a
    /// table parameter cannot become that parameter's value — a subscriber
    /// would otherwise decide the MIME type of its own notifications
    /// against 6.3.8's "changed by means of the endpoint.accept member",
    /// and could strip the @context reference the clause requires.
    #[test]
    fn receiver_info_cannot_take_over_a_table_7_2_2_parameter() {
        let body = json!({"id": "urn:n:1", "type": "Notification"});
        let link = "<https://ctx>; rel=\"http://www.w3.org/ns/json-ld#context\"";
        let m = build_message(
            &body,
            "application/json",
            Some(link),
            &[
                ("Content-Type".into(), "text/plain".into()),
                ("Link".into(), "<https://evil>".into()),
                ("NGSILD-Tenant".into(), "acme".into()),
            ],
        );
        assert_eq!(m["metadata"]["Content-Type"], "application/json");
        assert_eq!(m["metadata"]["Link"], link);
        // every other pair is still added, this one by the caller (6.3.22)
        assert_eq!(m["metadata"]["NGSILD-Tenant"], "acme");
    }
}
