// SPDX-License-Identifier: EUPL-1.2
//! Inbound bridge Channels (ADR-0031, T-5079): one topic of an external
//! message bridge bound to one Attribute of one Entity.
//!
//! A Channel is bound with `PUT /x/channels/{id}` under the binder's
//! `NGSILD-Tenant`. The bind is decided as the write it stands for, a 5.6.3
//! Append Attributes of that Attribute, by the policy seam, and every sample
//! is decided again as the binding subject: a sample runs through
//! [`AppState::call`] like any request, so negotiation, the bounds wall, the
//! policy seam and views apply exactly once. `retention: mirror` stores the
//! value (and its history, by the type's rules) and notifies; `relay`
//! notifies the matching Subscriptions and keeps nothing.
//!
//! Inbound only: the outbound half is a clause 7 MQTT notification
//! Subscription. A sample that is one of those notifications is dropped, so
//! a Channel and a Subscription on the same topic cannot loop.

use crate::attrs::{Intent, INTENT};
use crate::negotiate::{tenant_from, ApiError, ApiResult};
use crate::state::AppState;
use antares_model::{NgsiError, TenantId};
use antares_store::Kind;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The broker's own tenant the Channel documents live under, keyed
/// `{tenant}:{id}` (both `[A-Za-z0-9_-]`, so the key splits one way).
pub const CHANNEL_TENANT: &str = "antares-channels";

/// Samples per second a Channel accepts unless its binder asks for fewer.
pub const DEFAULT_RATE: u32 = 10;
/// The most samples per second any Channel accepts.
pub const MAX_RATE: u32 = 100;
/// Payload bytes a sample may carry unless its binder asks for fewer.
pub const DEFAULT_BYTES: u32 = 4096;
/// The largest sample payload any Channel accepts.
pub const MAX_BYTES: u32 = 65_536;
/// The longest topic a Channel binds.
const MAX_TOPIC: usize = 256;
/// Samples waiting for one Channel's worker; a full queue drops the newest.
const QUEUE: usize = 256;

/// One message a bridge received on a topic.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub topic: String,
    pub payload: Vec<u8>,
}

/// What [`Bridge::open`] answers: the task that feeds the sender, if the
/// bridge runs one, or the bridge's refusal.
pub type Opening<'a> =
    Pin<Box<dyn Future<Output = Result<Option<tokio::task::AbortHandle>, String>> + Send + 'a>>;

/// A message bridge Channels bind topics on. `open` starts delivering every
/// message published on `topic` to `tx`; the delivery stops when the
/// returned task is aborted or `tx`'s receiver is dropped.
pub trait Bridge: Send + Sync {
    fn open<'a>(&'a self, topic: &'a str, tx: tokio::sync::mpsc::Sender<Sample>) -> Opening<'a>;
}

/// A topic and the Channel queue its samples go to.
type Bound = (String, tokio::sync::mpsc::Sender<Sample>);

/// The reference bridge: a message published on it reaches the Channels
/// bound to its topic from the bridge's own thread, as a network bridge's
/// would arrive from its event loop. The test double of ADR-0031 and a way
/// for an embedding host to feed Channels without a network.
pub struct Loopback {
    tx: Mutex<std::sync::mpsc::Sender<Sample>>,
    subs: Arc<Mutex<Vec<Bound>>>,
}

impl Default for Loopback {
    fn default() -> Self {
        Self::new()
    }
}

impl Loopback {
    /// A loopback bridge and its dispatch thread, which ends with the bridge.
    #[must_use]
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Sample>();
        let subs: Arc<Mutex<Vec<Bound>>> = Arc::default();
        let table = Arc::clone(&subs);
        std::thread::spawn(move || {
            for sample in rx {
                let mut subs = table
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                subs.retain(|(_, s)| !s.is_closed());
                for (topic, s) in subs.iter() {
                    if *topic == sample.topic {
                        // a full Channel queue drops the sample, as MQTT does
                        let _ = s.try_send(sample.clone());
                    }
                }
            }
        });
        Self {
            tx: Mutex::new(tx),
            subs,
        }
    }

    /// Publish `payload` on `topic`.
    pub fn publish(&self, topic: &str, payload: impl Into<Vec<u8>>) {
        let tx = self
            .tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = tx.send(Sample {
            topic: topic.to_owned(),
            payload: payload.into(),
        });
    }
}

impl Bridge for Loopback {
    fn open<'a>(&'a self, topic: &'a str, tx: tokio::sync::mpsc::Sender<Sample>) -> Opening<'a> {
        Box::pin(async move {
            self.subs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((topic.to_owned(), tx));
            Ok(None)
        })
    }
}

/// A broker reached over MQTT 5 (feature `mqtt`): `uri` is an
/// `mqtt[s]://[user[:pass]@]host[:port]/` base whose topic part the Channel
/// supplies. The URI comes from the deployment's configuration, never from a
/// tenant, so a credential in it stays out of every stored document.
#[cfg(feature = "mqtt")]
pub struct Mqtt {
    base: antares_notifier::mqtt::MqttEndpoint,
}

#[cfg(feature = "mqtt")]
impl Mqtt {
    /// # Errors
    /// The 7.2 endpoint syntax, with the credential redacted.
    pub fn new(uri: &str) -> Result<Self, String> {
        // the base names no topic; parse it with a placeholder one
        let with_topic = format!("{}/_", uri.trim_end_matches('/'));
        antares_notifier::mqtt::MqttEndpoint::parse(&with_topic)
            .map(|base| Self { base })
            .map_err(|e| e.to_string())
    }
}

#[cfg(feature = "mqtt")]
impl Bridge for Mqtt {
    fn open<'a>(&'a self, topic: &'a str, tx: tokio::sync::mpsc::Sender<Sample>) -> Opening<'a> {
        Box::pin(async move {
            let mut ep = self.base.clone();
            ep.topic = topic.to_owned();
            let (in_tx, mut in_rx) = tokio::sync::mpsc::channel(QUEUE);
            let pump =
                antares_notifier::mqtt::listen(&ep, "antares", in_tx, Duration::from_secs(5))
                    .await
                    .map_err(|e| e.to_string())?;
            let forward = tokio::spawn(async move {
                let _pump = AbortOnDrop(pump.abort_handle());
                while let Some(m) = in_rx.recv().await {
                    let sample = Sample {
                        topic: m.topic,
                        payload: m.payload,
                    };
                    if tx.send(sample).await.is_err() {
                        return;
                    }
                }
            });
            Ok(Some(forward.abort_handle()))
        })
    }
}

/// Aborts a task when dropped.
#[cfg(feature = "mqtt")]
struct AbortOnDrop(tokio::task::AbortHandle);

#[cfg(feature = "mqtt")]
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The bridges a deployment names (`ANTARES_CHANNEL_BROKERS`): `name=uri`
/// pairs, comma-separated, where `uri` is `loopback` or an mqtt(s) base.
///
/// # Errors
/// A pair without `=`, a name outside `[A-Za-z0-9_-]{1,64}`, a repeated
/// name, or a URI the bridge refuses.
pub fn bridges_from(spec: &str) -> Result<HashMap<String, Arc<dyn Bridge>>, String> {
    let mut out: HashMap<String, Arc<dyn Bridge>> = HashMap::new();
    for pair in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let Some((name, uri)) = pair.split_once('=') else {
            return Err(format!(
                "ANTARES_CHANNEL_BROKERS: {:?} is no name=uri pair",
                pair.split('@').next_back().unwrap_or_default()
            ));
        };
        let name = name.trim();
        if !is_name(name) {
            return Err(format!(
                "ANTARES_CHANNEL_BROKERS: bridge name {name:?} is not [A-Za-z0-9_-]{{1,64}}"
            ));
        }
        let bridge: Arc<dyn Bridge> = match uri.trim() {
            "loopback" => Arc::new(Loopback::new()),
            #[cfg(feature = "mqtt")]
            uri => Arc::new(
                Mqtt::new(uri).map_err(|e| format!("ANTARES_CHANNEL_BROKERS {name}: {e}"))?,
            ),
            #[cfg(not(feature = "mqtt"))]
            _ => {
                return Err(format!(
                    "ANTARES_CHANNEL_BROKERS {name}: this build has no mqtt bridge"
                ))
            }
        };
        if out.insert(name.to_owned(), bridge).is_some() {
            return Err(format!("ANTARES_CHANNEL_BROKERS names {name:?} twice"));
        }
    }
    Ok(out)
}

fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Retention {
    /// Stored as a write: current state, history by the type's rules, and
    /// the notifications of that write.
    Mirror,
    /// Notified to the matching Subscriptions; no Entity and no history row.
    Relay,
}

/// What a binder sends. `direction` is `in`, the one this broker binds.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Spec {
    entity: String,
    attribute: String,
    broker: String,
    topic: String,
    #[serde(default)]
    direction: Option<String>,
    retention: Retention,
    #[serde(default)]
    max_rate: Option<u32>,
    #[serde(default)]
    max_bytes: Option<u32>,
}

/// A bound Channel, as stored and served (without its subject).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    pub id: String,
    pub tenant: Option<String>,
    pub entity: String,
    pub attribute: String,
    pub broker: String,
    pub topic: String,
    pub direction: String,
    pub retention: Retention,
    pub max_rate: u32,
    pub max_bytes: u32,
    /// The bind request's `Link` header: the samples' terms mean what the
    /// binder's did.
    #[serde(default)]
    pub link: Option<String>,
}

fn bad(m: impl Into<String>) -> ApiError {
    NgsiError::BadRequestData(m.into()).into()
}

impl Spec {
    fn check(self, id: &str, bridges: &HashMap<String, Arc<dyn Bridge>>) -> ApiResult<Channel> {
        if !is_name(id) {
            return Err(bad(
                "channel id must be [A-Za-z0-9_-]{1,64}, e.g. bus-12-speed",
            ));
        }
        antares_model::EntityId::new(&self.entity).map_err(|e| bad(format!("entity: {e}")))?;
        antares_model::check_attr_name(&self.attribute)
            .map_err(|e| bad(format!("attribute: {e}")))?;
        if ["id", "type", "scope", "location", "@context"].contains(&self.attribute.as_str()) {
            return Err(bad(format!(
                "attribute: {:?} is not a Property a Channel can write",
                self.attribute
            )));
        }
        if !bridges.contains_key(&self.broker) {
            let mut names: Vec<&str> = bridges.keys().map(String::as_str).collect();
            names.sort_unstable();
            return Err(bad(format!(
                "broker: {:?} is not a bridge of this broker; it has {names:?} (ANTARES_CHANNEL_BROKERS)",
                self.broker
            )));
        }
        let t = &self.topic;
        if t.is_empty() || t.len() > MAX_TOPIC || t.starts_with('$') || t.contains(['+', '#', '\0'])
        {
            return Err(bad(format!(
                "topic: one concrete topic of 1 to {MAX_TOPIC} bytes, no wildcard (+, #) and no $ prefix, e.g. city/bus/12/speed"
            )));
        }
        let direction = self.direction.unwrap_or_else(|| "in".into());
        if direction != "in" {
            return Err(bad(format!(
                "direction: {direction:?} is not bound here; a Channel is inbound (in), and an Attribute's changes leave over MQTT through a Subscription whose notification endpoint is mqtt(s)://"
            )));
        }
        let max_rate = self.max_rate.unwrap_or(DEFAULT_RATE);
        if !(1..=MAX_RATE).contains(&max_rate) {
            return Err(bad(format!("maxRate: samples per second, 1 to {MAX_RATE}")));
        }
        let max_bytes = self.max_bytes.unwrap_or(DEFAULT_BYTES);
        if !(1..=MAX_BYTES).contains(&max_bytes) {
            return Err(bad(format!("maxBytes: payload bytes, 1 to {MAX_BYTES}")));
        }
        Ok(Channel {
            id: id.to_owned(),
            tenant: None,
            entity: self.entity,
            attribute: self.attribute,
            broker: self.broker,
            topic: self.topic,
            direction,
            retention: self.retention,
            max_rate,
            max_bytes,
            link: None,
        })
    }
}

/// What happened to a Channel's samples since it was bound in this process.
#[derive(Debug, Default)]
pub struct Stats {
    pub written: AtomicU64,
    pub refused: AtomicU64,
    pub too_large: AtomicU64,
    pub over_rate: AtomicU64,
    pub looped: AtomicU64,
}

impl Stats {
    fn json(&self) -> Value {
        let n = |a: &AtomicU64| a.load(Ordering::Relaxed);
        json!({
            "written": n(&self.written),
            "refused": n(&self.refused),
            "tooLarge": n(&self.too_large),
            "overRate": n(&self.over_rate),
            "looped": n(&self.looped),
        })
    }
}

struct Running {
    tasks: Vec<tokio::task::AbortHandle>,
    stats: Arc<Stats>,
}

impl Drop for Running {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// The Channels surface: `/x/channels`, the bridges it binds on, and the
/// Channels running in this process.
pub struct Channels {
    bridges: HashMap<String, Arc<dyn Bridge>>,
    running: Arc<Mutex<HashMap<String, Running>>>,
    restored: Arc<OnceLock<()>>,
}

impl Channels {
    #[must_use]
    pub fn new(bridges: HashMap<String, Arc<dyn Bridge>>) -> Self {
        Self {
            bridges,
            running: Arc::default(),
            restored: Arc::default(),
        }
    }

    /// The bridges `ANTARES_CHANNEL_BROKERS` names.
    ///
    /// # Errors
    /// [`bridges_from`]'s.
    pub fn from_env() -> Result<Self, String> {
        bridges_from(&std::env::var("ANTARES_CHANNEL_BROKERS").unwrap_or_default()).map(Self::new)
    }

    /// The counters of Channel `key` (`{tenant}:{id}`) while it runs here.
    #[must_use]
    pub fn stats(&self, key: &str) -> Option<Arc<Stats>> {
        lock(&self.running).get(key).map(|r| Arc::clone(&r.stats))
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone)]
struct Shared {
    bridges: Arc<HashMap<String, Arc<dyn Bridge>>>,
    running: Arc<Mutex<HashMap<String, Running>>>,
}

fn store_tenant() -> ApiResult<TenantId> {
    Ok(TenantId::new_internal(CHANNEL_TENANT)?)
}

fn key(tenant: &TenantId, id: &str) -> String {
    format!("{}:{id}", tenant.as_str())
}

impl crate::ApiSurface for Channels {
    fn name(&self) -> &str {
        "channels"
    }

    fn prefix(&self) -> &str {
        "/x/channels"
    }

    fn router(&self, st: AppState) -> Router<AppState> {
        let shared = Shared {
            bridges: Arc::new(self.bridges.clone()),
            running: Arc::clone(&self.running),
        };
        // An in-process call builds the router again; the stored Channels
        // start once per process.
        if self.restored.set(()).is_ok() {
            let (st, shared) = (st.clone(), shared.clone());
            tokio::spawn(async move { restore(&st, &shared).await });
        }
        let one = {
            let (a, b, c) = (shared.clone(), shared.clone(), shared.clone());
            get(move |st: State<AppState>, p: Path<String>, h: HeaderMap| {
                retrieve(st, a.clone(), p, h)
            })
            .put(
                move |st: State<AppState>,
                      p: Path<String>,
                      h: HeaderMap,
                      body: axum::body::Bytes| { bind(st, b.clone(), p, h, body) },
            )
            .delete(move |st: State<AppState>, p: Path<String>, h: HeaderMap| {
                unbind(st, c.clone(), p, h)
            })
        };
        let all = shared.clone();
        Router::new()
            .route(
                "/",
                get(move |st: State<AppState>, h: HeaderMap| list(st, all.clone(), h)),
            )
            .route("/{id}", one)
    }

    fn version_info(&self) -> Value {
        let mut names: Vec<&String> = self.bridges.keys().collect();
        names.sort_unstable();
        json!({ "adr": "ADR-0031", "bridges": names })
    }
}

/// The headers a Channel's in-process requests carry: its tenant, the
/// binder's `Link`, and the binder's subject.
fn request_headers(ch: &Channel, subject: &[(String, String)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    let mut put = |name: &str, value: &str| {
        if let (Ok(n), Ok(v)) = (
            axum::http::HeaderName::from_bytes(name.as_bytes()),
            axum::http::HeaderValue::from_str(value),
        ) {
            h.append(n, v);
        }
    };
    put("content-type", "application/json");
    if let Some(t) = &ch.tenant {
        put("ngsild-tenant", t);
    }
    if let Some(l) = &ch.link {
        put("link", l);
    }
    for (k, v) in subject {
        if k != crate::policy::VIEW_PAIR {
            put(k, v);
        }
    }
    h
}

/// 5.6.3 Append Attributes of the Channel's Attribute with `value`, in
/// process, as the binding subject.
async fn write(
    st: &AppState,
    ch: &Channel,
    subject: &[(String, String)],
    value: Value,
    observed_at: Option<String>,
) -> Response {
    let mut attr = serde_json::Map::new();
    attr.insert("type".into(), json!("Property"));
    attr.insert("value".into(), value);
    if let Some(t) = observed_at {
        attr.insert("observedAt".into(), Value::String(t));
    }
    let body = json!({ ch.attribute.clone(): attr });
    let uri = format!(
        "/ngsi-ld/v1/entities/{}/attrs",
        crate::federation::path_segment(&ch.entity)
    );
    let body = body.to_string();
    let mut headers = request_headers(ch, subject);
    headers.insert(axum::http::header::CONTENT_LENGTH, body.len().into());
    let mut req = axum::http::Request::post(uri)
        .body(axum::body::Body::from(body))
        .unwrap_or_default();
    *req.headers_mut() = headers;
    st.call(&HeaderMap::new(), req).await
}

/// What a sample's payload is to the Channel: a value to write, or why it
/// is dropped.
#[derive(Debug, PartialEq)]
enum Judged {
    Value(Value),
    TooLarge,
    Looped,
    Refused,
}

/// A payload is the Property's value: JSON when it parses as JSON, else its
/// UTF-8 text. An NGSI-LD Notification, bare or in its clause 7 message, is
/// this broker's own output on its way back in, and is dropped.
fn judge(payload: &[u8], max_bytes: u32) -> Judged {
    if payload.len() > max_bytes as usize {
        return Judged::TooLarge;
    }
    let value = match serde_json::from_slice::<Value>(payload) {
        Ok(v) => v,
        Err(_) => match std::str::from_utf8(payload) {
            Ok(s) if !s.is_empty() => Value::String(s.to_owned()),
            _ => return Judged::Refused,
        },
    };
    let is_notification = |v: &Value| v.get("type").and_then(Value::as_str) == Some("Notification");
    if is_notification(&value) || value.get("body").is_some_and(is_notification) {
        return Judged::Looped;
    }
    if value.is_null() {
        return Judged::Refused;
    }
    Judged::Value(value)
}

/// A per-second sample budget: a window of one second that admits
/// `max_rate` samples.
struct Budget {
    max: u32,
    start: Instant,
    used: u32,
}

impl Budget {
    fn admits(&mut self, now: Instant) -> bool {
        if now.duration_since(self.start) >= Duration::from_secs(1) {
            self.start = now;
            self.used = 0;
        }
        if self.used >= self.max {
            return false;
        }
        self.used = self.used.saturating_add(1);
        true
    }
}

/// Start Channel `ch` on its bridge. A bind (`replace`) replaces a running
/// one of the same key; a restore leaves one a bind already started alone.
async fn start(
    st: &AppState,
    shared: &Shared,
    key: String,
    ch: Channel,
    subject: Vec<(String, String)>,
    replace: bool,
) -> Result<(), String> {
    let bridge = shared
        .bridges
        .get(&ch.broker)
        .cloned()
        .ok_or_else(|| format!("bridge {:?} is not configured", ch.broker))?;
    // the old one stops before the new one subscribes
    if replace {
        lock(&shared.running).remove(&key);
    } else if lock(&shared.running).contains_key(&key) {
        return Ok(());
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Sample>(QUEUE);
    let feed = bridge.open(&ch.topic, tx).await?;
    let stats = Arc::new(Stats::default());
    let counters = Arc::clone(&stats);
    let st = st.clone();
    let worker = tokio::spawn(async move {
        let mut budget = Budget {
            max: ch.max_rate,
            start: Instant::now(),
            used: 0,
        };
        while let Some(sample) = rx.recv().await {
            let value = match judge(&sample.payload, ch.max_bytes) {
                Judged::Value(v) => v,
                Judged::TooLarge => {
                    counters.too_large.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                Judged::Looped => {
                    counters.looped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                Judged::Refused => {
                    counters.refused.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            };
            if !budget.admits(Instant::now()) {
                counters.over_rate.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let at = Some(crate::state::now_iso());
            let resp = match ch.retention {
                Retention::Mirror => write(&st, &ch, &subject, value, at).await,
                Retention::Relay => {
                    INTENT
                        .scope(Intent::Relay, write(&st, &ch, &subject, value, at))
                        .await
                }
            };
            if resp.status().is_success() {
                counters.written.fetch_add(1, Ordering::Relaxed);
            } else {
                counters.refused.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    channel = %ch.id, status = %resp.status(), "channel sample refused"
                );
            }
        }
    });
    let mut tasks = vec![worker.abort_handle()];
    tasks.extend(feed);
    let run = Running { tasks, stats };
    let mut running = lock(&shared.running);
    // a restore that lost the race to a bind drops its own (and its tasks)
    if replace || !running.contains_key(&key) {
        running.insert(key, run);
    }
    Ok(())
}

/// Start every stored Channel; one its bridge refuses is logged and skipped.
async fn restore(st: &AppState, shared: &Shared) {
    let Ok(t) = store_tenant() else { return };
    let docs = match st.store.list(&t, Kind::View).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("channels: listing the stored channels failed: {e}");
            return;
        }
    };
    for doc in docs {
        let Ok(ch) = serde_json::from_value::<Channel>(doc.clone()) else {
            continue;
        };
        let Some(tenant) = stored_tenant(&doc) else {
            continue;
        };
        let subject = crate::policy::stored_subject(&tenant, &doc).headers;
        let k = key(&tenant, &ch.id);
        let (st, shared) = (st.clone(), shared.clone());
        tokio::spawn(async move {
            keep_starting(&st, &shared, k, doc, ch, subject, RESTORE_RETRY).await;
        });
    }
}

/// How long a restore waits before it tries a refused bridge again.
const RESTORE_RETRY: Duration = Duration::from_secs(30);

/// Start a restored Channel, again every `every` while its bridge refuses,
/// for as long as the stored View is still `doc`: a Channel deleted or
/// re-bound meanwhile is not this restore's to start (its delete stopped it,
/// its new bind started its own).
async fn keep_starting(
    st: &AppState,
    shared: &Shared,
    k: String,
    doc: Value,
    ch: Channel,
    subject: Vec<(String, String)>,
    every: Duration,
) {
    let Ok(t) = store_tenant() else { return };
    loop {
        match start(st, shared, k.clone(), ch.clone(), subject.clone(), false).await {
            Ok(()) => return,
            Err(e) => tracing::warn!(
                "channels: channel {k} did not start, retrying in {} s: {e}",
                every.as_secs()
            ),
        }
        tokio::time::sleep(every).await;
        match st.store.get(&t, Kind::View, &k).await {
            Ok(Some(now)) if now == doc => {}
            Ok(_) => return,
            // a store that cannot answer now may answer at the next try
            Err(e) => tracing::warn!("channels: reading channel {k} failed: {e}"),
        }
    }
}

/// The tenant a Channel request addresses, as the client named it.
fn tenant_of(headers: &HeaderMap) -> ApiResult<(TenantId, Option<String>)> {
    let tenant = tenant_from(headers)?;
    let named = headers
        .get("ngsild-tenant")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    Ok((tenant, named))
}

/// The stored Channel `id` of this tenant, when this subject bound it: a
/// Channel another subject bound answers as an absent one.
async fn owned(
    st: &AppState,
    tenant: &TenantId,
    headers: &HeaderMap,
    id: &str,
) -> ApiResult<(String, Value)> {
    let k = key(tenant, id);
    let doc = st
        .store
        .get(&store_tenant()?, Kind::View, &k)
        .await?
        .filter(|d| crate::policy::belongs_to(d, &crate::policy::subject_of(tenant, headers)))
        .ok_or_else(|| NgsiError::ResourceNotFound(format!("channel {id} not found")))?;
    Ok((k, doc))
}

fn served(mut doc: Value, stats: Option<Arc<Stats>>) -> Value {
    crate::policy::strip_internal(&mut doc);
    if let Some(o) = doc.as_object_mut() {
        o.remove("link");
        o.insert(
            "status".into(),
            match stats {
                Some(s) => json!({ "running": true, "samples": s.json() }),
                None => json!({ "running": false }),
            },
        );
    }
    doc
}

fn answer(r: ApiResult<Response>) -> Response {
    r.unwrap_or_else(IntoResponse::into_response)
}

async fn bind(
    State(st): State<AppState>,
    shared: Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    answer(bind_inner(&st, &shared, &id, &headers, &body).await)
}

async fn bind_inner(
    st: &AppState,
    shared: &Shared,
    id: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> ApiResult<Response> {
    let (tenant, named) = tenant_of(headers)?;
    let spec: Spec = serde_json::from_slice(body).map_err(|e| bad(format!("channel: {e}")))?;
    let mut ch = spec.check(id, &shared.bridges)?;
    ch.tenant = named;
    ch.link = headers
        .get("link")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let subject = crate::policy::subject_of(&tenant, headers);
    // The bind is the write it stands for, decided by the policy seam as
    // this subject: refused here, refused for every sample.
    let probe = INTENT
        .scope(
            Intent::Bind,
            write(st, &ch, &subject.headers, json!(0), None),
        )
        .await;
    if !probe.status().is_success() {
        return Ok(probe);
    }
    let k = key(&tenant, id);
    let store = store_tenant()?;
    let old = st.store.get(&store, Kind::View, &k).await?;
    if let Some(old) = &old {
        if !crate::policy::belongs_to(old, &subject) {
            return Err(NgsiError::AlreadyExists(format!(
                "channel {id} is bound by another subject"
            ))
            .into());
        }
    }
    let mut doc = serde_json::to_value(&ch).map_err(|e| NgsiError::InternalError(e.to_string()))?;
    if let (Some(o), Some(s)) = (doc.as_object_mut(), crate::policy::subject_member(&subject)) {
        o.insert(crate::policy::SUBJECT_MEMBER.into(), s);
    }
    // stored before it replaces the running Channel: a refused write leaves
    // the old binding running and stored (F-ANT3-011); a bridge that refuses
    // the new one puts the old document back, so nothing new is bound
    let replaced = st.store.upsert(&store, Kind::View, &k, doc).await?;
    if let Err(e) = start(st, shared, k.clone(), ch, subject.headers, true).await {
        match old {
            Some(old) => st.store.upsert(&store, Kind::View, &k, old).await.map(drop),
            None => st.store.delete(&store, Kind::View, &k).await.map(drop),
        }?;
        return Err(NgsiError::InternalError(format!("channel {id} is not bound: {e}")).into());
    }
    let mut resp = if replaced {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::CREATED.into_response()
    };
    if !replaced {
        if let Ok(v) = axum::http::HeaderValue::from_str(&format!("/x/channels/{id}")) {
            resp.headers_mut().insert(axum::http::header::LOCATION, v);
        }
    }
    Ok(resp)
}

async fn retrieve(
    State(st): State<AppState>,
    shared: Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    answer(
        async {
            let (tenant, _) = tenant_of(&headers)?;
            let (k, doc) = owned(&st, &tenant, &headers, &id).await?;
            let stats = lock(&shared.running).get(&k).map(|r| Arc::clone(&r.stats));
            Ok(Json(served(doc, stats)).into_response())
        }
        .await,
    )
}

async fn unbind(
    State(st): State<AppState>,
    shared: Shared,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    answer(
        async {
            let (tenant, _) = tenant_of(&headers)?;
            let (k, _) = owned(&st, &tenant, &headers, &id).await?;
            lock(&shared.running).remove(&k);
            st.store.delete(&store_tenant()?, Kind::View, &k).await?;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        .await,
    )
}

async fn list(State(st): State<AppState>, shared: Shared, headers: HeaderMap) -> Response {
    answer(
        async {
            let (tenant, _) = tenant_of(&headers)?;
            let subject = crate::policy::subject_of(&tenant, &headers);
            let prefix = key(&tenant, "");
            let docs = st.store.list(&store_tenant()?, Kind::View).await?;
            let running = lock(&shared.running);
            let out: Vec<Value> = docs
                .into_iter()
                .filter(|d| crate::policy::belongs_to(d, &subject))
                .filter_map(|d| {
                    let id = d.get("id")?.as_str()?.to_owned();
                    let k = format!("{prefix}{id}");
                    (stored_tenant(&d).as_ref() == Some(&tenant))
                        .then(|| served(d, running.get(&k).map(|r| Arc::clone(&r.stats))))
                })
                .collect();
            Ok(Json(out).into_response())
        }
        .await,
    )
}

/// The tenant a stored Channel document belongs to; `None` for one that
/// names no valid tenant.
fn stored_tenant(doc: &Value) -> Option<TenantId> {
    match doc.get("tenant").and_then(Value::as_str) {
        None => Some(TenantId::default()),
        Some(t) => TenantId::new_internal(t).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_is_json_or_text_and_a_notification_is_looped() {
        assert_eq!(judge(b"42.5", 64), Judged::Value(json!(42.5)));
        assert_eq!(judge(b"open", 64), Judged::Value(json!("open")));
        assert_eq!(judge(b"null", 64), Judged::Refused);
        assert_eq!(judge(b"", 64), Judged::Refused);
        assert_eq!(judge(&[0xff, 0xfe], 64), Judged::Refused);
        assert_eq!(judge(b"12345", 4), Judged::TooLarge);
        assert_eq!(
            judge(br#"{"type":"Notification","data":[]}"#, 256),
            Judged::Looped
        );
        assert_eq!(
            judge(br#"{"metadata":{},"body":{"type":"Notification"}}"#, 256),
            Judged::Looped
        );
        assert_eq!(
            judge(br#"{"speed":3}"#, 256),
            Judged::Value(json!({"speed": 3}))
        );
    }

    #[test]
    fn the_budget_admits_max_rate_per_second() {
        let t0 = Instant::now();
        let mut b = Budget {
            max: 2,
            start: t0,
            used: 0,
        };
        assert!(b.admits(t0));
        assert!(b.admits(t0));
        assert!(!b.admits(t0));
        assert!(b.admits(t0 + Duration::from_secs(1)));
    }

    /// A bridge that refuses its first `n` opens.
    struct Flaky(std::sync::atomic::AtomicUsize);

    impl Bridge for Flaky {
        fn open<'a>(&'a self, _: &'a str, _: tokio::sync::mpsc::Sender<Sample>) -> Opening<'a> {
            Box::pin(async move {
                let left = self.0.load(Ordering::SeqCst);
                if left == 0 {
                    return Ok(None);
                }
                self.0.store(left.saturating_sub(1), Ordering::SeqCst);
                Err("bridge down".to_owned())
            })
        }
    }

    /// F-ANT3-012: a restore retrying a refused bridge stops once the
    /// Channel is deleted, so the bridge coming back starts nothing.
    #[tokio::test]
    async fn a_restore_retry_stops_once_the_channel_is_gone() {
        let st = AppState::new("antares-channels-retry".into());
        let flaky: Arc<dyn Bridge> = Arc::new(Flaky(std::sync::atomic::AtomicUsize::new(3)));
        let shared = Shared {
            bridges: Arc::new(HashMap::from([("lo".to_owned(), flaky)])),
            running: Arc::default(),
        };
        let ch = Channel {
            id: "bus-1-speed".into(),
            tenant: None,
            entity: "urn:ngsi-ld:Bus:1".into(),
            attribute: "speed".into(),
            broker: "lo".into(),
            topic: "bus/1/speed".into(),
            direction: "in".into(),
            retention: Retention::Mirror,
            max_rate: 10,
            max_bytes: 1024,
            link: None,
        };
        let k = key(&TenantId::default(), &ch.id);
        let doc = serde_json::to_value(&ch).expect("channel doc");
        let t = store_tenant().expect("store tenant");
        st.store
            .create(&t, Kind::View, &k, doc.clone())
            .await
            .expect("seed");
        let retry = {
            let (st, shared, k, doc) = (st.clone(), shared.clone(), k.clone(), doc.clone());
            tokio::spawn(async move {
                keep_starting(
                    &st,
                    &shared,
                    k,
                    doc,
                    ch,
                    Vec::new(),
                    Duration::from_millis(20),
                )
                .await;
            })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(st.store.delete(&t, Kind::View, &k).await.expect("delete"));
        tokio::time::timeout(Duration::from_secs(5), retry)
            .await
            .expect("the retry never stopped")
            .expect("retry task");
        assert!(
            !lock(&shared.running).contains_key(&k),
            "a deleted Channel was started by its restore"
        );
    }

    #[test]
    fn bridges_parse_names_and_refuse_repeats() {
        let b = bridges_from("lo = loopback, lo2=loopback").map(|m| m.len());
        assert_eq!(b, Ok(2));
        assert!(bridges_from("lo=loopback,lo=loopback").is_err());
        assert!(bridges_from("bad name=loopback").is_err());
        assert!(bridges_from("loopback").is_err());
        assert_eq!(bridges_from("").map(|m| m.len()), Ok(0));
    }
}
