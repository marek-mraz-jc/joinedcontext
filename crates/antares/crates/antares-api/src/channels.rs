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
use crate::policy::Subject;
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The reserved Tenant Channel documents lived under, keyed `{tenant}:{id}`,
/// before each moved to its own (S4-16). A start moves what is left there
/// ([`move_leftovers`]); the name stays reserved, so no client ever owns it.
const LEFTOVER_TENANT: &str = "antares-channels";

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
    tenant: TenantId,
    id: String,
    /// The document it runs; once the stored one differs, it is stale.
    doc: Arc<Value>,
    /// Whether the last [`tend`] could read it; no sample is written else.
    fresh: Arc<AtomicBool>,
}

/// A stored Channel as it is started.
#[derive(Clone)]
struct Bind {
    /// The Tenant whose row it is, whatever the document says.
    tenant: TenantId,
    doc: Arc<Value>,
    ch: Channel,
    subject: Vec<(String, String)>,
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
            // a purged Tenant's Channels stop here (S4-30)
            let running = Arc::clone(&self.running);
            st.purge_hooks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Arc::new(move |tenant: &TenantId| {
                    let prefix = key(tenant, "");
                    lock(&running).retain(|k, _| !k.starts_with(&prefix));
                }));
            let (st, shared) = (st.clone(), shared.clone());
            tokio::spawn(async move {
                restore(&st, &shared).await;
                loop {
                    tokio::time::sleep(TEND_EVERY).await;
                    tend(&st, &shared).await;
                }
            });
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
            // EP-188: the bind door and every read of a binding
            .layer(axum::middleware::from_fn_with_state(
                st.clone(),
                crate::admin_auth::existing_tenant_layer,
            ))
            // T-5032: the API's own door, so a caller nobody vouches for
            // reads and binds no Channel (S4-16)
            .layer(axum::middleware::from_fn_with_state(
                st,
                crate::admin_auth::subject_or_admin,
            ))
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
    crate::policy::as_identity(st.call(&HeaderMap::new(), req)).await
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
async fn start(st: &AppState, shared: &Shared, bound: Bind, replace: bool) -> Result<(), String> {
    let Bind {
        tenant,
        doc,
        ch,
        subject,
    } = bound;
    let key = key(&tenant, &ch.id);
    let id = ch.id.clone();
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
    if replica_full(shared, &key) {
        return Err(refused_by_replica());
    }
    let fresh = Arc::new(AtomicBool::new(true));
    let readable = Arc::clone(&fresh);
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
            if !readable.load(Ordering::Relaxed) {
                counters.refused.fetch_add(1, Ordering::Relaxed);
                continue;
            }
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
    let run = Running {
        tasks,
        stats,
        tenant,
        id,
        doc,
        fresh,
    };
    let mut running = lock(&shared.running);
    // a restore that lost the race to a bind drops its own (and its tasks)
    if replace || !running.contains_key(&key) {
        running.insert(key, run);
    }
    Ok(())
}

/// Delete the Channels of `tenant` still waiting to be moved to it (S4-30):
/// its own go with the store's purge, and the purge hook the router
/// registered stops the running ones.
///
/// # Errors
/// The store's.
pub(crate) async fn purge_tenant(st: &AppState, tenant: &TenantId) -> Result<(), NgsiError> {
    let store = TenantId::new_internal(LEFTOVER_TENANT)?;
    let mut keys = Vec::new();
    crate::csource::walk_docs(st, &store, Kind::View, |doc| {
        if stored_tenant(&doc).as_ref() == Some(tenant) {
            if let Some(id) = doc.get("id").and_then(Value::as_str) {
                keys.push(key(tenant, id));
            }
        }
        Ok(())
    })
    .await?;
    for k in keys {
        st.store.delete(&store, Kind::View, &k).await?;
    }
    Ok(())
}

/// Whether a Channel of `tenant` (as its binder named it) may bind `topic`
/// (T-7306). A named Tenant binds only topics under `{tenant}/`, so on a
/// bridge every Tenant shares no Tenant receives another's messages; a
/// request naming no Tenant binds no topic whose first level is a Tenant of
/// an Organization.
fn topic_in_namespace(tenant: Option<&str>, topic: &str) -> bool {
    match tenant {
        Some(t) => topic
            .strip_prefix(t)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|rest| !rest.is_empty()),
        None => {
            let first = topic.split('/').next().unwrap_or(topic);
            TenantId::organization_of(first).is_none()
        }
    }
}

/// Whether this process runs as many Channels as it may and `key` is none
/// of them.
fn replica_full(shared: &Shared, key: &str) -> bool {
    let running = lock(&shared.running);
    !running.contains_key(key) && running.len() >= *crate::bounds::CHANNELS_MAX
}

/// What a Channel past [`crate::bounds::CHANNELS_MAX`] is told.
fn refused_by_replica() -> String {
    format!(
        "this broker runs {} Channels, the most it may (ANTARES_CHANNELS_MAX): unbind one, or ask the operator to raise it",
        *crate::bounds::CHANNELS_MAX
    )
}

fn count_refused(cap: &'static str) {
    metrics::counter!("antares_tenant_cap_refused_total", "cap" => cap).increment(1);
}

/// The Channel a row of `tenant` holds. The Tenant is the row's: a rename
/// moves the row and leaves the document's own `tenant` as it was bound.
fn channel_of(tenant: &TenantId, doc: &Value) -> Option<Channel> {
    let mut ch: Channel = serde_json::from_value(doc.clone()).ok()?;
    ch.tenant = (*tenant != TenantId::default()).then(|| tenant.as_str().to_owned());
    Some(ch)
}

/// Every Channel document of `tenant`, read through the Tenant's own scope.
async fn stored(st: &AppState, tenant: &TenantId) -> Result<Vec<Value>, NgsiError> {
    let mut docs = Vec::new();
    crate::csource::walk_docs(st, tenant, Kind::View, |doc| {
        docs.push(doc);
        Ok(())
    })
    .await?;
    Ok(docs)
}

/// Move the documents left in [`LEFTOVER_TENANT`] to their Tenants. Safe to
/// repeat and on every replica: a Channel the Tenant holds already wins, and
/// the leftover goes only after the move is stored.
async fn move_leftovers(st: &AppState) -> Result<(), NgsiError> {
    let from = TenantId::new_internal(LEFTOVER_TENANT)?;
    for doc in st.store.list(&from, Kind::View).await? {
        let (Some(tenant), Some(id)) = (
            stored_tenant(&doc),
            doc.get("id").and_then(Value::as_str).map(str::to_owned),
        ) else {
            continue;
        };
        // a renamed-away Tenant's is refused by the store and waits where it is
        if let Err(e) = st.store.create(&tenant, Kind::View, &id, doc).await {
            tracing::warn!(
                channel = %id, tenant = tenant.as_str(),
                "channels: a stored channel was not moved to its Tenant: {e}"
            );
            continue;
        }
        st.store
            .delete(&from, Kind::View, &key(&tenant, &id))
            .await?;
    }
    Ok(())
}

/// Start every stored Channel; one its bridge refuses is retried, one past
/// the process's cap is logged and left stopped.
async fn restore(st: &AppState, shared: &Shared) {
    if let Err(e) = move_leftovers(st).await {
        tracing::error!("channels: moving the stored channels to their Tenants failed: {e}");
    }
    let names = match st.store.tenant_ids().await {
        Ok(names) => names,
        Err(e) => {
            tracing::error!("channels: listing the Tenants failed: {e}");
            return;
        }
    };
    let mut started = 0usize;
    for name in names {
        let Ok(tenant) = TenantId::new_internal(&name) else {
            continue;
        };
        let docs = match stored(st, &tenant).await {
            Ok(docs) if docs.is_empty() => continue,
            Ok(docs) => docs,
            Err(e) => {
                tracing::error!(
                    tenant = tenant.as_str(),
                    "channels: listing the stored channels failed: {e}"
                );
                continue;
            }
        };
        // EP-188: the replay acts for the stored Tenant, which no caller
        // names now: its shape, tombstone and stored Organization decide.
        if crate::admin_auth::tenant_door(st, Some(tenant.as_str()), &HeaderMap::new(), true)
            .await
            .is_some()
        {
            tracing::warn!(
                tenant = tenant.as_str(),
                "channels: a Tenant with stored channels is refused by the Organization check (EP-188): not restored"
            );
            continue;
        }
        for doc in docs {
            let Some(ch) = channel_of(&tenant, &doc) else {
                continue;
            };
            // a binding stored before the topic namespace (T-7306) that reaches
            // outside it stays stored and stopped until its binder binds again
            if !topic_in_namespace(ch.tenant.as_deref(), &ch.topic) {
                tracing::warn!(
                    channel = %ch.id, tenant = tenant.as_str(),
                    "channels: a stored channel's topic is outside its Tenant's namespace: not restored; bind it again under {}/",
                    tenant.as_str()
                );
                continue;
            }
            // the cap bounds the retrying tasks as well as the running ones
            if started >= *crate::bounds::CHANNELS_MAX {
                count_refused("channels_replica");
                tracing::error!(
                    channel = %ch.id, tenant = tenant.as_str(),
                    "channels: not restored: {}", refused_by_replica()
                );
                continue;
            }
            started = started.saturating_add(1);
            let Some(subject) = acting(st, &crate::policy::stored_subject(&tenant, &doc)) else {
                tracing::warn!(
                    channel = %ch.id, tenant = tenant.as_str(),
                    "channels: a stored channel names nobody it can act as (only roles, groups or nothing): not restored; its binder binds it again as a named person or client"
                );
                continue;
            };
            let bound = Bind {
                subject,
                tenant: tenant.clone(),
                doc: Arc::new(doc),
                ch,
            };
            let (st, shared) = (st.clone(), shared.clone());
            tokio::spawn(async move {
                keep_starting(&st, &shared, bound, RESTORE_RETRY).await;
            });
        }
    }
}

/// How often the running Channels are held to what is stored.
const TEND_EVERY: Duration = Duration::from_secs(5);

/// Stop the Channels here whose document another replica deleted, replaced
/// or purged, and hold the samples of those that cannot be read (S4-16).
/// One read per Tenant with a Channel running here.
async fn tend(st: &AppState, shared: &Shared) {
    // taken before the read: a bind made during it is not judged by it
    let mut held: HashMap<TenantId, Vec<(String, String, Arc<Value>)>> = HashMap::new();
    for (k, run) in lock(&shared.running).iter() {
        held.entry(run.tenant.clone()).or_default().push((
            k.clone(),
            run.id.clone(),
            Arc::clone(&run.doc),
        ));
    }
    for (tenant, runs) in held {
        let read = stored(st, &tenant).await;
        let mut running = lock(&shared.running);
        for (k, id, doc) in runs {
            let Some(run) = running.get(&k).filter(|run| Arc::ptr_eq(&run.doc, &doc)) else {
                continue;
            };
            match &read {
                Err(e) => {
                    run.fresh.store(false, Ordering::Relaxed);
                    tracing::warn!("channels: channel {k} is unreadable, its samples wait: {e}");
                }
                Ok(docs) => {
                    let now = docs
                        .iter()
                        .find(|d| d.get("id").and_then(Value::as_str) == Some(id.as_str()));
                    if now == Some(&*doc) {
                        run.fresh.store(true, Ordering::Relaxed);
                    } else {
                        running.remove(&k);
                    }
                }
            }
        }
    }
}

/// How long a restore waits before it tries a refused bridge again.
const RESTORE_RETRY: Duration = Duration::from_secs(30);

/// Start a restored Channel, again every `every` while its bridge refuses,
/// for as long as the stored document is still the one it read: a Channel
/// deleted or re-bound meanwhile is not this restore's to start (its delete
/// stopped it, its new bind started its own).
async fn keep_starting(st: &AppState, shared: &Shared, bound: Bind, every: Duration) {
    let k = key(&bound.tenant, &bound.ch.id);
    loop {
        match start(st, shared, bound.clone(), false).await {
            Ok(()) => return,
            Err(e) => tracing::warn!(
                "channels: channel {k} did not start, retrying in {} s: {e}",
                every.as_secs()
            ),
        }
        tokio::time::sleep(every).await;
        match st.store.get(&bound.tenant, Kind::View, &bound.ch.id).await {
            Ok(Some(now)) if now == *bound.doc => {}
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
/// Who a Channel of `subject` acts as: the identity the engine names and
/// nothing else it held (S4-17). Without an engine the subject decides
/// nothing, so the write carries none, unless the API requires one.
fn acting(st: &AppState, subject: &Subject) -> Option<Vec<(String, String)>> {
    match &st.policy {
        Some(engine) => crate::policy::identity(engine.as_ref(), subject),
        None => (!st.subject_required).then(Vec::new),
    }
}

/// Whether `caller` is who bound `doc`: the same identity, whatever either
/// holds; the same subject where the document names no identity.
fn owns(st: &AppState, tenant: &TenantId, doc: &Value, caller: &Subject) -> bool {
    let who = |subject: &Subject| {
        st.policy
            .as_ref()
            .and_then(|engine| crate::policy::identity(engine.as_ref(), subject))
    };
    match who(&crate::policy::stored_subject(tenant, doc)) {
        Some(binder) => who(caller) == Some(binder),
        None => crate::policy::belongs_to(doc, caller),
    }
}

async fn owned(
    st: &AppState,
    tenant: &TenantId,
    headers: &HeaderMap,
    id: &str,
) -> ApiResult<(String, Value)> {
    let k = key(tenant, id);
    let doc = st
        .store
        .get(tenant, Kind::View, id)
        .await?
        .filter(|d| owns(st, tenant, d, &crate::policy::subject_of(tenant, headers)))
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
    if !topic_in_namespace(named.as_deref(), &ch.topic) {
        return Err(bad(match &named {
            Some(t) => format!(
                "topic: a Channel of Tenant {t} binds topics under {t}/ only, e.g. {t}/bus/12/speed"
            ),
            None => "topic: its first level names a Tenant of an Organization; bind it under that Tenant's NGSILD-Tenant".to_owned(),
        }));
    }
    ch.tenant = named;
    ch.link = headers
        .get("link")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let subject = crate::policy::subject_of(&tenant, headers);
    let Some(acts_as) = acting(st, &subject) else {
        return Err(NgsiError::OperationNotSupported(
            "a Channel writes later as who bound it, and this caller names no person or client: a role, a group or a membership alone cannot bind one; bind it signed in, or as a client".to_owned(),
        )
        .into());
    };
    // The bind is the write it stands for, decided by the policy seam as
    // the identity each sample carries: refused here, refused for every sample.
    let probe = INTENT
        .scope(Intent::Bind, write(st, &ch, &acts_as, json!(0), None))
        .await;
    if !probe.status().is_success() {
        return Ok(probe);
    }
    let k = key(&tenant, id);
    let old = st.store.get(&tenant, Kind::View, id).await?;
    if let Some(old) = &old {
        if !owns(st, &tenant, old, &subject) {
            return Err(NgsiError::AlreadyExists(format!(
                "channel {id} is bound by another subject"
            ))
            .into());
        }
    }
    if replica_full(shared, &k) {
        count_refused("channels_replica");
        return Err(NgsiError::TooManyResults(refused_by_replica()).into());
    }
    let mut doc = serde_json::to_value(&ch).map_err(|e| NgsiError::InternalError(e.to_string()))?;
    // with an engine the identity alone is kept; without one the subject,
    // which then tells owners apart and is never acted as
    let kept = Subject {
        tenant: tenant.clone(),
        headers: if st.policy.is_some() {
            acts_as.clone()
        } else {
            subject.headers.clone()
        },
    };
    if let (Some(o), Some(s)) = (doc.as_object_mut(), crate::policy::subject_member(&kept)) {
        o.insert(crate::policy::SUBJECT_MEMBER.into(), s);
    }
    // stored before it replaces the running Channel: a refused write leaves
    // the old binding running and stored (F-ANT3-011); a bridge that refuses
    // the new one puts the old document back, so nothing new is bound
    let replaced = st
        .store
        .upsert(&tenant, Kind::View, id, doc.clone())
        .await?;
    if !replaced {
        // counted with this one stored, so two replicas cannot both take the
        // last place: at worst both are refused and one binds again
        let held = stored(st, &tenant).await;
        let cap = *crate::bounds::CHANNELS_PER_TENANT;
        if !matches!(&held, Ok(docs) if docs.len() <= cap) {
            st.store.delete(&tenant, Kind::View, id).await?;
            held?;
            count_refused("channels");
            return Err(NgsiError::TooManyResults(format!(
                "this Tenant holds {cap} Channels, the most this broker keeps for one (ANTARES_CHANNELS_PER_TENANT): unbind one, or ask the operator to raise it"
            ))
            .into());
        }
    }
    let bound = Bind {
        tenant: tenant.clone(),
        doc: Arc::new(doc),
        ch,
        subject: acts_as,
    };
    if let Err(e) = start(st, shared, bound, true).await {
        match old {
            Some(old) => st
                .store
                .upsert(&tenant, Kind::View, id, old)
                .await
                .map(drop),
            None => st.store.delete(&tenant, Kind::View, id).await.map(drop),
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
            st.store.delete(&tenant, Kind::View, &id).await?;
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
            let docs = stored(&st, &tenant).await?;
            let running = lock(&shared.running);
            let out: Vec<Value> = docs
                .into_iter()
                .filter(|d| owns(&st, &tenant, d, &subject))
                .filter_map(|d| {
                    let id = d.get("id")?.as_str()?.to_owned();
                    let k = format!("{prefix}{id}");
                    Some(served(d, running.get(&k).map(|r| Arc::clone(&r.stats))))
                })
                .collect();
            Ok(Json(out).into_response())
        }
        .await,
    )
}

/// The Tenant a document left in [`LEFTOVER_TENANT`] names; `None` for one
/// that names no valid one. A moved document's Tenant is its row's.
fn stored_tenant(doc: &Value) -> Option<TenantId> {
    match doc.get("tenant").and_then(Value::as_str) {
        None => Some(TenantId::default()),
        Some(t) => TenantId::new_internal(t).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-7306: a Tenant binds topics under its own name only, and a request
    /// naming no Tenant binds no topic of an Organization's Tenant.
    #[test]
    fn a_tenant_binds_topics_under_its_own_name_only() {
        assert!(topic_in_namespace(Some("sk_b__x"), "sk_b__x/bus/1/speed"));
        for outside in [
            "sk_a__x/bus/1",
            "bus/1/speed",
            "sk_b__x",
            "sk_b__x/",
            "sk_b__xy/bus",
        ] {
            assert!(!topic_in_namespace(Some("sk_b__x"), outside), "{outside}");
        }
        assert!(topic_in_namespace(None, "bus/1/speed"));
        assert!(topic_in_namespace(None, "city"));
        assert!(!topic_in_namespace(None, "sk_b__x/bus/1"));
        assert!(!topic_in_namespace(None, "sk_b__x"));
    }

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
        let id = ch.id.clone();
        let doc = serde_json::to_value(&ch).expect("channel doc");
        let t = TenantId::default();
        st.store
            .create(&t, Kind::View, &id, doc.clone())
            .await
            .expect("seed");
        let bound = Bind {
            tenant: t.clone(),
            doc: Arc::new(doc),
            ch,
            subject: Vec::new(),
        };
        let retry = {
            let (st, shared) = (st.clone(), shared.clone());
            tokio::spawn(async move {
                keep_starting(&st, &shared, bound, Duration::from_millis(20)).await;
            })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(st.store.delete(&t, Kind::View, &id).await.expect("delete"));
        tokio::time::timeout(Duration::from_secs(5), retry)
            .await
            .expect("the retry never stopped")
            .expect("retry task");
        assert!(
            !lock(&shared.running).contains_key(&k),
            "a deleted Channel was started by its restore"
        );
    }

    /// S4-16: a document of the former shared Tenant moves to the Tenant it
    /// names, once, and never over the Channel that Tenant holds already.
    #[tokio::test]
    async fn leftovers_move_to_their_tenants_and_never_over_a_newer_channel() {
        let st = AppState::new("antares-channels-move".into());
        let from = TenantId::new_internal(LEFTOVER_TENANT).expect("tenant");
        let city = TenantId::new("city").expect("tenant");
        for id in ["moved", "newer"] {
            let doc = json!({"id": id, "tenant": "city", "topic": "city/old"});
            st.store
                .create(&from, Kind::View, &key(&city, id), doc)
                .await
                .expect("leftover");
        }
        let newer = json!({"id": "newer", "tenant": "city", "topic": "city/new"});
        st.store
            .create(&city, Kind::View, "newer", newer.clone())
            .await
            .expect("newer");
        for _ in 0..2 {
            move_leftovers(&st).await.expect("move");
            let get = |id| st.store.get(&city, Kind::View, id);
            assert!(get("moved").await.expect("read").is_some());
            assert_eq!(get("newer").await.expect("read"), Some(newer.clone()));
            assert!(st
                .store
                .list(&from, Kind::View)
                .await
                .expect("list")
                .is_empty());
        }
    }

    /// S4-16, two replicas: a Channel another one deleted or re-bound stops
    /// here at the next check, and an unchanged one runs on.
    #[tokio::test]
    async fn a_channel_changed_on_another_replica_stops_here() {
        let st = AppState::new("antares-channels-tend".into());
        let shared = Shared {
            bridges: Arc::default(),
            running: Arc::default(),
        };
        let city = TenantId::new("city").expect("tenant");
        for id in ["same", "deleted", "rebound"] {
            let doc = json!({"id": id, "topic": "city/a"});
            st.store
                .create(&city, Kind::View, id, doc.clone())
                .await
                .expect("seed");
            let run = Running {
                tasks: Vec::new(),
                stats: Arc::default(),
                tenant: city.clone(),
                id: id.to_owned(),
                doc: Arc::new(doc),
                fresh: Arc::new(AtomicBool::new(false)),
            };
            lock(&shared.running).insert(key(&city, id), run);
        }
        st.store
            .delete(&city, Kind::View, "deleted")
            .await
            .expect("delete");
        let rebound = json!({"id": "rebound", "topic": "city/b"});
        st.store
            .upsert(&city, Kind::View, "rebound", rebound)
            .await
            .expect("rebind");
        tend(&st, &shared).await;
        let running = lock(&shared.running);
        assert_eq!(running.len(), 1, "{:?}", running.keys());
        let same = running
            .get(&key(&city, "same"))
            .expect("the unchanged one runs");
        assert!(
            same.fresh.load(Ordering::Relaxed),
            "a readable one takes samples"
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
