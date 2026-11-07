// SPDX-License-Identifier: EUPL-1.2
//! The `SensorThings` MQTT extension of a view (OGC 18-088 section 14, the receive side), MQTT
//! 3.1.1 over a WebSocket at `/x/views/{view}/sta/mqtt` (T-5911, `docs/src/sta.md`).
//!
//! - Every topic a client subscribes to becomes one NGSI-LD Subscription made through the view
//!   as the caller, with this connection as its endpoint: the notifier feeds it, nothing polls.
//! - A notification re-reads the topic's resource through the broker's own router as the caller:
//!   every message is the HTTP read of that resource byte for byte, so the view and the policy
//!   decide what it holds exactly as they decide a GET.
//! - A topic whose resource the caller cannot read (unknown, hidden or refused) is refused in the
//!   `SUBACK` and gets nothing; a client PUBLISH, a Will included, closes the connection: the
//!   extension is read only.

use crate::hub::{Conn, Hub, Next};
use crate::limits::MAX_NOTIFICATION_BYTES;
use crate::mqtt::{self, Bad, Packet};
use crate::socket::{self, Rate, WebSocket};
use antares_ops::{percent, Caller, Ops};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{interval, sleep_until, timeout, Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::Message;

/// Every topic starts with the version (18-088 14.2.1).
const ROOT: &str = "v1.1/";

/// The longest topic filter read.
const MAX_TOPIC: usize = 1024;

/// How long a new connection may take to send its CONNECT.
const CONNECT_WITHIN: Duration = Duration::from_secs(10);

/// The entity sets of the Sensing profile.
const SETS: [&str; 8] = [
    "Things",
    "Locations",
    "HistoricalLocations",
    "Datastreams",
    "Sensors",
    "Observations",
    "ObservedProperties",
    "FeaturesOfInterest",
];

/// The sets a topic may name whole: each changed Entity is one Thing and its Datastreams.
const WHOLE: [&str; 2] = ["Things", "Datastreams"];

/// The members of a collection a notification compares: a collection topic sees changes in its
/// first page of this many.
const MEMBERS: usize = 100;

/// The newest Observations of a Datastream a notification compares: a burst of more between two
/// notifications publishes the newest ten.
const NEWEST: usize = 10;

/// The most digests one connection remembers over all its topics (a resource is one, a
/// collection one per member); past it the memory starts over, and the next change of each
/// resource is published once more than needed.
const MAX_SEEN: usize = 10_000;

/// The most Entity types a Subscription of a view lists.
const MAX_TYPES: usize = 64;

/// The most Entities of one notification a set topic re-reads.
const MAX_ENTITIES: usize = 100;

/// The most reads one notification makes, pages and members together. A changed member past it
/// is left unremembered, so the next notification of its topic reads and publishes it.
const MAX_READS: usize = 100;

/// What a topic names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// `{Set}('{id}')` or `{Set}('{id}')/{Nav}`: the path below `v1.1/`, the Entity in front of
    /// the id, and whether it is a collection (`Nav` a set).
    Resource {
        path: String,
        urn: String,
        collection: bool,
    },
    /// `{Set}`: every Thing or every Datastream the view shows.
    Set(&'static str),
}

/// A topic as read: its target and its `$select`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Topic {
    pub target: Target,
    pub select: Option<String>,
}

/// An id as the STA surface writes it into a path: an `OData` key literal, percent-encoded but for
/// `:` and the quote (`antares_sta::mapping::self_link`).
fn key(id: &str) -> String {
    percent::encode(&id.replace('\'', "''"))
        .replace("%3A", ":")
        .replace("%27", "'")
}

/// The quoted key at the front of `text` (after `('`) unescaped, and what follows its `')`.
fn quoted(text: &str) -> Result<(String, &str), &'static str> {
    let mut id = String::new();
    let mut chars = text.char_indices();
    while let Some((at, c)) = chars.next() {
        if c != '\'' {
            id.push(c);
            continue;
        }
        let rest = text.get(at.saturating_add(1)..).unwrap_or_default();
        if rest.starts_with('\'') {
            id.push('\'');
            let _quote = chars.next();
            continue;
        }
        return rest
            .strip_prefix(')')
            .map(|after| (id, after))
            .ok_or("an id is closed by ')");
    }
    Err("an id is closed by ')")
}

/// The Entity a resource id of `set` is made from (`docs/src/sta.md`, "Resource ids"): the id
/// itself for a Thing and a Sensor, the part in front of the first `/` for the others.
fn entity_of(set: &str, id: &str) -> Result<String, &'static str> {
    let urn = match set {
        "Things" | "Sensors" => Some(id.to_owned()),
        "ObservedProperties" => return Err("an ObservedProperty has no topic: name a Datastream"),
        _ => id.split('/').next().and_then(percent::decoded),
    };
    urn.filter(|u| antares_ops::outputs::is_uri(u))
        .ok_or("the id names no Entity of the view")
}

/// Read a topic filter. No wildcard: a topic names one resource or one set, so no
/// subscription reaches past the view the connection is bound to.
pub(crate) fn topic(text: &str) -> Result<Topic, &'static str> {
    if text.len() > MAX_TOPIC {
        return Err("a topic is at most 1024 bytes");
    }
    if text.contains(['+', '#']) {
        return Err("a topic names one resource: no wildcard");
    }
    if text.chars().any(|c| c.is_control() || c == ' ') {
        return Err("a topic holds no space or control character");
    }
    let rest = text.strip_prefix(ROOT).ok_or("a topic starts with v1.1/")?;
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };
    let select = match query {
        None => None,
        Some(q) => {
            let list = q
                .strip_prefix("$select=")
                .ok_or("a topic takes $select alone")?;
            let names = list.split(',');
            let fine = |n: &str| {
                !n.is_empty()
                    && n.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '/'))
            };
            if !names.clone().all(fine) {
                return Err("$select lists property names");
            }
            Some(list.to_owned())
        }
    };
    let Some((set, after)) = path.split_once("('") else {
        let whole = WHOLE
            .iter()
            .find(|s| **s == path)
            .ok_or("a whole set topic is v1.1/Things or v1.1/Datastreams")?;
        return Ok(Topic {
            target: Target::Set(whole),
            select,
        });
    };
    if !SETS.contains(&set) {
        return Err("no such entity set");
    }
    let (raw, tail) = quoted(after)?;
    let id = percent::decoded(&raw).ok_or("the id is not percent-encoded text")?;
    let urn = entity_of(set, &id)?;
    let nav = match tail {
        "" => None,
        t => {
            let nav = t
                .strip_prefix('/')
                .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphabetic()))
                .ok_or("a topic navigates one property name deep")?;
            Some(nav)
        }
    };
    let mut path = format!("{set}('{}')", key(&id));
    if let Some(nav) = nav {
        path.push('/');
        path.push_str(nav);
    }
    Ok(Topic {
        target: Target::Resource {
            path,
            urn,
            collection: nav.is_some_and(|n| SETS.contains(&n)),
        },
        select,
    })
}

/// A path a notification re-reads.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Probe {
    /// Below `v1.1/`, no query.
    path: String,
    collection: bool,
}

impl Probe {
    /// The page a collection probe reads: the newest Observations, else the first members.
    fn page(&self) -> String {
        if self.path.ends_with("/Observations") {
            format!("{}?$orderby=phenomenonTime%20desc&$top={NEWEST}", self.path)
        } else {
            format!("{}?$top={MEMBERS}", self.path)
        }
    }
}

impl Target {
    /// What a notification about `entity` re-reads.
    fn probe(&self, entity: &str) -> Probe {
        match self {
            Self::Resource {
                path, collection, ..
            } => Probe {
                path: path.clone(),
                collection: *collection,
            },
            Self::Set(set) => {
                let thing = format!("Things('{}')", key(entity));
                match *set {
                    "Things" => Probe {
                        path: thing,
                        collection: false,
                    },
                    other => Probe {
                        path: format!("{thing}/{other}"),
                        collection: true,
                    },
                }
            }
        }
    }
}

/// What a topic last published of one probe.
enum Seen {
    One(u64),
    Members(HashMap<String, u64>),
}

impl Seen {
    /// The digests it holds.
    fn weight(&self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Members(m) => m.len().max(1),
        }
    }
}

/// What every topic of one connection last published, keyed by Subscription and probe path,
/// held to [`MAX_SEEN`] digests.
#[derive(Default)]
struct Memory {
    probes: HashMap<String, Seen>,
    digests: usize,
}

impl Memory {
    fn key(sub: &str, path: &str) -> String {
        format!("{sub}\0{path}")
    }

    fn get(&self, sub: &str, path: &str) -> Option<&Seen> {
        self.probes.get(&Self::key(sub, path))
    }

    /// Keep what `path` of `sub` read last, starting over past [`MAX_SEEN`] digests.
    fn remember(&mut self, sub: &str, path: &str, seen: Seen) {
        let key = Self::key(sub, path);
        if let Some(old) = self.probes.remove(&key) {
            self.digests = self.digests.saturating_sub(old.weight());
        }
        if self.digests.saturating_add(seen.weight()) > MAX_SEEN {
            self.probes.clear();
            self.digests = 0;
        }
        self.digests = self.digests.saturating_add(seen.weight());
        self.probes.insert(key, seen);
    }

    /// Drop what `sub` remembered: its topic is gone.
    fn forget(&mut self, sub: &str) {
        let prefix = Self::key(sub, "");
        self.probes.retain(|k, _| !k.starts_with(&prefix));
        self.digests = self.probes.values().map(Seen::weight).sum();
    }
}

/// One subscribed topic.
struct Watch {
    /// As the client subscribed: the topic every message of it is published on.
    topic: String,
    parsed: Topic,
    /// Its NGSI-LD Subscription.
    sub: String,
}

fn digest(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// The path below `v1.1/` of a member's `@iot.selfLink`.
fn member_path(member: &Value) -> Option<&str> {
    let link = member.get("@iot.selfLink")?.as_str()?;
    let at = link.find("/sta/v1.1/")?;
    link.get(at.saturating_add("/sta/v1.1/".len())..)
}

/// The path a member or a single resource is read with: the topic's `$select` added.
fn selected(path: &str, select: Option<&str>) -> String {
    match select {
        Some(s) => format!("{path}?$select={s}"),
        None => path.to_owned(),
    }
}

/// Where a topic's resources are read: the view's STA root, as the connection's caller. It
/// holds no borrow of the session, whose socket is not `Sync`.
#[derive(Clone)]
struct Reader {
    ops: Ops,
    conn: Arc<Conn>,
    root: String,
}

impl Reader {
    /// The body of a `200` read of `rest` below the root; `None` for any other answer, and
    /// without a read when `reads` is spent.
    async fn get(&self, rest: &str, reads: &mut usize) -> Option<axum::body::Bytes> {
        *reads = reads.checked_sub(1)?;
        let path = if rest.is_empty() {
            self.root.clone()
        } else {
            format!("{}/{rest}", self.root)
        };
        let caller: &Caller = &self.conn.caller;
        let (status, body) = self.ops.get_as(caller, &path, MAX_NOTIFICATION_BYTES).await;
        (status == 200 && !body.is_empty()).then_some(body)
    }
}

/// Re-read `probe` for `watch` and remember it in `memory`: what changed since the last read,
/// each the read of one resource (none when `publish` is false), at most `reads` reads; `None`
/// when the probe cannot be read.
async fn reread(
    reader: &Reader,
    watch: &Watch,
    memory: &mut Memory,
    probe: &Probe,
    publish: bool,
    reads: &mut usize,
) -> Option<Vec<axum::body::Bytes>> {
    let select = watch.parsed.select.as_deref();
    let (seen, mut out) = if probe.collection {
        let page: Value =
            serde_json::from_slice(&reader.get(&probe.page(), reads).await?).unwrap_or_default();
        let before = match memory.get(&watch.sub, &probe.path) {
            Some(Seen::Members(m)) => m.clone(),
            _ => HashMap::new(),
        };
        let mut now = HashMap::new();
        let mut out = Vec::new();
        for member in page
            .get("value")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let id = member
                .get("@iot.id")
                .map(Value::to_string)
                .unwrap_or_default();
            let h = digest(member.to_string().as_bytes());
            if publish && before.get(&id) != Some(&h) {
                let Some(path) = member_path(member) else {
                    now.insert(id, h);
                    continue;
                };
                if *reads == 0 {
                    // read by the next notification of this topic
                    continue;
                }
                out.extend(reader.get(&selected(path, select), reads).await);
            }
            now.insert(id, h);
        }
        (Seen::Members(now), out)
    } else {
        let body = reader.get(&selected(&probe.path, select), reads).await?;
        let h = digest(&body);
        let same = matches!(memory.get(&watch.sub, &probe.path), Some(Seen::One(o)) if *o == h);
        (
            Seen::One(h),
            vec![body].into_iter().filter(|_| !same).collect(),
        )
    };
    if !publish {
        out.clear();
    }
    memory.remember(&watch.sub, &probe.path, seen);
    Some(out)
}

/// Why a connection ended.
enum End {
    Gone,
    Closed(&'static str),
}

struct Session {
    hub: Arc<Hub>,
    conn: Arc<Conn>,
    ops: Ops,
    view: String,
    reader: Reader,
    socket: WebSocket,
    buf: Vec<u8>,
    connected: bool,
    keep_alive: Option<Duration>,
    heard: Instant,
    renewed: Instant,
    rate: Rate,
    generation: u64,
    watches: Vec<Watch>,
    seen: Memory,
}

impl Session {
    async fn write(&mut self, bytes: Vec<u8>) -> bool {
        let limits = &self.hub.limits;
        timeout(
            limits.pong_within,
            self.socket.send(Message::Binary(bytes.into())),
        )
        .await
        .is_ok_and(|r| r.is_ok())
    }

    async fn close(&mut self, reason: &'static str) -> End {
        let _closed = timeout(Duration::from_secs(1), self.socket.close(None)).await;
        End::Closed(reason)
    }

    /// Publish each of `bodies` on `topic`.
    async fn publish(&mut self, topic: &str, bodies: Vec<axum::body::Bytes>) -> Option<End> {
        for body in bodies {
            if !self.write(mqtt::publish(topic, &body)).await {
                return Some(End::Gone);
            }
        }
        None
    }

    /// SUBSCRIBE of one topic filter: its `SUBACK` code.
    async fn watch(&mut self, text: &str) -> u8 {
        if self.watches.iter().any(|w| w.topic == text) {
            return 0;
        }
        let parsed = match topic(text) {
            Ok(t) => t,
            Err(why) => {
                tracing::debug!(connection = %self.conn.id, topic = text, why, "sta mqtt topic refused");
                return mqtt::SUBACK_FAILURE;
            }
        };
        let Some(view) = self.ops.view(&self.view) else {
            return mqtt::SUBACK_FAILURE;
        };
        // the set or the resource must be one the caller reads now
        let visible = match &parsed.target {
            Target::Set(set) => self
                .reader
                .get(&format!("{set}?$top=1"), &mut 1)
                .await
                .is_some(),
            Target::Resource { .. } => true,
        };
        let types = view.watched_types().await;
        if !visible || types.is_empty() || types.len() > MAX_TYPES {
            return mqtt::SUBACK_FAILURE;
        }
        let urn = match &parsed.target {
            Target::Resource { urn, .. } => Some(urn.clone()),
            Target::Set(_) => None,
        };
        let entities: Vec<Value> = types
            .iter()
            .map(|t| match &urn {
                Some(u) => json!({"id": u, "type": t}),
                None => json!({"type": t}),
            })
            .collect();
        let id = format!("urn:ngsi-ld:Subscription:sta-mqtt-{}", crate::random::id());
        let mut body = serde_json::Map::new();
        body.insert("id".into(), json!(id));
        body.insert("type".into(), json!("Subscription"));
        body.insert("entities".into(), Value::Array(entities));
        body.insert("notification".into(), json!({"format": "normalized"}));
        let (status, answer) = socket::subscribe(&self.conn, &self.hub.limits, body).await;
        if status != 201 {
            tracing::debug!(connection = %self.conn.id, topic = text, status, ?answer, "sta mqtt subscription refused");
            return mqtt::SUBACK_FAILURE;
        }
        let probe = parsed.target.probe(urn.as_deref().unwrap_or_default());
        let resource = matches!(parsed.target, Target::Resource { .. });
        let watch = Watch {
            topic: text.to_owned(),
            parsed,
            sub: id.clone(),
        };
        // subscribed before it is read, so no change between the two is lost
        if resource
            && reread(&self.reader, &watch, &mut self.seen, &probe, false, &mut {
                MAX_READS
            })
            .await
            .is_none()
        {
            let _deleted = socket::unsubscribe(&self.conn, &id).await;
            return mqtt::SUBACK_FAILURE;
        }
        self.watches.push(watch);
        0
    }

    async fn unwatch(&mut self, text: &str) {
        if let Some(at) = self.watches.iter().position(|w| w.topic == text) {
            let w = self.watches.remove(at);
            self.seen.forget(&w.sub);
            let _deleted = socket::unsubscribe(&self.conn, &w.sub).await;
        }
    }

    /// One notification queued for this connection: the topics of its Subscription re-read.
    async fn notified(&mut self, text: &str) -> Option<End> {
        let message: Value = serde_json::from_str(text).unwrap_or_default();
        let sub = message
            .pointer("/metadata/x-subscription")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let reader = self.reader.clone();
        // a notification of a topic unsubscribed meanwhile finds none
        let watch = self.watches.iter().find(|w| w.sub == sub)?;
        let probes: Vec<Probe> = match &watch.parsed.target {
            t @ Target::Resource { .. } => vec![t.probe("")],
            t @ Target::Set(_) => message
                .pointer("/body/data")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|e| e.get("id").and_then(Value::as_str))
                .take(MAX_ENTITIES)
                .map(|id| t.probe(id))
                .collect(),
        };
        let mut bodies = Vec::new();
        let mut reads = MAX_READS;
        for probe in &probes {
            if reads == 0 {
                break;
            }
            bodies.extend(
                reread(&reader, watch, &mut self.seen, probe, true, &mut reads)
                    .await
                    .into_iter()
                    .flatten(),
            );
        }
        let topic = watch.topic.clone();
        self.publish(&topic, bodies).await
    }

    async fn drain(&mut self) -> Option<End> {
        let Some(Next::Send(texts)) = self.conn.next(self.generation, &self.hub.limits) else {
            // an MQTT connection is never resumed, so only an overflow takes its queue
            return Some(self.close("overflow").await);
        };
        for text in texts {
            if let Some(end) = self.notified(&text).await {
                return Some(end);
            }
        }
        None
    }

    /// One client packet.
    async fn packet(&mut self, packet: Packet) -> Option<End> {
        if !self.connected {
            return match packet {
                Packet::Connect {
                    level, will: false, ..
                } if level != 4 => {
                    let _sent = self.write(mqtt::connack(mqtt::rc::BAD_PROTOCOL)).await;
                    Some(self.close("protocol level").await)
                }
                Packet::Connect {
                    will: false,
                    keep_alive,
                    ..
                } => {
                    if self.reader.get("", &mut 1).await.is_none() {
                        let _sent = self.write(mqtt::connack(mqtt::rc::NOT_AUTHORIZED)).await;
                        return Some(self.close("not authorized").await);
                    }
                    self.connected = true;
                    self.keep_alive = (keep_alive > 0)
                        .then(|| Duration::from_secs(u64::from(keep_alive)).mul_f32(1.5));
                    (!self.write(mqtt::connack(mqtt::rc::ACCEPTED)).await).then_some(End::Gone)
                }
                // a Will is a message the server would publish for the client: no writes
                Packet::Connect { .. } => {
                    let _sent = self.write(mqtt::connack(mqtt::rc::NOT_AUTHORIZED)).await;
                    Some(self.close("will").await)
                }
                _ => Some(self.close("no CONNECT").await),
            };
        }
        match packet {
            Packet::Connect { .. } => Some(self.close("a second CONNECT").await),
            Packet::Subscribe { id, topics } => {
                let mut codes = Vec::with_capacity(topics.len());
                for t in &topics {
                    codes.push(self.watch(t).await);
                }
                (!self.write(mqtt::suback(id, &codes)).await).then_some(End::Gone)
            }
            Packet::Unsubscribe { id, topics } => {
                for t in &topics {
                    self.unwatch(t).await;
                }
                (!self.write(mqtt::unsuback(id)).await).then_some(End::Gone)
            }
            Packet::PingReq => (!self.write(mqtt::pingresp()).await).then_some(End::Gone),
            Packet::Disconnect => Some(End::Gone),
            Packet::Publish => {
                tracing::info!(connection = %self.conn.id, tenant = %self.conn.tenant, "sta mqtt client published: refused, the extension is read only");
                Some(self.close("publish").await)
            }
        }
    }

    /// A WebSocket frame: MQTT packets in binary frames (MQTT 3.1.1 section 6).
    async fn frame(
        &mut self,
        frame: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    ) -> Option<End> {
        if matches!(frame, Some(Ok(_))) && !self.rate.take(Instant::now()) {
            tracing::warn!(connection = %self.conn.id, tenant = %self.conn.tenant, "sta mqtt client past its message rate");
            return Some(self.close("rate").await);
        }
        let bytes = match frame {
            Some(Ok(Message::Binary(b))) => b,
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => return None,
            Some(Ok(Message::Text(_))) => return Some(self.close("text frame").await),
            Some(Ok(Message::Close(_)) | Err(_)) | None => return Some(End::Gone),
        };
        self.heard = Instant::now();
        self.buf.extend_from_slice(&bytes);
        loop {
            match mqtt::read(&self.buf) {
                Ok((packet, used)) => {
                    self.buf.drain(..used.min(self.buf.len()));
                    if let Some(end) = self.packet(packet).await {
                        return Some(end);
                    }
                }
                // a packet is complete at MAX_PACKET and its header: the buffer stays below
                Err(Bad::Incomplete) => return None,
                Err(Bad::TooBig) => return Some(self.close("too big").await),
                Err(Bad::Malformed(why)) => {
                    tracing::debug!(connection = %self.conn.id, why, "sta mqtt packet malformed");
                    return Some(self.close("malformed").await);
                }
            }
        }
    }

    /// Once a second: keep alive (3.1.2.10), the CONNECT deadline, a removed view, the leases.
    async fn tick(&mut self, opened: Instant) -> Option<End> {
        let now = Instant::now();
        if !self.connected && now.duration_since(opened) > CONNECT_WITHIN {
            return Some(self.close("no CONNECT").await);
        }
        if self
            .keep_alive
            .is_some_and(|k| now.duration_since(self.heard) > k)
        {
            return Some(self.close("keep alive").await);
        }
        if self.ops.view(&self.view).is_none() {
            return Some(self.close("view removed").await);
        }
        let lease = self.hub.limits.subscription_lease;
        if now.duration_since(self.renewed) >= lease.checked_div(3).unwrap_or_default() {
            self.renewed = now;
            socket::renew(&self.conn, lease).await;
        }
        None
    }

    async fn run(&mut self) -> End {
        let opened = Instant::now();
        let deadline = opened
            .checked_add(self.hub.limits.max_lifetime)
            .unwrap_or(opened);
        let mut tick = interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            let conn = self.conn.clone();
            let end = tokio::select! {
                frame = self.socket.next() => self.frame(frame).await,
                () = conn.wake.notified() => self.drain().await,
                _ = tick.tick() => self.tick(opened).await,
                () = sleep_until(deadline) => Some(self.close("lifetime").await),
            };
            if let Some(end) = end {
                return end;
            }
        }
    }
}

/// Serve `conn` on `socket` until it ends; then its Subscriptions are deleted at once: an MQTT
/// connection does not resume.
pub(crate) async fn serve(
    hub: Arc<Hub>,
    conn: Arc<Conn>,
    socket: WebSocket,
    ops: Ops,
    view: String,
) {
    let generation = conn.attach();
    let now = Instant::now();
    let reader = Reader {
        ops: ops.clone(),
        conn: conn.clone(),
        root: format!("/x/views/{view}/sta/v1.1"),
    };
    let mut session = Session {
        rate: Rate::new(&hub.limits, now),
        hub,
        conn,
        ops,
        view,
        reader,
        socket,
        buf: Vec::new(),
        connected: false,
        keep_alive: None,
        heard: now,
        renewed: now,
        generation,
        watches: Vec::new(),
        seen: Memory::default(),
    };
    tracing::info!(connection = %session.conn.id, view = %session.view, "sta mqtt opened");
    let reason = match session.run().await {
        End::Gone => "gone",
        End::Closed(why) => why,
    };
    let Session { hub, conn, .. } = session;
    tracing::info!(connection = %conn.id, reason, "sta mqtt closed");
    let _detached = conn.detach(generation);
    for sub in conn.subscriptions() {
        let _deleted = socket::unsubscribe(&conn, &sub).await;
    }
    hub.close(&conn.id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(text: &str) -> Result<Target, &'static str> {
        topic(text).map(|t| t.target)
    }

    #[test]
    fn a_resource_topic_names_its_entity_and_path() {
        let t = target("v1.1/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations");
        let want = Target::Resource {
            path: "Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Observations".into(),
            urn: "urn:ngsi-ld:Car:1".into(),
            collection: true,
        };
        assert_eq!(t, Ok(want));
        // the raw id, as a client types it, reads the same
        let raw = target("v1.1/Datastreams('urn:ngsi-ld:Car:1/speed')/Observations");
        assert_eq!(raw, t);
        let thing = target("v1.1/Things('urn:ngsi-ld:Car:O''Neil')");
        let want = Target::Resource {
            path: "Things('urn:ngsi-ld:Car:O''Neil')".into(),
            urn: "urn:ngsi-ld:Car:O'Neil".into(),
            collection: false,
        };
        assert_eq!(thing, Ok(want));
        let single = target("v1.1/Datastreams('urn:ngsi-ld:Car:1%2Fspeed')/Thing");
        assert!(matches!(
            single,
            Ok(Target::Resource {
                collection: false,
                ..
            })
        ));
    }

    #[test]
    fn a_set_topic_and_select_read() {
        assert_eq!(
            topic("v1.1/Things?$select=name,@iot.id"),
            Ok(Topic {
                target: Target::Set("Things"),
                select: Some("name,@iot.id".into())
            })
        );
        assert_eq!(target("v1.1/Datastreams"), Ok(Target::Set("Datastreams")));
    }

    #[test]
    fn wildcards_and_other_topics_are_refused() {
        for bad in [
            "v1.1/#",
            "v1.1/+/Observations",
            "v1.1/Things('urn:a:1')/#",
            "Things",
            "v1.0/Things",
            "v1.1/Sensors",
            "v1.1/Cars('urn:a:1')",
            "v1.1/Things('urn:a:1'",
            "v1.1/Things('not a uri')",
            "v1.1/Things('nouri')",
            "v1.1/ObservedProperties('speed')",
            "v1.1/Things('urn:a:1')/Datastreams/Observations",
            "v1.1/Things?$filter=name eq 'x'",
            "v1.1/Things?$top=1",
            "v1.1/Things?$select=a&$expand=b",
            "v1.1/Things?$select=a,,b",
            "v1.1/Things('1a:b')",
            "v1.1/Things('a_b:c')",
            "v1.1/Things('urn:a:1",
            "v1.1/Things('urn:a:1')/",
            "v1.1/Things\u{7}",
        ] {
            assert!(topic(bad).is_err(), "{bad}");
        }
        assert!(topic(&format!("v1.1/Things('urn:a:{}')", "x".repeat(MAX_TOPIC))).is_err());
    }

    #[test]
    fn a_set_topic_reads_the_thing_of_each_changed_entity() {
        let things = Target::Set("Things").probe("urn:ngsi-ld:Car:1");
        assert_eq!(things.path, "Things('urn:ngsi-ld:Car:1')");
        assert!(!things.collection);
        let streams = Target::Set("Datastreams").probe("urn:ngsi-ld:Car:1");
        assert_eq!(streams.path, "Things('urn:ngsi-ld:Car:1')/Datastreams");
        assert_eq!(
            streams.page(),
            "Things('urn:ngsi-ld:Car:1')/Datastreams?$top=100"
        );
        let obs = Probe {
            path: "Datastreams('x')/Observations".into(),
            collection: true,
        };
        assert_eq!(
            obs.page(),
            "Datastreams('x')/Observations?$orderby=phenomenonTime%20desc&$top=10"
        );
    }

    #[test]
    fn the_memory_of_a_topic_starts_over_past_its_bound() {
        let mut memory = Memory::default();
        for i in 0..MAX_SEEN {
            memory.remember("s", &i.to_string(), Seen::One(0));
        }
        memory.remember("s", "0", Seen::One(1));
        assert_eq!(memory.probes.len(), MAX_SEEN, "a known path is replaced");
        memory.remember("s", "new", Seen::One(1));
        assert_eq!(
            memory.probes.len(),
            1,
            "a new path past the bound starts over"
        );
    }

    /// F-R2ANT4-001: a collection weighs one digest per member, and every topic of the
    /// connection shares the bound, so 100 Things of 100 Datastreams on each of two topics hold
    /// at most `MAX_SEEN` digests, not two times 100 paths of 100 members.
    #[test]
    fn the_memory_of_a_connection_counts_members_over_all_its_topics() {
        let members = |n: usize| Seen::Members((0..n).map(|i| (i.to_string(), 0)).collect());
        let mut memory = Memory::default();
        for sub in ["a", "b"] {
            for thing in 0..MAX_SEEN / MEMBERS {
                memory.remember(sub, &thing.to_string(), members(MEMBERS));
            }
        }
        let held: usize = memory
            .probes
            .values()
            .map(|s| match s {
                Seen::One(_) => 1,
                Seen::Members(m) => m.len(),
            })
            .sum();
        assert!(held <= MAX_SEEN, "{held} digests held");
        assert_eq!(held, memory.digests);
        memory.forget("b");
        assert!(memory.probes.keys().all(|k| k.starts_with("a\0")));
        assert_eq!(
            memory.digests,
            memory.probes.values().map(Seen::weight).sum::<usize>()
        );
    }

    #[test]
    fn a_member_path_is_its_self_link_below_the_root() {
        let m = json!({"@iot.selfLink": "http://h/x/views/v/sta/v1.1/Observations('a')"});
        assert_eq!(member_path(&m), Some("Observations('a')"));
        assert_eq!(member_path(&json!({})), None);
    }
}
