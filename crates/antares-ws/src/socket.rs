// SPDX-License-Identifier: EUPL-1.2
//! One socket: the welcome, the client's operations through the operation
//! layer, the queue drained in order, Ping/Pong, idle, lifetime and resume
//! (ADR-0026 rules 4 to 8).

use crate::envelope::{self, ClientOp, Unread};
use crate::hub::{Conn, Hub, Next, Runner};
use crate::limits::Limits;
use antares_ops::{Caller, OperationId, OpsRequest};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::error::ProtocolError;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

/// The socket of one connection: tungstenite on the connection hyper
/// upgraded (T-3704).
pub(crate) type WebSocket =
    tokio_tungstenite::WebSocketStream<hyper_util::rt::TokioIo<hyper::upgrade::Upgraded>>;
use axum::http::{header, HeaderValue, Method, StatusCode};
use serde_json::{Map, Value};
use std::sync::Arc;
use tokio::time::{interval_at, sleep_until, timeout, Instant, MissedTickBehavior};

/// How long a server close waits for the client's close frame: long enough
/// for frames already on the wire, short enough that a client that never
/// answers holds its connection one second and not a Pong deadline.
const CLOSE_ANSWER: std::time::Duration = std::time::Duration::from_secs(1);

tokio::task_local! {
    /// The connection a Subscription is being made for (rule 4, W6): the
    /// sink accepts `urn:antares:ws:<id>` only inside this scope.
    pub(crate) static CREATING: String;
}

/// The close codes this surface uses (RFC 6455 7.4.1).
mod code {
    pub const NORMAL: u16 = 1000;
    pub const PROTOCOL: u16 = 1002;
    pub const UNSUPPORTED: u16 = 1003;
    pub const NOT_UTF8: u16 = 1007;
    pub const POLICY: u16 = 1008;
    pub const TOO_BIG: u16 = 1009;
    pub const TRY_AGAIN: u16 = 1013;
}

/// The largest answer body read back from the operation layer.
const ANSWER_BYTES: usize = 1024 * 1024;

/// Why a socket ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// The client closed it, or it broke.
    Gone,
    /// Another socket resumed the connection.
    TakenOver,
    /// The server closed it with `x-closing` and this reason.
    Closed(&'static str),
}

fn not_found() -> (u16, Option<Value>) {
    (StatusCode::NOT_FOUND.as_u16(), Some(envelope::not_found()))
}

fn bad(detail: &str) -> (u16, Option<Value>) {
    (
        StatusCode::BAD_REQUEST.as_u16(),
        Some(envelope::bad_request(detail)),
    )
}

/// A path segment: everything but the unreserved characters and `:`
/// percent-encoded, so a Subscription id is one segment whatever it holds.
fn segment(id: &str) -> String {
    id.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~:".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Run one operation where the connection runs, inside its marker.
async fn run(conn: &Conn, op: OperationId, req: OpsRequest) -> (u16, Option<Value>) {
    let caller: &Caller = &conn.caller;
    let answer = CREATING
        .scope(conn.id.clone(), async {
            match &conn.runner {
                Runner::Root(ops) => ops.run(op, caller, req).await,
                Runner::View(ops, id) => match ops.view(id) {
                    Some(view) => view.run(op, caller, req).await,
                    None => Err(antares_ops::Refusal::NotAllowed { allow: Vec::new() }),
                },
            }
        })
        .await
        .unwrap_or_else(|refusal| refusal.ngsi_ld());
    let status = answer.status().as_u16();
    let body = axum::body::to_bytes(answer.into_body(), ANSWER_BYTES)
        .await
        .ok()
        .filter(|b| !b.is_empty())
        .and_then(|b| serde_json::from_slice(&b).ok());
    (status, body)
}

fn json_request(method: Method, path: String, body: &Map<String, Value>) -> OpsRequest {
    let bytes = Value::Object(body.clone()).to_string();
    let media = if body.contains_key("@context") {
        "application/ld+json"
    } else {
        "application/json"
    };
    OpsRequest::new(method, path)
        .header(header::CONTENT_TYPE, HeaderValue::from_static(media))
        .header(header::CONTENT_LENGTH, HeaderValue::from(bytes.len()))
        .body(bytes)
}

/// The endpoint member a client message names, when it names one.
fn named_endpoint(body: &Map<String, Value>) -> Option<&Value> {
    body.get("notification")?.get("endpoint")?.get("uri")
}

fn show_changes(body: &Map<String, Value>) -> Option<bool> {
    body.get("notification")?.get("showChanges")?.as_bool()
}

async fn retrieve(conn: &Conn, id: &str) -> Option<Value> {
    let req = OpsRequest::new(
        Method::GET,
        format!("/ngsi-ld/v1/subscriptions/{}", segment(id)),
    );
    run(conn, OperationId::RetrieveSubscription, req).await.1
}

/// The NGSI-LD Null a fragment removes a member with (4.5.0).
const NULL: &str = "urn:ngsi-ld:null";

/// The `expiresAt` a socket Subscription is given (rule 7a): the lease
/// from now, or the client's own `wish` when it is earlier. A wish that is
/// no `DateTime` is passed on as it is, for the operation to refuse.
fn leased(wish: Option<&str>, lease: std::time::Duration) -> String {
    let until = chrono::Duration::from_std(lease)
        .ok()
        .and_then(|d| chrono::Utc::now().checked_add_signed(d))
        .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC);
    match wish {
        Some(w) => match chrono::DateTime::parse_from_rfc3339(w) {
            Ok(at) if at < until => w.to_owned(),
            Ok(_) => until.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            Err(_) => w.to_owned(),
        },
        None => until.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    }
}

/// Renew the lease of every Subscription `conn` made (rule 7a); one deleted
/// meanwhile (over HTTP, or by the sweep) is forgotten.
pub(crate) async fn renew(conn: &Conn, lease: std::time::Duration) {
    for sub in conn.subscriptions() {
        let mut fragment = Map::new();
        let until = leased(conn.wish(&sub).as_deref(), lease);
        fragment.insert("expiresAt".into(), Value::String(until));
        let path = format!("/ngsi-ld/v1/subscriptions/{}", segment(&sub));
        let req = json_request(Method::PATCH, path, &fragment);
        let (status, answer) = run(conn, OperationId::UpdateSubscription, req).await;
        tracing::debug!(connection = %conn.id, subscription = %sub, status, ?answer, "websocket subscription lease renewed");
        if status == StatusCode::NOT_FOUND.as_u16() {
            conn.forget(&sub);
        }
    }
}

/// `x-subscribe`: Create Subscription (10.5.2) with this connection as the
/// endpoint; a body naming another endpoint is refused (W6), past
/// `max_subscriptions` too (W9).
pub(crate) async fn subscribe(
    conn: &Conn,
    limits: &Limits,
    mut body: Map<String, Value>,
) -> (u16, Option<Value>) {
    let max = limits.max_subscriptions;
    if conn.subscription_count() >= max {
        return bad(&format!("a connection makes at most {max} Subscriptions"));
    }
    let own = conn.endpoint();
    match named_endpoint(&body) {
        None => {}
        Some(Value::String(uri)) if *uri == own => {}
        Some(_) => {
            return bad("the endpoint of a Subscription made over this socket is the socket")
        }
    }
    let notification = body
        .entry("notification")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(notification) = notification.as_object_mut() else {
        return bad("notification is a JSON object");
    };
    let endpoint = notification
        .entry("endpoint")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(endpoint) = endpoint.as_object_mut() else {
        return bad("notification.endpoint is a JSON object");
    };
    endpoint.insert("uri".into(), Value::String(own));
    // the id is known before the Subscription exists, so a notification
    // sent the moment it is created already finds it
    let id = match body.get("id") {
        None => {
            let id = format!("urn:ngsi-ld:Subscription:ws-{}", crate::random::id());
            body.insert("id".into(), Value::String(id.clone()));
            id
        }
        Some(Value::String(id)) => id.clone(),
        Some(_) => return bad("id is a URI"),
    };
    if conn.owns(&id) {
        return (
            StatusCode::CONFLICT.as_u16(),
            Some(serde_json::json!({
                "type": "https://uri.etsi.org/ngsi-ld/errors/AlreadyExists",
                "title": "AlreadyExists",
                "status": 409,
            })),
        );
    }
    let wish = match body.get("expiresAt") {
        None => None,
        Some(Value::String(w)) => Some(w.clone()),
        Some(_) => return bad("expiresAt is a DateTime"),
    };
    let until = leased(wish.as_deref(), limits.subscription_lease);
    body.insert("expiresAt".into(), Value::String(until));
    conn.record(&id, show_changes(&body).unwrap_or(false));
    let req = json_request(Method::POST, "/ngsi-ld/v1/subscriptions".into(), &body);
    let (status, answer) = run(conn, OperationId::CreateSubscription, req).await;
    if status != StatusCode::CREATED.as_u16() {
        conn.forget(&id);
        return (status, answer);
    }
    conn.want(&id, wish);
    (status, retrieve(conn, &id).await)
}

/// `x-update`: Update Subscription (10.5.3) of one this connection made.
async fn update(
    conn: &Conn,
    limits: &Limits,
    id: &str,
    mut fragment: Map<String, Value>,
) -> (u16, Option<Value>) {
    if !conn.owns(id) {
        return not_found();
    }
    // a new expiresAt is a new wish under the same lease; removing it
    // leaves only the lease
    let wish = match fragment.get("expiresAt") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(w)) if w == NULL => Some(None),
        Some(Value::String(w)) => Some(Some(w.clone())),
        Some(_) => return bad("expiresAt is a DateTime"),
    };
    if let Some(w) = &wish {
        let until = leased(w.as_deref(), limits.subscription_lease);
        fragment.insert("expiresAt".into(), Value::String(until));
    }
    match named_endpoint(&fragment) {
        None => {}
        Some(Value::String(uri)) if *uri == conn.endpoint() => {}
        Some(_) => {
            return bad("the endpoint of a Subscription made over this socket is the socket")
        }
    }
    let path = format!("/ngsi-ld/v1/subscriptions/{}", segment(id));
    let (status, answer) = run(
        conn,
        OperationId::UpdateSubscription,
        json_request(Method::PATCH, path, &fragment),
    )
    .await;
    if status != StatusCode::NO_CONTENT.as_u16() {
        return (status, answer);
    }
    if let Some(show) = show_changes(&fragment) {
        conn.record(id, show);
    }
    if let Some(w) = wish {
        conn.want(id, w);
    }
    (status, retrieve(conn, id).await)
}

/// `x-unsubscribe` and the end of the grace period: Delete Subscription.
pub(crate) async fn unsubscribe(conn: &Conn, id: &str) -> (u16, Option<Value>) {
    if !conn.owns(id) {
        return not_found();
    }
    let req = OpsRequest::new(
        Method::DELETE,
        format!("/ngsi-ld/v1/subscriptions/{}", segment(id)),
    );
    let (status, answer) = run(conn, OperationId::DeleteSubscription, req).await;
    if status == StatusCode::NO_CONTENT.as_u16() || status == StatusCode::NOT_FOUND.as_u16() {
        conn.forget(id);
    }
    (status, answer)
}

/// One attached socket and what it has seen.
struct Session {
    hub: Arc<Hub>,
    conn: Arc<Conn>,
    generation: u64,
    socket: WebSocket,
    heard: Instant,
    ponged: Instant,
    /// When this socket last renewed the connection's leases.
    renewed: Instant,
    first: bool,
    /// What the client may still send (W9).
    rate: Rate,
}

/// A token bucket over the frames a client sends: `burst` at once, then
/// `per_sec` a second. It runs before a message is parsed, so a flood of
/// `x-subscribe`, `x-update` or Pings costs one comparison a frame.
#[derive(Debug)]
pub(crate) struct Rate {
    tokens: f64,
    at: Instant,
    per_sec: f64,
    burst: f64,
}

impl Rate {
    pub(crate) fn new(limits: &Limits, now: Instant) -> Self {
        let burst = f64::from(limits.message_burst);
        Self {
            tokens: burst,
            at: now,
            per_sec: f64::from(limits.messages_per_sec),
            burst,
        }
    }

    /// Whether one more frame at `now` stays within the bound.
    pub(crate) fn take(&mut self, now: Instant) -> bool {
        let earned = now.duration_since(self.at).as_secs_f64() * self.per_sec;
        self.tokens = (self.tokens + earned).min(self.burst);
        self.at = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

impl Session {
    /// Write one frame. A peer that does not take it within `pong_within`
    /// is gone (rule 6): a client that stops reading cannot hold the
    /// session in a write while its queue overflows.
    async fn write(&mut self, message: Message) -> bool {
        let within = self.hub.limits.pong_within;
        matches!(timeout(within, self.socket.send(message)).await, Ok(Ok(())))
    }

    /// Write one text frame.
    async fn send(&mut self, text: String) -> bool {
        self.write(Message::Text(text.into())).await
    }

    /// Write each message in order; false when the socket broke.
    async fn send_all(&mut self, texts: Vec<String>) -> bool {
        for t in texts {
            if !self.send(t).await {
                return false;
            }
        }
        true
    }

    /// Write the close frame, then wait up to [`CLOSE_ANSWER`] for the
    /// client's (RFC 6455 7.1.1), discarding what it sent meanwhile. A socket
    /// dropped with unread frames (a Pong on its way) answers with a reset,
    /// and the client can lose the `x-closing` and the close frame before it
    /// reads them (T-5245).
    async fn close(&mut self, code: u16, reason: &'static str) {
        let frame = CloseFrame {
            code: CloseCode::from(code),
            reason: reason.into(),
        };
        // a socket the write found broken ends the read at once
        let _closed = self.write(Message::Close(Some(frame))).await;
        let _answered = timeout(CLOSE_ANSWER, async {
            while let Some(Ok(_)) = self.socket.next().await {}
        })
        .await;
    }

    /// The server closes: `x-closing`, then the close frame.
    async fn closing(&mut self, code: u16, reason: &'static str) -> End {
        let _closing = self.send(envelope::closing(reason)).await;
        self.close(code, reason).await;
        End::Closed(reason)
    }

    /// A frame that breaks RFC 6455 closes with the code 7.4.1 gives it;
    /// any other read error, a dropped connection included, means the peer
    /// is gone.
    async fn broken(&mut self, e: &WsError) -> End {
        let (code, reason) = match e {
            WsError::Protocol(p) if !matches!(p, ProtocolError::ResetWithoutClosingHandshake) => {
                (code::PROTOCOL, "protocol error")
            }
            WsError::Utf8(_) => (code::NOT_UTF8, "text is not UTF-8"),
            WsError::Capacity(_) => (code::TOO_BIG, "message too big"),
            _ => return End::Gone,
        };
        self.close(code, reason).await;
        End::Closed(reason)
    }

    /// A frame from the client.
    async fn incoming(&mut self, frame: Option<Result<Message, WsError>>) -> Option<End> {
        if matches!(frame, Some(Ok(_))) && !self.rate.take(Instant::now()) {
            tracing::warn!(connection = %self.conn.id, tenant = %self.conn.tenant, "websocket client past its message rate");
            return Some(self.closing(code::POLICY, "rate").await);
        }
        let text = match frame {
            Some(Ok(Message::Text(t))) => t,
            Some(Ok(Message::Pong(_))) => {
                self.ponged = Instant::now();
                return None;
            }
            Some(Ok(Message::Ping(_) | Message::Frame(_))) => return None,
            Some(Ok(Message::Binary(_))) => {
                self.close(code::UNSUPPORTED, "binary frames are not read")
                    .await;
                return Some(End::Closed("binary"));
            }
            Some(Err(e)) => return Some(self.broken(&e).await),
            Some(Ok(Message::Close(_))) | None => {
                // tungstenite queued the answering close frame (RFC 6455
                // 5.5.1); it reaches the client only when flushed.
                let _flushed = timeout(self.hub.limits.pong_within, self.socket.flush()).await;
                return Some(End::Gone);
            }
        };
        self.heard = Instant::now();
        let first = std::mem::take(&mut self.first);
        let message = match envelope::read(text.as_str()) {
            Ok(m) => m,
            Err(Unread::Close(why)) => {
                self.close(code::TOO_BIG, why).await;
                return Some(End::Closed("bound"));
            }
            Err(Unread::Bad(why)) => {
                let r = envelope::result(None, 400, Some(envelope::bad_request(&why)));
                return (!self.send(r).await).then_some(End::Gone);
            }
        };
        let id = message.id;
        let (status, body) = match message.op {
            ClientOp::Subscribe(body) => subscribe(&self.conn, &self.hub.limits, body).await,
            ClientOp::Update {
                subscription,
                fragment,
            } => update(&self.conn, &self.hub.limits, &subscription, fragment).await,
            ClientOp::Unsubscribe { subscription } => unsubscribe(&self.conn, &subscription).await,
            ClientOp::Resume {
                connection,
                token,
                seq,
            } => {
                return self
                    .resume(id.as_deref(), first, &connection, &token, seq)
                    .await
            }
        };
        let r = envelope::result(id.as_deref(), status, body);
        (!self.send(r).await).then_some(End::Gone)
    }

    /// `x-resume` (rule 7): the first message of a new socket takes over a
    /// connection within its grace period; every failure is the same 404.
    async fn resume(
        &mut self,
        id: Option<&str>,
        first: bool,
        connection: &str,
        token: &str,
        seq: u64,
    ) -> Option<End> {
        let resumed = first
            .then(|| self.hub.resumable(connection, token, &self.conn))
            .flatten();
        let Some(old) = resumed else {
            let r = envelope::result(id, 404, Some(envelope::not_found()));
            return (!self.send(r).await).then_some(End::Gone);
        };
        self.hub.close(&self.conn.id);
        self.conn = old;
        self.generation = self.conn.attach();
        let mut replay = vec![envelope::result(id, 200, None)];
        replay.extend(self.conn.replay(seq));
        if !self.send_all(replay).await {
            return Some(End::Gone);
        }
        // the leases did not move while no socket held the connection
        renew(&self.conn, self.hub.limits.subscription_lease).await;
        self.renewed = Instant::now();
        // what was queued while the socket was away follows
        self.conn.wake.notify_one();
        None
    }

    /// The queue has something (or overflowed, or another socket took the
    /// connection).
    async fn drain(&mut self) -> Option<End> {
        match self.conn.next(self.generation, &self.hub.limits) {
            None => Some(End::TakenOver),
            Some(Next::Overflow(from, to)) => {
                let _gap = self.send(envelope::gap(from, to)).await;
                Some(self.closing(code::TRY_AGAIN, "overflow").await)
            }
            Some(Next::Send(texts)) => (!self.send_all(texts).await).then_some(End::Gone),
        }
    }

    /// The heartbeat tick: takeover, a removed view, a missing Pong, idle,
    /// then the Ping.
    async fn tick(&mut self) -> Option<End> {
        let now = Instant::now();
        if !self.conn.is_attached(self.generation) {
            return Some(End::TakenOver);
        }
        if let Runner::View(ops, view) = &self.conn.runner {
            if ops.view(view).is_none() {
                return Some(self.closing(code::POLICY, "view-removed").await);
            }
        }
        let limits = &self.hub.limits;
        if now.duration_since(self.ponged) > limits.pong_within {
            return Some(End::Gone);
        }
        if self.conn.subscription_count() == 0 && now.duration_since(self.heard) > limits.idle {
            return Some(self.closing(code::NORMAL, "idle").await);
        }
        if now.duration_since(self.renewed)
            >= limits.subscription_lease.checked_div(3).unwrap_or_default()
        {
            self.renewed = now;
            renew(&self.conn, limits.subscription_lease).await;
        }
        let ping = self.write(Message::Ping(Vec::new().into())).await;
        (!ping).then_some(End::Gone)
    }

    async fn run(&mut self) -> End {
        if !self
            .send(envelope::welcome(&self.conn.id, Hub::token(&self.conn)))
            .await
        {
            return End::Gone;
        }
        let limits = self.hub.limits.clone();
        let opened = Instant::now();
        let deadline = opened.checked_add(limits.max_lifetime).unwrap_or(opened);
        let mut ping = interval_at(
            opened.checked_add(limits.ping_every).unwrap_or(opened),
            limits.ping_every,
        );
        ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            let conn = self.conn.clone();
            let end = tokio::select! {
                frame = self.socket.next() => self.incoming(frame).await,
                () = conn.wake.notified() => self.drain().await,
                _ = ping.tick() => self.tick().await,
                () = sleep_until(deadline) => Some(self.closing(code::NORMAL, "lifetime").await),
            };
            if let Some(end) = end {
                return end;
            }
        }
    }
}

/// Serve `conn` on `socket` until it ends, then detach it.
pub(crate) async fn serve(hub: Arc<Hub>, conn: Arc<Conn>, socket: WebSocket) {
    let generation = conn.attach();
    tracing::info!(
        connection = %conn.id,
        tenant = %conn.tenant,
        view = conn.view.as_deref().unwrap_or(""),
        "websocket opened"
    );
    let now = Instant::now();
    let rate = Rate::new(&hub.limits, now);
    let mut session = Session {
        hub,
        conn,
        generation,
        socket,
        heard: now,
        ponged: now,
        renewed: now,
        first: true,
        rate,
    };
    let end = session.run().await;
    let Session {
        hub,
        conn,
        generation,
        ..
    } = session;
    let reason = match end {
        End::Gone => "gone",
        End::TakenOver => "resumed elsewhere",
        End::Closed(why) => why,
    };
    tracing::info!(
        connection = %conn.id,
        tenant = %conn.tenant,
        view = conn.view.as_deref().unwrap_or(""),
        reason,
        "websocket closed"
    );
    if end != End::TakenOver && conn.detach(generation) {
        linger(hub, conn, generation);
    }
}

/// Rule 7: a connection without Subscriptions goes at once; one with
/// Subscriptions waits `resume_grace` for a resume, and then its
/// Subscriptions are deleted like any client delete.
fn linger(hub: Arc<Hub>, conn: Arc<Conn>, generation: u64) {
    if conn.subscription_count() == 0 {
        hub.close(&conn.id);
        return;
    }
    let grace = hub.limits.resume_grace;
    tokio::spawn(async move {
        tokio::time::sleep(grace).await;
        // a resume in the meantime attached a later generation and took it
        if !conn.detached_since(generation) {
            return;
        }
        for sub in conn.subscriptions() {
            let _deleted = unsubscribe(&conn, &sub).await;
        }
        hub.close(&conn.id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W9: the burst passes, the next frame is refused, and the pace earns
    /// one frame back; nothing past the burst is saved up.
    #[test]
    fn the_message_rate_holds_a_burst_then_the_pace() {
        let limits = Limits {
            messages_per_sec: 10,
            message_burst: 3,
            ..Limits::default()
        };
        let start = Instant::now();
        let mut rate = Rate::new(&limits, start);
        assert!((0..3).all(|_| rate.take(start)), "the burst");
        assert!(!rate.take(start), "one past the burst");
        let later = start + std::time::Duration::from_millis(100);
        assert!(rate.take(later), "a tenth of a second earns one frame");
        assert!(!rate.take(later));
        let idle = later + std::time::Duration::from_secs(60);
        assert!((0..3).all(|_| rate.take(idle)));
        assert!(!rate.take(idle), "idle time saves no more than the burst");
    }

    #[test]
    fn a_subscription_id_is_one_path_segment() {
        assert_eq!(
            segment("urn:ngsi-ld:Subscription:1"),
            "urn:ngsi-ld:Subscription:1"
        );
        assert_eq!(segment("urn:x:a/b?c#d e"), "urn:x:a%2Fb%3Fc%23d%20e");
    }

    #[test]
    fn a_lease_keeps_an_earlier_wish_and_passes_a_wrong_one_on() {
        let lease = std::time::Duration::from_secs(60);
        let now = chrono::Utc::now();
        let until = |s: &str| chrono::DateTime::parse_from_rfc3339(s).map(|t| t.to_utc());
        let free = until(&leased(None, lease)).unwrap_or(now);
        assert!(free > now);
        assert!(free <= chrono::Utc::now() + chrono::Duration::seconds(60));
        let held = until(&leased(Some("2999-01-01T00:00:00Z"), lease)).unwrap_or(now);
        assert!(held < chrono::Utc::now() + chrono::Duration::seconds(61));
        assert_eq!(
            leased(Some("2020-01-01T00:00:00Z"), lease),
            "2020-01-01T00:00:00Z"
        );
        assert_eq!(leased(Some("tomorrow"), lease), "tomorrow");
        // a lease past every DateTime is the last one
        assert!(leased(None, std::time::Duration::MAX).starts_with("+262142-12-31"));
    }

    #[test]
    fn the_answer_codes_are_the_http_ones() {
        assert_eq!(not_found().0, 404);
        assert_eq!(bad("x").0, 400);
    }
}
