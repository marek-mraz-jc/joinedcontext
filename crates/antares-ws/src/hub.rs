// SPDX-License-Identifier: EUPL-1.2
//! The per-process table of live connections (ADR-0026 rule 9), each
//! connection's bounded queue and resume ring (rules 6 and 7), and the
//! admission bounds (rule 10).

use crate::envelope;
use crate::limits::{Limits, MAX_NOTIFICATION_BYTES};
use antares_ops::{Caller, Ops, Outbound};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::Notify;

/// The endpoint URI prefix of a connection (rule 4).
pub const ENDPOINT_PREFIX: &str = "urn:antares:ws:";

/// The digits of `u64::MAX`, the longest seq a notification can carry.
const U64_MAX_DIGITS: usize = 20;

/// The decimal digits of `n`.
fn digits(n: u64) -> usize {
    usize::try_from(n.checked_ilog10().unwrap_or(0))
        .unwrap_or(U64_MAX_DIGITS)
        .saturating_add(1)
}

/// Where a connection's operations run: the NGSI-LD root of its Tenant, or
/// one view, looked up again on every use so a removed view is noticed.
#[derive(Clone)]
pub(crate) enum Runner {
    Root(Ops),
    View(Ops, String),
}

/// One connection, live or within its grace period.
pub(crate) struct Conn {
    pub id: String,
    token: String,
    pub tenant: String,
    pub view: Option<String>,
    pub caller: Caller,
    subject: Vec<(String, Vec<u8>)>,
    pub runner: Runner,
    state: Mutex<State>,
    pub wake: Notify,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Queued>,
    ring: VecDeque<(u64, String)>,
    /// The next `x-seq`; the first notification is 1.
    next_seq: u64,
    /// The notifications lost to an overflow, first to last.
    lost: Option<(u64, u64)>,
    /// The Subscriptions this connection made, with their `showChanges`.
    subs: BTreeMap<String, bool>,
    /// The `expiresAt` the client asked of a Subscription, earlier or later
    /// than its lease (rule 7a).
    wishes: BTreeMap<String, String>,
    /// The socket serving it now, by generation; `None` within grace.
    attached: Option<u64>,
    generation: u64,
    /// The queue overflowed while a socket was attached: it closes.
    overflowed: bool,
    /// The bytes of the queued texts and of the ring's: together they stay
    /// under `ANTARES_WS_QUEUE_BYTES` (F-ANT3-001).
    queue_bytes: usize,
    ring_bytes: usize,
}

struct Queued {
    seq: u64,
    sub: String,
    entity: Option<String>,
    coalescible: bool,
    /// Earlier notifications this one replaced (`x-coalesced`).
    merged: u64,
    /// An `x-gap` from this seq to `seq`: notifications the broker lost
    /// before they reached this queue.
    gap_from: Option<u64>,
    text: String,
}

/// What a socket writes next.
pub(crate) enum Next {
    /// The queue overflowed: `x-gap` over the lost range, `x-closing`, close.
    Overflow(u64, u64),
    /// Notifications, in order.
    Send(Vec<String>),
}

impl State {
    /// The queue is full: what it holds and `seq` are lost; an attached
    /// socket writes the `x-gap` and closes.
    fn overflow(&mut self, seq: u64) {
        let from = self
            .queue
            .front()
            .map_or(seq, |q| q.gap_from.unwrap_or(q.seq));
        let from = self.lost.map_or(from, |(f, _)| f.min(from));
        self.lost = Some((from, seq));
        self.queue.clear();
        self.queue_bytes = 0;
        if self.attached.is_some() {
            self.overflowed = true;
        }
    }

    /// Drops the ring's oldest texts while it holds more than `count`, or
    /// while the queue, the ring and `add` more bytes pass `budget`.
    fn trim(&mut self, count: usize, budget: usize, add: usize) {
        while let Some(len) = self.ring.front().map(|(_, t)| t.len()).filter(|_| {
            self.ring.len() > count
                || self
                    .queue_bytes
                    .saturating_add(self.ring_bytes)
                    .saturating_add(add)
                    > budget
        }) {
            drop(self.ring.pop_front());
            self.ring_bytes = self.ring_bytes.saturating_sub(len);
        }
    }

    /// Room for `add` more queued bytes: the ring's oldest texts leave
    /// first; false when the queue alone would pass `queue_bytes`.
    fn room(&mut self, add: usize, limits: &Limits) -> bool {
        self.trim(limits.queue, limits.queue_bytes, add);
        self.queue_bytes.saturating_add(add) <= limits.queue_bytes
    }

    /// Offer one notification (rule 6): coalesced into a queued one of the
    /// same Subscription and single Entity when neither shows changes,
    /// queued when there is room, and otherwise lost with everything still
    /// queued. Room is a count (`queue`) and bytes (`queue_bytes`, shared
    /// with the ring).
    fn offer(&mut self, n: Forwarded, limits: &Limits) -> Answer {
        let Some(&show) = self.subs.get(&n.subscription) else {
            return Answer::Unknown;
        };
        let coalescible = !show && n.entity.is_some() && !n.changes;
        let into = coalescible
            .then(|| {
                // never past a queued x-gap: a change after a loss leaves
                // after it, so the consumer reads the gap first
                self.queue
                    .iter()
                    .rposition(|q| {
                        q.gap_from.is_some()
                            || (q.coalescible && q.sub == n.subscription && q.entity == n.entity)
                    })
                    .filter(|&i| self.queue.get(i).is_some_and(|q| q.gap_from.is_none()))
            })
            .flatten();
        let queued = into.and_then(|i| self.queue.get(i));
        let seq = queued.map_or(self.next_seq.max(1), |q| q.seq);
        let merged = queued.map_or(0, |q| q.merged.saturating_add(1));
        let old = queued.map_or(0, |q| q.text.len());
        // rendered once, with the seq it takes; the cap is judged as if the
        // seq had the most digits a u64 has, so a text at the cap stays
        // under it whatever seq it gets
        let text = envelope::notification(&n.rendered, seq, &n.subscription, merged);
        if text
            .len()
            .saturating_add(U64_MAX_DIGITS.saturating_sub(digits(seq)))
            > MAX_NOTIFICATION_BYTES
        {
            return Answer::TooLarge;
        }
        if into.is_some() && !self.room(text.len().saturating_sub(old), limits) {
            self.overflow(self.next_seq.saturating_sub(1).max(seq));
            return Answer::Overflow;
        }
        if let Some(q) = into.and_then(|i| self.queue.get_mut(i)) {
            self.queue_bytes = self
                .queue_bytes
                .saturating_sub(old)
                .saturating_add(text.len());
            q.text = text;
            q.merged = merged;
            return Answer::Queued;
        }
        self.next_seq = seq.saturating_add(1);
        if self.queue.len() >= limits.queue || !self.room(text.len(), limits) {
            self.overflow(seq);
            return Answer::Overflow;
        }
        self.queue_bytes = self.queue_bytes.saturating_add(text.len());
        self.queue.push_back(Queued {
            seq,
            sub: n.subscription,
            entity: n.entity,
            coalescible,
            merged,
            gap_from: None,
            text,
        });
        Answer::Queued
    }

    /// A notification of `sub` lost before it was offered, as an `x-gap`
    /// in its place; losses in a row share one. An `x-gap` is counted in
    /// the queue's bytes but never refused on them: at most `queue` of them,
    /// each under 100 bytes.
    fn lose(&mut self, sub: &str, limits: &Limits) -> Answer {
        if !self.subs.contains_key(sub) {
            return Answer::Unknown;
        }
        let seq = self.next_seq.max(1);
        self.next_seq = seq.saturating_add(1);
        if let Some(q) = self.queue.back_mut() {
            if let Some(from) = q.gap_from {
                let text = envelope::gap(from, seq);
                self.queue_bytes = self
                    .queue_bytes
                    .saturating_sub(q.text.len())
                    .saturating_add(text.len());
                q.seq = seq;
                q.text = text;
                return Answer::Queued;
            }
        }
        let text = envelope::gap(seq, seq);
        if self.queue.len() >= limits.queue {
            self.overflow(seq);
            return Answer::Overflow;
        }
        self.queue_bytes = self.queue_bytes.saturating_add(text.len());
        self.queue.push_back(Queued {
            seq,
            sub: sub.to_owned(),
            entity: None,
            coalescible: false,
            merged: 0,
            gap_from: Some(seq),
            text,
        });
        Answer::Queued
    }

    /// What the socket of `generation` writes next; written notifications
    /// enter the ring, which keeps the last `queue` and, with the queue,
    /// at most `queue_bytes`.
    fn next(&mut self, generation: u64, limits: &Limits) -> Option<Next> {
        if self.attached != Some(generation) {
            return None;
        }
        if self.overflowed {
            self.overflowed = false;
            let (from, to) = self.lost.unwrap_or((0, 0));
            return Some(Next::Overflow(from, to));
        }
        let mut out = Vec::with_capacity(self.queue.len());
        while let Some(q) = self.queue.pop_front() {
            let len = q.text.len();
            out.push(q.text.clone());
            self.queue_bytes = self.queue_bytes.saturating_sub(len);
            self.ring_bytes = self.ring_bytes.saturating_add(len);
            self.ring.push_back((q.seq, q.text));
        }
        self.trim(limits.queue, limits.queue_bytes, 0);
        Some(Next::Send(out))
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Conn {
    pub fn endpoint(&self) -> String {
        format!("{ENDPOINT_PREFIX}{}", self.id)
    }

    pub fn subscriptions(&self) -> Vec<String> {
        lock(&self.state).subs.keys().cloned().collect()
    }

    pub fn owns(&self, sub: &str) -> bool {
        lock(&self.state).subs.contains_key(sub)
    }

    pub fn subscription_count(&self) -> usize {
        lock(&self.state).subs.len()
    }

    pub fn record(&self, sub: &str, show_changes: bool) {
        lock(&self.state).subs.insert(sub.to_owned(), show_changes);
    }

    pub fn forget(&self, sub: &str) {
        let mut st = lock(&self.state);
        st.subs.remove(sub);
        st.wishes.remove(sub);
    }

    /// The `expiresAt` the client asked of `sub`, `None` for no expiry.
    pub fn want(&self, sub: &str, wish: Option<String>) {
        let mut st = lock(&self.state);
        match wish {
            Some(w) => st.wishes.insert(sub.to_owned(), w),
            None => st.wishes.remove(sub),
        };
    }

    /// What [`Conn::want`] last recorded for `sub`.
    pub fn wish(&self, sub: &str) -> Option<String> {
        lock(&self.state).wishes.get(sub).cloned()
    }

    pub fn is_attached(&self, generation: u64) -> bool {
        lock(&self.state).attached == Some(generation)
    }

    /// Offer one notification ([`State::offer`]); an overflow closes an
    /// attached socket.
    fn offer(&self, n: Forwarded, limits: &Limits) -> Answer {
        let answer = lock(&self.state).offer(n, limits);
        if matches!(answer, Answer::Queued | Answer::Overflow) {
            self.wake.notify_one();
        }
        answer
    }

    /// A notification of `sub` the broker dropped before offering it (a
    /// full delivery lane): it takes the next seq and leaves as an `x-gap`
    /// in its place, so the consumer learns of it in order. Losses in a row
    /// share one `x-gap`.
    fn lose(&self, sub: &str, limits: &Limits) -> Answer {
        let answer = lock(&self.state).lose(sub, limits);
        if matches!(answer, Answer::Queued | Answer::Overflow) {
            self.wake.notify_one();
        }
        answer
    }

    /// What the socket of `generation` writes next ([`State::next`]).
    pub fn next(&self, generation: u64, limits: &Limits) -> Option<Next> {
        lock(&self.state).next(generation, limits)
    }

    /// Attach a socket: the next generation, which the previous socket of
    /// this connection, if any, sees as its end.
    pub fn attach(&self) -> u64 {
        let mut st = lock(&self.state);
        st.generation = st.generation.saturating_add(1);
        st.attached = Some(st.generation);
        st.overflowed = false;
        st.generation
    }

    /// Whether no socket attached since the one of `generation` detached.
    pub fn detached_since(&self, generation: u64) -> bool {
        let st = lock(&self.state);
        st.attached.is_none() && st.generation == generation
    }

    /// Detach the socket of `generation`; false when another took over.
    pub fn detach(&self, generation: u64) -> bool {
        let mut st = lock(&self.state);
        if st.attached != Some(generation) {
            return false;
        }
        st.attached = None;
        true
    }

    /// The messages a resumed socket starts with (rule 7): `x-gap` for
    /// what the ring no longer holds after `seq`, the ring after `seq`, and
    /// `x-gap` for what an overflow lost.
    pub fn replay(&self, seq: u64) -> Vec<String> {
        let st = lock(&self.state);
        let mut out = Vec::new();
        let after = seq.saturating_add(1);
        let oldest = st.ring.front().map(|(s, _)| *s);
        let lost_from = st.lost.map(|(f, _)| f);
        // the first seq the ring or the queue still holds
        let first_kept = oldest
            .or_else(|| st.queue.front().map(|q| q.gap_from.unwrap_or(q.seq)))
            .or(lost_from)
            .unwrap_or(st.next_seq.max(1));
        if first_kept > after {
            out.push(envelope::gap(after, first_kept.saturating_sub(1)));
        }
        out.extend(
            st.ring
                .iter()
                .filter(|(s, _)| *s > seq)
                .map(|(_, t)| t.clone()),
        );
        if let Some((from, to)) = st.lost.filter(|(_, to)| *to > seq) {
            out.push(envelope::gap(from.max(after), to));
        }
        out
    }
}

/// One notification as the sink hands it to the connection: rendered, with
/// what the queue decides on. It is also the payload one pod sends another
/// for a connection it does not hold (rule 9).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Forwarded {
    /// The Tenant the notification is of.
    pub tenant: String,
    /// `subscriptionId`.
    pub subscription: String,
    /// The one Entity's id when the notification carries exactly one.
    pub entity: Option<String>,
    /// Whether any Attribute carries a previous value (`showChanges`).
    pub changes: bool,
    /// The TS 104 243 message the MQTT binding would publish.
    pub rendered: Value,
}

/// What a connection answers an offered notification.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Answer {
    /// Queued or coalesced.
    Queued,
    /// The queue was full: lost, the socket closes.
    Overflow,
    /// Larger than [`MAX_NOTIFICATION_BYTES`]: never written.
    TooLarge,
    /// No such connection here, or not one of its Subscriptions, or of
    /// another Tenant: never written.
    Unknown,
}

impl Forwarded {
    /// Read a notification for delivery; `None` when it names no
    /// Subscription.
    #[must_use]
    pub fn of(out: &Outbound) -> Option<Self> {
        let subscription = out.body.get("subscriptionId")?.as_str()?.to_owned();
        let data = out.body.get("data").and_then(Value::as_array);
        let entity = data
            .filter(|d| d.len() == 1)
            .and_then(|d| d.first())
            .and_then(|e| e.get("id"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let changes = data.is_some_and(|d| d.iter().any(carries_previous));
        let tenant = out
            .receiver_info
            .iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case("NGSILD-Tenant"))
            .map_or_else(|| "default".to_owned(), |(_, v)| v.clone());
        Some(Self {
            tenant,
            subscription,
            entity,
            changes,
            rendered: antares_ops::build_message(
                &out.body,
                &out.accept,
                Some(&out.link),
                &out.receiver_info,
            ),
        })
    }
}

fn carries_previous(entity: &Value) -> bool {
    entity.as_object().is_some_and(|o| {
        o.values().any(|attr| {
            let one = |a: &Value| {
                a.as_object()
                    .is_some_and(|a| a.keys().any(|k| k.starts_with("previous")))
            };
            match attr {
                Value::Array(instances) => instances.iter().any(one),
                a => one(a),
            }
        })
    })
}

/// Cross-pod delivery (rule 9, `bus = nats`): a request on the connection's
/// own subject, answered by the pod that holds it. The broker implements it
/// over its bus; the hub only asks and answers.
pub trait Relay: Send + Sync {
    /// Send `payload` (a serialized [`Forwarded`]) to whichever pod holds
    /// `connection`; its serialized [`Answer`], or `None` when no pod
    /// answered.
    fn request<'a>(
        &'a self,
        connection: &'a str,
        payload: Vec<u8>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Vec<u8>>> + Send + 'a>>;
    /// Start answering requests for `connection` on this pod, through
    /// [`Hub::answer`].
    fn listen(&self, hub: std::sync::Weak<Hub>, connection: &str);
    /// Stop answering for `connection`.
    fn unlisten(&self, connection: &str);
}

/// The connections of this process.
pub struct Hub {
    pub(crate) limits: Limits,
    conns: Mutex<HashMap<String, Arc<Conn>>>,
    relay: Mutex<Option<Arc<dyn Relay>>>,
    overflows: std::sync::atomic::AtomicU64,
}

/// Why an upgrade was not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// `ANTARES_WS_MAX_CONNECTIONS`.
    Process,
    /// `ANTARES_WS_MAX_CONNECTIONS_PER_TENANT`.
    Tenant,
}

impl Hub {
    /// An empty table under `limits`.
    ///
    /// ```
    /// let hub = antares_ws::Hub::new(antares_ws::Limits::default());
    /// assert_eq!(hub.connections(), 0);
    /// ```
    #[must_use]
    pub fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Self {
            limits,
            conns: Mutex::default(),
            relay: Mutex::default(),
            overflows: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// The live connections and those within their grace period.
    #[must_use]
    pub fn connections(&self) -> usize {
        lock(&self.conns).len()
    }

    /// The connections a socket holds now: [`Hub::connections`] less those
    /// waiting out their grace period.
    ///
    /// ```
    /// let hub = antares_ws::Hub::new(antares_ws::Limits::default());
    /// assert_eq!(hub.attached(), 0);
    /// ```
    #[must_use]
    pub fn attached(&self) -> usize {
        let conns: Vec<Arc<Conn>> = lock(&self.conns).values().cloned().collect();
        conns
            .iter()
            .filter(|c| lock(&c.state).attached.is_some())
            .count()
    }

    /// Connections closed for overflow since start (`/q/metrics`).
    #[must_use]
    pub fn overflows(&self) -> u64 {
        self.overflows.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The changes the queues hold for their sockets to write: a coalesced
    /// notification counts each change it carries, an `x-gap` none
    /// (`/q/version`).
    ///
    /// ```
    /// let hub = antares_ws::Hub::new(antares_ws::Limits::default());
    /// assert_eq!(hub.queued(), 0);
    /// ```
    #[must_use]
    pub fn queued(&self) -> u64 {
        let conns: Vec<Arc<Conn>> = lock(&self.conns).values().cloned().collect();
        conns
            .iter()
            .flat_map(|c| {
                lock(&c.state)
                    .queue
                    .iter()
                    .filter(|q| q.gap_from.is_none())
                    .map(|q| q.merged.saturating_add(1))
                    .collect::<Vec<_>>()
            })
            .fold(0, u64::saturating_add)
    }

    /// Deliver to connections other pods hold through `relay`.
    pub fn set_relay(&self, relay: Arc<dyn Relay>) {
        *lock(&self.relay) = Some(relay);
    }

    fn relay(&self) -> Option<Arc<dyn Relay>> {
        lock(&self.relay).clone()
    }

    pub(crate) fn get(&self, id: &str) -> Option<Arc<Conn>> {
        lock(&self.conns).get(id).cloned()
    }

    /// Admit and register a new connection (rule 10, W2).
    pub(crate) fn open(
        self: &Arc<Self>,
        tenant: String,
        view: Option<String>,
        caller: Caller,
        subject: Vec<(String, Vec<u8>)>,
        runner: Runner,
    ) -> Result<Arc<Conn>, Refused> {
        let mut conns = lock(&self.conns);
        if conns.len() >= self.limits.max_connections {
            return Err(Refused::Process);
        }
        if conns.values().filter(|c| c.tenant == tenant).count()
            >= self.limits.max_connections_per_tenant
        {
            return Err(Refused::Tenant);
        }
        let mut id = crate::random::id();
        while conns.contains_key(&id) {
            id = crate::random::id();
        }
        let conn = Arc::new(Conn {
            id: id.clone(),
            token: crate::random::token(),
            tenant,
            view,
            caller,
            subject,
            runner,
            state: Mutex::new(State {
                next_seq: 1,
                ..State::default()
            }),
            wake: Notify::new(),
        });
        conns.insert(id.clone(), conn.clone());
        drop(conns);
        if let Some(relay) = self.relay() {
            relay.listen(Arc::downgrade(self), &id);
        }
        Ok(conn)
    }

    /// The token a client resumes `conn` with.
    pub(crate) fn token(conn: &Conn) -> &str {
        &conn.token
    }

    /// Remove a connection from the table.
    pub(crate) fn close(&self, id: &str) {
        lock(&self.conns).remove(id);
        if let Some(relay) = self.relay() {
            relay.unlisten(id);
        }
    }

    /// The connection a resume names, when every check of rule 7 holds:
    /// the token (constant time), the Tenant, the view and the subject.
    pub(crate) fn resumable(
        &self,
        connection: &str,
        token: &str,
        fresh: &Conn,
    ) -> Option<Arc<Conn>> {
        let conn = self.get(connection)?;
        let same_token = crate::random::equal(conn.token.as_bytes(), token.as_bytes());
        (same_token
            && conn.id != fresh.id
            && conn.tenant == fresh.tenant
            && conn.view == fresh.view
            && conn.subject == fresh.subject)
            .then_some(conn)
    }

    /// Offer a notification to a connection this pod holds.
    fn offer_here(&self, id: &str, n: Forwarded) -> Answer {
        let Some(conn) = self.get(id) else {
            return Answer::Unknown;
        };
        // rule 9: a write compares the Tenants
        if conn.tenant != n.tenant {
            return Answer::Unknown;
        }
        let answer = conn.offer(n, &self.limits);
        if answer == Answer::Overflow {
            self.overflows
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        answer
    }

    /// Tell the connection `id` this pod holds that a notification of its
    /// Subscription `sub` was lost before delivery: the socket writes an
    /// `x-gap` over it. ponytail: a connection another pod holds is not
    /// told; relay a loss message when `bus = nats` fan-out needs it.
    pub fn lose(&self, id: &str, tenant: &str, sub: &str) -> Answer {
        let Some(conn) = self.get(id) else {
            return Answer::Unknown;
        };
        if conn.tenant != tenant {
            return Answer::Unknown;
        }
        let answer = conn.lose(sub, &self.limits);
        if answer == Answer::Overflow {
            self.overflows
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        answer
    }

    /// Answer a request another pod sent for a connection on this one.
    #[must_use]
    pub fn answer(&self, connection: &str, payload: &[u8]) -> Vec<u8> {
        let answer = serde_json::from_slice::<Forwarded>(payload)
            .map_or(Answer::Unknown, |n| self.offer_here(connection, n));
        serde_json::to_vec(&answer).unwrap_or_default()
    }

    /// Deliver to the connection `id`: here when this pod holds it, through
    /// the relay otherwise.
    pub(crate) async fn deliver(&self, id: &str, n: Forwarded) -> Answer {
        if self.get(id).is_some() {
            return self.offer_here(id, n);
        }
        let Some(relay) = self.relay() else {
            return Answer::Unknown;
        };
        // a Forwarded always serializes; an empty payload would be Unknown
        // on the other side
        let payload = serde_json::to_vec(&n).unwrap_or_default();
        relay
            .request(id, payload)
            .await
            .and_then(|a| serde_json::from_slice(&a).ok())
            .unwrap_or(Answer::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An attached connection's state with one Subscription `s`.
    fn attached() -> State {
        let mut st = State {
            next_seq: 1,
            attached: Some(1),
            ..State::default()
        };
        st.subs.insert("s".to_owned(), false);
        st
    }

    fn big() -> Forwarded {
        Forwarded {
            tenant: "t".to_owned(),
            subscription: "s".to_owned(),
            entity: None,
            changes: false,
            rendered: json!({ "body": { "x": "a".repeat(900 * 1024) } }),
        }
    }

    /// The texts a socket writes, none for an overflow.
    fn texts(next: Option<Next>) -> Vec<String> {
        match next {
            Some(Next::Send(texts)) => texts,
            _ => Vec::new(),
        }
    }

    fn ring_bytes(st: &State) -> usize {
        st.ring.iter().map(|(_, t)| t.len()).sum()
    }

    /// F-ANT3-001: 256 notifications of 900 KiB, each drained as it comes,
    /// leave the ring under the byte budget, not 256 of them.
    #[test]
    fn the_ring_holds_bytes_under_the_budget() {
        let limits = Limits::default();
        let mut st = attached();
        for _ in 0..256 {
            assert_eq!(st.offer(big(), &limits), Answer::Queued);
            let sent = st.next(1, &limits);
            assert!(matches!(sent, Some(Next::Send(_))));
        }
        assert!(ring_bytes(&st) <= limits.queue_bytes, "{}", ring_bytes(&st));
        assert_eq!(st.ring_bytes, ring_bytes(&st));
        assert!(st.ring.len() < 256, "{}", st.ring.len());
        let newest = st.ring.back().map(|(seq, _)| *seq);
        assert_eq!(newest, Some(256));
    }

    /// F-ANT3-001: a coalesced notification that grows the queue past its
    /// bytes overflows too; an `x-gap` is counted in them.
    #[test]
    fn a_coalesced_notification_past_the_bytes_overflows() {
        let limits = Limits {
            queue_bytes: MAX_NOTIFICATION_BYTES,
            ..Limits::default()
        };
        let mut st = attached();
        assert_eq!(st.offer(big(), &limits), Answer::Queued);
        let mut small = big();
        small.entity = Some("e".to_owned());
        small.rendered = json!({ "body": { "x": 1 } });
        assert_eq!(st.offer(small.clone(), &limits), Answer::Queued);
        assert_eq!(
            st.offer(small, &limits),
            Answer::Queued,
            "coalesced, it fits"
        );
        assert_eq!(st.queue.len(), 2);
        let mut grown = big();
        grown.entity = Some("e".to_owned());
        assert_eq!(st.offer(grown, &limits), Answer::Overflow);
        assert_eq!(st.queue_bytes, 0);
        assert_eq!(st.lose("s", &limits), Answer::Queued);
        assert!(st.queue_bytes > 0);
        assert_eq!(st.lose("s", &limits), Answer::Queued);
        let gap = st.queue.front().map(|q| q.text.len());
        assert_eq!(Some(st.queue_bytes), gap);
    }

    /// F-ANT3-001: offered and not drained, the queue overflows on bytes
    /// long before its count, through the `x-gap` path.
    #[test]
    fn the_queue_overflows_on_bytes() {
        let limits = Limits::default();
        let mut st = attached();
        let answers: Vec<Answer> = (0..256).map(|_| st.offer(big(), &limits)).collect();
        let first = answers.iter().position(|a| *a == Answer::Overflow);
        assert!(first.is_some(), "no overflow reported");
        assert!(first < Some(limits.queue), "{first:?}");
        let held = st.queue_bytes.saturating_add(ring_bytes(&st));
        assert!(held <= limits.queue_bytes, "{held}");
        let gap = st.next(1, &limits);
        assert!(matches!(gap, Some(Next::Overflow(1, _))));
        assert_eq!(texts(gap), Vec::<String>::new());
        let lengths: Vec<usize> = texts(st.next(1, &limits)).iter().map(String::len).collect();
        assert!(lengths.iter().sum::<usize>() <= limits.queue_bytes);
        assert!(ring_bytes(&st) <= limits.queue_bytes, "{}", ring_bytes(&st));
    }
}
