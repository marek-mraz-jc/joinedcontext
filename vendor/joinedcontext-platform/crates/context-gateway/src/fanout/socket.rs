//! The sockets of the fan-out (ADR-N-049): the registry of hubs, each hub's one upstream to the
//! broker view, and each client's session.

use super::hub::{self, Answer, Hub, Kept, Out, Queue};
use crate::app::Gateway;
use crate::auth::subject::Subject;
use crate::middleware::tenancy;
use crate::resolver::Endpoint;
use axum::body::Body;
use axum::extract::ws::{self, WebSocket};
use axum::http::header::ACCEPT;
use axum::http::{HeaderMap, HeaderValue, Method};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

/// Connections one space may hold when `JC_GATEWAY_WS_PER_SPACE` names no other number
/// (API/07 §3).
pub const PER_SPACE: usize = 1000;

/// `JC_GATEWAY_WS_PER_SPACE`: the space cap, raised for a measurement (the `ws-fanout` rig).
pub fn per_space() -> usize {
    static CAP: OnceLock<usize> = OnceLock::new();
    *CAP.get_or_init(|| env_cap("JC_GATEWAY_WS_PER_SPACE", PER_SPACE))
}
/// Hubs the gateway holds when `JC_GATEWAY_WS_HUBS` names no other number (API/07 §3): each is one
/// broker WebSocket and up to `JC_GATEWAY_WS_ENTITIES` entities, so this bounds both.
pub const HUBS: usize = 64;
/// Hubs one space may hold when `JC_GATEWAY_WS_HUBS_PER_SPACE` names no other number: under the
/// broker's 50 connections per tenant, so the broker never refuses a hub the gateway admitted.
pub const HUBS_PER_SPACE: usize = 32;
/// Hubs one signed-in caller may hold in this replica, one per Endpoint they watch (PRF-03): so
/// one identity cannot take every hub of a space. Anonymous callers share one hub per public
/// Endpoint and are held by address instead.
pub const HUBS_PER_CALLER: usize = 8;

/// A positive whole number from `name`, else `default`.
fn env_cap(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

/// `JC_GATEWAY_WS_HUBS`: the hubs, and so the broker connections, of this replica.
pub fn hubs_cap() -> usize {
    static CAP: OnceLock<usize> = OnceLock::new();
    *CAP.get_or_init(|| env_cap("JC_GATEWAY_WS_HUBS", HUBS))
}

/// `JC_GATEWAY_WS_HUBS_PER_SPACE`: the hubs one space may hold in this replica.
pub fn hubs_per_space() -> usize {
    static CAP: OnceLock<usize> = OnceLock::new();
    *CAP.get_or_init(|| env_cap("JC_GATEWAY_WS_HUBS_PER_SPACE", HUBS_PER_SPACE))
}

/// Frames a client may send at once, then [`MESSAGES_PER_SEC`] a second; past that the
/// connection closes with `1008` (API/07 §3).
const MESSAGE_BURST: f64 = 100.0;
const MESSAGES_PER_SEC: f64 = 20.0;

/// How long a hub keeps its upstream after its last client left (ADR-N-049 §2.1); the default of
/// [`Gateway::ws_linger`].
pub const LINGER: Duration = Duration::from_secs(60);
/// The upstream's reconnect delay grows to this.
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// How often a hub checks its Endpoint and reports its table.
const TICK: Duration = Duration::from_secs(5);
/// The server pings this often and ends a connection that answered no ping for twice as long
/// (API/07 §3).
const PING: Duration = Duration::from_secs(30);
/// A connection with no subscription that sent nothing for this long ends (API/07 §3).
const IDLE: Duration = Duration::from_secs(60);
/// A client whose socket takes no frame for this long is not reading: it is closed as `overflow`
/// and gives its slot back, instead of parking its session on a full send buffer (T-4902).
const STALL: Duration = Duration::from_secs(10);
/// After its close frame, how long the gateway waits for the client's (T-5931).
const CLOSING: Duration = Duration::from_secs(5);
/// The entities a snapshot page asks for.
const PAGE: usize = 1000;
/// What the broker writes: a notification of up to 1 MiB (Antares `MAX_NOTIFICATION_BYTES`).
const FROM_BROKER: usize = 1024 * 1024;
/// RFC 6455 7.4.1 close codes.
const NORMAL: u16 = 1000;
const UNSUPPORTED: u16 = 1003;
const TOO_BIG: u16 = 1009;
const POLICY: u16 = 1008;
const INTERNAL: u16 = 1011;

/// `JC_GATEWAY_WS_ENTITIES`, the entity table's bound per Endpoint (ADR-N-049 §2.2).
fn bound() -> usize {
    static BOUND: OnceLock<usize> = OnceLock::new();
    *BOUND.get_or_init(|| env_cap("JC_GATEWAY_WS_ENTITIES", hub::DEFAULT_ENTITIES))
}

/// Every hub, keyed by `hub_key`: one per Endpoint and caller identity (T-4755).
/// Each holds the hub's space, its caller ([`caller_of`]) and the hub.
#[derive(Default)]
pub struct Hubs(Mutex<HashMap<String, Held>>);

/// A hub in the registry: its space, its caller ([`caller_of`], `None` for anonymous) and the hub.
type Held = (String, Option<String>, Arc<Hub>);

/// Why a connection could not join.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The space holds [`per_space`] connections.
    SpaceFull,
    /// The caller's identity is no header value.
    NoIdentity,
    /// The gateway holds [`hubs_cap`] hubs, or the space [`hubs_per_space`], each with a client.
    HubsFull,
    /// The caller holds [`HUBS_PER_CALLER`] hubs, each with a connection.
    CallerFull,
}

impl Hubs {
    /// Opens a connection on the hub of the Endpoint and the caller's identity, starting the hub
    /// and its upstream when it has none; under the registry's lock, so a hub never stops between
    /// the join and the open. A hub this join starts names `request_id` (the joining request's)
    /// to the broker and runs in the joining request's span: its lines and the broker's carry the
    /// request that started it. The id is no part of the hub's key, so callers still share hubs.
    pub fn join(
        &self,
        gateway: &Arc<Gateway>,
        endpoint: Arc<Endpoint>,
        (subject, request_id): (Subject, Option<HeaderValue>),
        principal: String,
    ) -> Result<hub::Joined, Refused> {
        let mut headers =
            upstream_headers(endpoint.tenant.as_ref(), &subject).ok_or(Refused::NoIdentity)?;
        let key = hub_key(&endpoint.slug, &headers);
        let mut hubs = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let in_space: usize = hubs
            .values()
            .filter(|(space, _, _)| *space == endpoint.space)
            .map(|(_, _, hub)| hub.connections())
            .sum();
        if in_space >= per_space() {
            return Err(Refused::SpaceFull);
        }
        let hub = match hubs.get(&key) {
            Some((_, _, hub)) => Arc::clone(hub),
            None => {
                let caller = caller_of(&headers);
                make_room(
                    &mut hubs,
                    (&endpoint.space, caller.as_deref()),
                    (hubs_cap(), hubs_per_space(), HUBS_PER_CALLER),
                )?;
                let hub = Arc::new(Hub::new(&endpoint.slug, bound()));
                hubs.insert(key, (endpoint.space.clone(), caller, Arc::clone(&hub)));
                metrics::gauge!("jc_gateway_ws_hubs").set(hubs.len() as f64);
                if let Some(id) = request_id {
                    headers.insert(tenancy::REQUEST_ID, id);
                }
                tokio::spawn(tracing::Instrument::instrument(
                    upstream(Arc::clone(gateway), Arc::clone(&hub), headers),
                    tracing::Span::current(),
                ));
                hub
            }
        };
        let (conn, queue) = hub.open(endpoint, principal, subject);
        Ok(hub::Joined::new(hub, conn, queue))
    }

    /// Judges every open connection again after a reload of the Endpoint or account tables
    /// (T-4874): what [`Hub::recheck`] names ends at once.
    pub fn recheck(&self, gateway: &Gateway) {
        let hubs = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for (_, _, hub) in hubs.values() {
            recheck(gateway, hub);
        }
    }

    /// Closes every open connection: the tables they were admitted under went stale.
    pub fn close_all(&self) {
        let hubs = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for (_, _, hub) in hubs.values() {
            let closed = hub.close_ended(|_, _| true);
            if !closed.is_empty() {
                tracing::warn!(slug = %hub.slug, closed = closed.len(), "WebSocket connections closed: the access rules went stale");
            }
        }
    }

    /// Drops `hub` when it has been without a connection for `linger`; `true` when it did, or
    /// when [`make_room`] dropped it already.
    fn leave_if_idle(&self, hub: &Arc<Hub>, linger: Duration) -> bool {
        self.leave(hub, |hub| lingered(hub, linger))
    }

    /// Drops `hub` when the registry no longer holds it or `done` says so; `true` when it is gone.
    fn leave(&self, hub: &Arc<Hub>, done: impl Fn(&Hub) -> bool) -> bool {
        let mut hubs = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let held = hubs.values().any(|(_, _, held)| Arc::ptr_eq(held, hub));
        if held && !done(hub) {
            return false;
        }
        hubs.retain(|_, (_, _, held)| !Arc::ptr_eq(held, hub));
        metrics::gauge!("jc_gateway_ws_hubs").set(hubs.len() as f64);
        true
    }
}

/// Whether `hub` has been without a connection for `linger`.
fn lingered(hub: &Hub, linger: Duration) -> bool {
    hub.idle_for().is_some_and(|idle| idle >= linger)
}

/// Makes room for one more hub of `caller` in `space`: at a cap (the caller's, the space's, the
/// replica's), drops the hub idle longest among those the cap counts (no connection, in its
/// linger), whose upstream then stops at its next tick; refused when every one has a client.
fn make_room(
    hubs: &mut HashMap<String, Held>,
    (space, caller): (&str, Option<&str>),
    (total, per_space, per_caller): (usize, usize, usize),
) -> Result<(), Refused> {
    #[derive(PartialEq)]
    enum Counted {
        Caller,
        Space,
        All,
    }
    for (cap, scope) in [
        (per_caller, Counted::Caller),
        (per_space, Counted::Space),
        (total, Counted::All),
    ] {
        let counts = |(held, by, _): &Held| match scope {
            Counted::Caller => caller.is_some() && by.as_deref() == caller,
            Counted::Space => held == space,
            Counted::All => true,
        };
        if hubs.values().filter(|held| counts(held)).count() < cap {
            continue;
        }
        let idlest = hubs
            .iter()
            .filter(|(_, held)| counts(held))
            .filter_map(|(key, (_, _, hub))| Some((hub.idle_for()?, key.clone())))
            .max();
        let (_, key) = idlest.ok_or(if scope == Counted::Caller {
            Refused::CallerFull
        } else {
            Refused::HubsFull
        })?;
        hubs.remove(&key);
    }
    Ok(())
}

/// Who a hub's upstream speaks as, from its headers: the person or account, or a participant's
/// DID; `None` for the anonymous caller.
fn caller_of(headers: &HeaderMap) -> Option<String> {
    let named: Vec<&str> = [&tenancy::SUBJECT, &tenancy::CLIENT, &tenancy::DID]
        .into_iter()
        .filter_map(|name| headers.get(name)?.to_str().ok())
        .collect();
    (!named.is_empty()).then(|| named.join("\n"))
}

/// A token bucket over the frames one client sends (API/07 §3). It runs before a message is
/// parsed, so a flood of `x-subscribe`, `x-update` or Pings costs a comparison a frame.
struct Rate {
    tokens: f64,
    at: Instant,
}

impl Rate {
    fn new(now: Instant) -> Self {
        Self {
            tokens: MESSAGE_BURST,
            at: now,
        }
    }

    /// Whether one more frame at `now` stays within the bound.
    fn take(&mut self, now: Instant) -> bool {
        let earned = now.duration_since(self.at).as_secs_f64() * MESSAGES_PER_SEC;
        self.tokens = (self.tokens + earned).min(MESSAGE_BURST);
        self.at = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

/// [`Hub::recheck`] of `hub` against the gateway's current tables.
fn recheck(gateway: &Gateway, hub: &Hub) {
    let accounts = gateway.accounts.load();
    let ended = hub.close_ended(|subject, issued| ended(gateway, &accounts, subject, issued));
    if !ended.is_empty() {
        tracing::info!(slug = %hub.slug, closed = ended.len(), "WebSocket connections closed: the caller's access ended");
    }
    let closed = hub.recheck(
        gateway.resolver.resolve(&hub.slug).as_ref(),
        |azp| accounts.admits_on(azp, &hub.slug),
        |subject, endpoint| still_reads(gateway, subject, endpoint),
    );
    if !closed.is_empty() {
        tracing::info!(slug = %hub.slug, closed = closed.len(), "WebSocket connections closed by a reload");
    }
}

/// Whether the access `subject` was admitted with, by a token issued at the unix second
/// `issued`, ended since (S-3-12): the person was revoked at or after it, the ServiceAccount left the repository or
/// its last credential ended, or the data space agreement no longer serves.
fn ended(
    gateway: &Gateway,
    accounts: &crate::auth::accounts::ServiceAccounts,
    subject: &Subject,
    issued: i64,
) -> bool {
    let revoked = subject
        .id
        .as_deref()
        .zip(gateway.verifier.as_deref())
        .and_then(|(id, verifier)| verifier.revoked_at(id))
        .is_some_and(|mark| mark >= issued);
    let now = jc_time::TimestampTz::now();
    let account_gone = subject.service_account.as_deref().is_some_and(|name| {
        let project = subject
            .service_account_project
            .as_deref()
            .unwrap_or_default();
        !accounts.holds(project, name, now)
    });
    let agreement_gone = subject
        .agreement
        .as_deref()
        .is_some_and(|id| gateway.agreements.load().serving(id, now).is_none());
    revoked || account_gone || agreement_gone
}

/// Whether `subject` is still served the WebSocket of `endpoint` after a change of its readable
/// scope (T-6776): the PDP lets it read something there, as [`crate::auth::subject::discoverable`]
/// asks for a space, and the Endpoint serves no personal attribute in public, as the upgrade
/// asks (`admit`, DM-122).
fn still_reads(gateway: &Gateway, subject: &Subject, endpoint: &Endpoint) -> bool {
    let verdict = gateway.pdp.decide(
        subject,
        jc_core::kinds::Operation::QueryEntity,
        &crate::query::requested(&[]),
        endpoint,
    );
    endpoint.public_personal_attributes().is_empty()
        && !matches!(verdict, crate::pdp::evaluator::Verdict::Deny)
}

type Upstream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The headers of a hub's upstream hops: the space's tenant and the caller's identity, as a REST
/// read through the view carries them (T-4755). The broker's view and engine decide what the
/// upstream subscription and its snapshot read, so a delivery holds only what this caller may
/// read; nothing in the gateway cuts it again. `None` when the Endpoint has no tenant.
fn upstream_headers(tenant: Option<&tenancy::Tenant>, subject: &Subject) -> Option<HeaderMap> {
    let mut request = axum::extract::Request::new(Body::empty());
    tenancy::pin_caller(&mut request, tenant, subject).ok()?;
    let mut headers = std::mem::take(request.headers_mut());
    // Provenance names a writer and one request; the upstream writes nothing, and a hub is
    // shared by identity.
    headers.remove(tenancy::PROVENANCE_CHANNEL);
    headers.remove(tenancy::PROVENANCE_ACTOR);
    headers.remove(tenancy::PROVENANCE_ORGANIZATION);
    headers.remove(tenancy::PROVENANCE_ACTOR_KIND);
    headers.remove(tenancy::PROVENANCE_CLIENT);
    headers.remove(tenancy::REQUEST_ID);
    Some(headers)
}

/// The registry key of a hub: the Endpoint and every header its upstream speaks with, so two
/// callers share a hub only when the broker cannot tell them apart.
fn hub_key(slug: &str, headers: &HeaderMap) -> String {
    let mut pairs: Vec<String> = headers
        .iter()
        .map(|(name, value)| format!("{name}={}", String::from_utf8_lossy(value.as_bytes())))
        .collect();
    pairs.sort_unstable();
    format!("{slug}\n{}", pairs.join("\n"))
}

async fn connect(gateway: &Gateway, slug: &str, headers: &HeaderMap) -> Result<Upstream, String> {
    let path = format!("/x/views/{slug}/ws");
    let base = gateway
        .broker
        .base_for(&path, headers)
        .map_err(|e| e.to_string())?;
    let authority = base
        .strip_prefix("http://")
        .ok_or("the fan-out reaches only an http:// broker")?
        .trim_end_matches('/');
    let mut request = format!("ws://{authority}{path}")
        .into_client_request()
        .map_err(|e| e.to_string())?;
    for (name, value) in headers {
        request.headers_mut().append(name.clone(), value.clone());
    }
    gateway.broker.stamp(request.headers_mut());
    let config = WebSocketConfig::default()
        .max_message_size(Some(FROM_BROKER))
        .max_frame_size(Some(FROM_BROKER));
    tokio_tungstenite::connect_async_with_config(request, Some(config), false)
        .await
        .map(|(socket, _)| socket)
        .map_err(|e| e.to_string())
}

/// The one subscription of the hub: every type the view serves, every attribute, normalized,
/// and the entity's deletion so the table lets it go.
fn upstream_subscription() -> Value {
    json!({
        "metadata": {"x-op": "x-subscribe", "x-id": "up"},
        "body": {
            "type": "Subscription",
            "entities": [{"type": "*"}],
            "notificationTrigger": ["entityCreated", "entityUpdated", "entityDeleted"],
            // The stamps tell the hub an entity's creation (`fanout::created`, F2).
            "notification": {"format": "normalized", "sysAttrs": true},
        },
    })
}

/// A hub's upstream: connect, subscribe once (or resume where the last connection ended), read
/// the snapshot, then apply every notification; reconnect with backoff; stop once the hub has
/// been idle for [`Gateway::ws_linger`].
async fn upstream(gateway: Arc<Gateway>, hub: Arc<Hub>, headers: HeaderMap) {
    let mut backoff = Duration::from_secs(1);
    // The broker's connection name, resume token and last x-seq, after a clean lifetime end.
    let mut resume: Option<(String, String, u64)> = None;
    let mut started = false;
    loop {
        if gateway.hubs.leave_if_idle(&hub, gateway.ws_linger) {
            tracing::info!(slug = %hub.slug, "the fan-out stopped: no client left");
            return;
        }
        let socket = match connect(&gateway, &hub.slug, &headers).await {
            Ok(socket) => socket,
            Err(error) => {
                tracing::warn!(slug = %hub.slug, %error, "the fan-out upstream did not open");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        backoff = Duration::from_secs(1);
        let (to_broker, from_broker) = socket.split();
        let ended = run(
            &gateway,
            &hub,
            &headers,
            to_broker,
            from_broker,
            resume.take(),
            &mut started,
        )
        .await;
        resume = ended;
        match hub.refused() {
            Some(status) if lasting(status) => hold(&gateway, &hub).await,
            Some(_) => tokio::time::sleep(MAX_BACKOFF).await,
            None => {}
        }
    }
}

/// Whether the broker answers `status` to the hub's subscription until the Endpoint changes:
/// 403 (no Policy lists the gateway), 404 (no such view) or 405 (the view does not serve
/// subscriptions). Any other refusal is retried after [`MAX_BACKOFF`].
fn lasting(status: u16) -> bool {
    matches!(status, 403..=405)
}

/// After a [`lasting`] refusal the upstream stays closed while the Endpoint resolves to the same
/// record (T-5321): a reconnect would only be refused again. The hub keeps the refusal, so a
/// client's `x-subscribe` is answered at once without a broker round. A changed record (the
/// next reconcile) opens the upstream again; a removed one, the hub idle for
/// [`Gateway::ws_linger`], or [`make_room`] taking the idle hub, stops it.
async fn hold(gateway: &Gateway, hub: &Arc<Hub>) {
    let refused_on = gateway.resolver.resolve(&hub.slug);
    tracing::info!(slug = %hub.slug, "the fan-out upstream waits for the Endpoint to change");
    let mut tick = tokio::time::interval(TICK);
    loop {
        tick.tick().await;
        recheck(gateway, hub);
        let now = gateway.resolver.resolve(&hub.slug);
        let gone = |hub: &Hub| now.is_none() || lingered(hub, gateway.ws_linger);
        if gateway.hubs.leave(hub, gone) || now != refused_on {
            return;
        }
    }
}

/// One upstream connection, the two halves of its socket, until it ends; what a resume needs
/// when it ended by its lifetime.
async fn run(
    gateway: &Arc<Gateway>,
    hub: &Arc<Hub>,
    headers: &HeaderMap,
    mut to_broker: impl futures_util::Sink<Message> + Unpin,
    mut from_broker: impl futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
    resume: Option<(String, String, u64)>,
    started: &mut bool,
) -> Option<(String, String, u64)> {
    let mut name = String::new();
    let mut token = String::new();
    let mut seq = 0_u64;
    let mut tick = tokio::time::interval(TICK);
    let first = match &resume {
        Some((name, token, seq)) => json!({"metadata": {
            "x-op": "x-resume", "x-id": "resume",
            "x-connection": name, "x-resume-token": token, "x-seq": seq}}),
        None => upstream_subscription(),
    };
    if let Some((_, _, last)) = &resume {
        seq = *last;
    }
    let mut sent_first = false;
    loop {
        let frame = tokio::select! {
            frame = from_broker.next() => frame,
            // A first filter on a hub whose table is due: read it now (T-5182).
            () = hub.snapshot_wanted(), if *started => {
                read_if_wanted(gateway, hub, headers);
                continue;
            }
            _ = tick.tick() => {
                // The reload rechecks at once (Hubs::recheck); this tick catches a connection
                // that joined on the old table while the reload ran.
                recheck(gateway, hub);
                if hub.rescope_now() {
                    tokio::spawn(rescope(Arc::clone(gateway), Arc::clone(hub), headers.clone()));
                }
                // ponytail: an Endpoint with several identities' hubs reports the last one's
                // table; a sum over the registry when a dashboard needs the Endpoint's total.
                metrics::gauge!("jc_gateway_ws_entities", "endpoint" => hub.slug.clone())
                    .set(hub.entities() as f64);
                if gateway.hubs.leave_if_idle(hub, gateway.ws_linger) {
                    let _ = to_broker.close().await;
                    return None;
                }
                continue;
            }
        };
        let text = match frame {
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Close(_))) | None => return lost(hub, &name, &token, seq),
            Some(Ok(_)) => continue,
            Some(Err(error)) => {
                tracing::warn!(slug = %hub.slug, %error, "the fan-out upstream failed");
                return lost(hub, &name, &token, seq);
            }
        };
        let Ok((metadata, body)) = hub::envelope(text.as_str()) else {
            continue;
        };
        let op = metadata.get("x-op").and_then(Value::as_str).unwrap_or("");
        match op {
            "x-welcome" => {
                name = text_of(&metadata, "x-connection");
                token = text_of(&metadata, "x-resume-token");
                if !sent_first {
                    sent_first = true;
                    if to_broker
                        .send(Message::text(first.to_string()))
                        .await
                        .is_err()
                    {
                        return None;
                    }
                }
            }
            "x-result" => {
                let status = metadata
                    .get("x-status")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                match (text_of(&metadata, "x-id").as_str(), status) {
                    ("resume", 200..=299) | ("up", 200..=299) if *started => {}
                    ("up", 200..=299) => {
                        *started = true;
                        hub.live();
                        read_if_wanted(gateway, hub, headers);
                    }
                    ("resume", _) => {
                        // The broker no longer holds our place: subscribe again, read again,
                        // and tell every client it may have missed something.
                        seq = 0;
                        hub.gap();
                        let again = upstream_subscription().to_string();
                        if to_broker.send(Message::text(again)).await.is_err() {
                            return None;
                        }
                        read_if_wanted(gateway, hub, headers);
                    }
                    (id, status) => {
                        tracing::warn!(slug = %hub.slug, id, status, ?body, "the broker refused the fan-out subscription");
                        // Never a silent 201: the clients learn it (ADR-N-049 §2 item 10).
                        let detail = body
                            .as_ref()
                            .and_then(|b| b.get("detail"))
                            .and_then(Value::as_str)
                            .unwrap_or("no detail");
                        hub.refuse(u16::try_from(status).unwrap_or(0), detail.to_owned());
                        *started = false;
                        return None;
                    }
                }
            }
            "x-notification" => {
                seq = metadata.get("x-seq").and_then(Value::as_u64).unwrap_or(seq);
                let now = jc_time::TimestampTz::now();
                for entity in body
                    .as_ref()
                    .and_then(|b| b.get("data"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    hub.apply(entity.clone(), now);
                }
            }
            "x-gap" => {
                hub.gap();
                read_if_wanted(gateway, hub, headers);
            }
            "x-closing" => return lost(hub, &name, &token, seq),
            _ => {}
        }
    }
}

/// The upstream ended: a later connection resumes it (the broker holds it 30 s).
fn lost(hub: &Hub, name: &str, token: &str, seq: u64) -> Option<(String, String, u64)> {
    tracing::info!(slug = %hub.slug, seq, "the fan-out upstream ended; it resumes");
    (!name.is_empty()).then(|| (name.to_owned(), token.to_owned(), seq))
}

fn text_of(metadata: &Map<String, Value>, key: &str) -> String {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Snapshots read at once across every hub: a sweep that subscribes on many Endpoints at once
/// would otherwise hold a page of each in memory together (T-5182).
const SNAPSHOTS: usize = 2;

/// The smallest page a snapshot reads when a page passes the body cap.
const SMALLEST_PAGE: usize = 50;

/// Starts [`snapshot`] when the hub's table is due and a filter needs it.
fn read_if_wanted(gateway: &Arc<Gateway>, hub: &Arc<Hub>, headers: &HeaderMap) {
    if hub.snapshot_now() {
        tokio::spawn(snapshot(
            Arc::clone(gateway),
            Arc::clone(hub),
            headers.clone(),
        ));
    }
}

/// Reads the view's entities once, page by page, into the table (ADR-N-049 §2.2), at most
/// [`SNAPSHOTS`] hubs at a time.
async fn snapshot(gateway: Arc<Gateway>, hub: Arc<Hub>, headers: HeaderMap) {
    read_view(&gateway, &hub.slug, headers, |entity| hub.seed(entity)).await;
}

/// Reads the view again after a change of its readable scope and hands the read to
/// [`Hub::rescope`] (T-6776); a read that fails ends every connection, as a change did before.
/// ponytail: the read is held whole beside the table (up to twice the table's bound for a
/// moment); a paged merge by id when a view near the bound changes its Policy often.
async fn rescope(gateway: Arc<Gateway>, hub: Arc<Hub>, headers: HeaderMap) {
    let mut fresh = Vec::new();
    if read_view(&gateway, &hub.slug, headers, |entity| fresh.push(entity)).await {
        let before = hub.entities();
        hub.rescope(fresh, jc_time::TimestampTz::now());
        tracing::info!(slug = %hub.slug, before, after = hub.entities(), "the fan-out read its view again under a changed scope");
    } else {
        tracing::warn!(slug = %hub.slug, "the fan-out could not read its view again; its connections close");
        hub.rescope_failed();
    }
}

/// Reads the view's entities page by page into `each`, at most [`SNAPSHOTS`] hubs at a time;
/// `false` when a page was refused, failed or could not be read.
async fn read_view(
    gateway: &Gateway,
    slug: &str,
    headers: HeaderMap,
    mut each: impl FnMut(Value),
) -> bool {
    static TURNS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(SNAPSHOTS);
    // Nothing closes this static semaphore, so the acquire waits for a turn and never fails.
    let _turn = TURNS.acquire().await;
    let mut offset = 0;
    let mut limit = PAGE;
    while offset < bound() {
        let mut headers = headers.clone();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let path =
            format!("/x/views/{slug}/ngsi-ld/v1/entities?type=*&limit={limit}&offset={offset}");
        let answer = match gateway
            .broker
            .send(Method::GET, &path, headers, Body::empty())
            .await
        {
            Ok(answer) if answer.status().is_success() => answer,
            Ok(answer) => {
                tracing::warn!(slug, status = %answer.status(), "the fan-out snapshot was refused");
                return false;
            }
            Err(error) => {
                tracing::warn!(slug, %error, "the fan-out snapshot failed");
                return false;
            }
        };
        // A page is held to the gateway's own body cap; one past it is read again in smaller
        // pages, so a view of large entities never buffers 64 MiB per page (F-PLT1-015).
        let Ok(bytes) = axum::body::to_bytes(answer.into_body(), crate::app::MAX_BODY).await else {
            if limit > SMALLEST_PAGE {
                limit = (limit / 4).max(SMALLEST_PAGE);
                continue;
            }
            tracing::warn!(
                slug,
                limit,
                "a fan-out snapshot page is larger than the body cap"
            );
            return false;
        };
        let Ok(Value::Array(page)) = serde_json::from_slice::<Value>(&bytes) else {
            return false;
        };
        let read = page.len();
        page.into_iter().for_each(&mut each);
        if read < limit {
            return true;
        }
        offset += read;
    }
    true
}

/// How a session ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// The client closed or went away.
    Closed,
    /// The client sent a message over the bound.
    TooBig,
    /// The client sent a binary frame.
    Binary,
    /// The server ended it with this `x-closing` reason.
    Closing(&'static str),
    /// The client answered no ping.
    NoPong,
    /// The broker refused the hub's upstream subscription.
    Refused,
    /// The client broke RFC 6455; closed with this code (7.4.1).
    Broken(u16, &'static str),
}

impl Ended {
    /// The `reason` label of `jc_gateway_ws_closed_total`.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::TooBig => "too-big",
            Self::Binary => "binary",
            Self::Closing(reason) => reason,
            Self::NoPong => "no-pong",
            Self::Refused => "refused",
            Self::Broken(1007, _) => "not-utf8",
            Self::Broken(..) => "protocol-error",
        }
    }
}

/// One client connection: welcome, then its messages answered and its queue written, until it
/// ends; its subscriptions leave the index then and its place is held for a resume.
pub async fn session(
    client: WebSocket,
    hub: Arc<Hub>,
    conn: u64,
    queue: Arc<Queue>,
    principal: String,
    lifetime: Duration,
) -> Ended {
    let (mut to_client, mut from_client) = client.split();
    // Without randomness the name and token would be guessable: the session ends at once, and
    // the empty token keeps it from being held for a resume.
    let named = hub::random_name().zip(hub::random_name());
    let (name, token) = named.clone().unwrap_or_default();
    let mut seq = 0_u64;
    let mut sent = Kept::default();
    let mut ended = welcome(&mut to_client, named.as_ref()).await;
    let mut ping = tokio::time::interval(PING);
    ping.tick().await;
    // When the oldest ping the client has not answered yet went out.
    let mut unanswered: Option<Instant> = None;
    let mut last_message = Instant::now();
    let mut first = true;
    let mut rate = Rate::new(Instant::now());
    let expired = tokio::time::sleep(lifetime);
    tokio::pin!(expired);
    while ended.is_none() {
        let mut outgoing: Vec<String> = Vec::new();
        // In this order: a Pong already read is counted before a late tick judges it, and the
        // queue, ready on every turn under a backlog, cannot hold back the ping or the lifetime.
        // The client's frames cannot hold back the rest: they are rate limited.
        tokio::select! {
            biased;
            read = from_client.next() => {
                last_message = Instant::now();
                if matches!(read, Some(Ok(_))) && !rate.take(last_message) {
                    tracing::warn!(slug = %hub.slug, %principal, "WebSocket client past its message rate");
                    ended = Some(Ended::Closing("rate"));
                } else {
                match read {
                    Some(Ok(ws::Message::Text(text))) => {
                        let replies = answer(&hub, conn, &principal, text.as_str(), first, &mut seq, &mut sent);
                        first = false;
                        outgoing.extend(replies);
                    }
                    Some(Ok(ws::Message::Binary(_))) => ended = Some(Ended::Binary),
                    Some(Ok(ws::Message::Pong(_))) => unanswered = None,
                    Some(Ok(ws::Message::Ping(_))) => {}
                    Some(Ok(ws::Message::Close(_))) | None => ended = Some(Ended::Closed),
                    Some(Err(error)) => ended = Some(read_error(&error)),
                }
                }
            }
            _ = ping.tick() => {
                ended = tick(unanswered, last_message, || hub.subscriptions(conn));
                if ended.is_none() {
                    ended = deliver(&mut to_client, ws::Message::Ping(Vec::new().into())).await;
                    unanswered.get_or_insert_with(Instant::now);
                }
            }
            () = &mut expired => ended = Some(Ended::Closing("lifetime")),
            () = queue.ready() => {
                let (items, overflowed) = queue.take();
                let mut notified = Vec::new();
                for item in items {
                    match item {
                        Out::Notification { subscription, entity, body } => {
                            notified.push(entity);
                            seq += 1;
                            let text = json!({"metadata": {
                                "x-op": "x-notification", "x-seq": seq,
                                "x-subscription": subscription, "Content-Type": "application/json"},
                                "body": body}).to_string();
                            sent.push(seq, text.clone());
                            outgoing.push(text);
                        }
                        Out::Gap => {
                            seq += 1;
                            sent.mark(seq);
                            outgoing.push(gap(seq, seq));
                        }
                        Out::OutOfScope { entity, subscriptions } => {
                            seq += 1;
                            let text = json!({"metadata": {
                                "x-op": "x-out-of-scope", "x-seq": seq,
                                "Content-Type": "application/json"},
                                "body": {"id": entity, "subscriptionIds": subscriptions}}).to_string();
                            sent.push(seq, text.clone());
                            outgoing.push(text);
                        }
                        Out::Closing(reason) => {
                            // Nothing queued behind a close is sent (T-4874).
                            ended = Some(Ended::Closing(reason));
                            break;
                        }
                        Out::Refused => ended = Some(Ended::Refused),
                    }
                }
                if overflowed {
                    outgoing.push(gap(seq + 1, seq + 1));
                    ended = Some(Ended::Closing("overflow"));
                }
                if !notified.is_empty() {
                    hub.log_personal_read(conn, &notified);
                }
            }
        }
        for text in outgoing {
            if let Some(end) = deliver(&mut to_client, ws::Message::text(text)).await {
                ended = Some(end);
                break;
            }
        }
    }
    let ended = ended.unwrap_or(Ended::Closed);
    // A stalled client takes no close frame either.
    let _ = tokio::time::timeout(STALL, finish(&mut to_client, &ended)).await;
    hub.close(conn, &name, &token, sent);
    linger(&ended, &mut from_client).await;
    ended
}

/// After the gateway's own close frame, reads and discards what the client still sends until
/// its close frame, the end of its stream or [`CLOSING`]. Dropping the socket over frames the
/// gateway never read resets the connection (RST), and the client, or a proxy in between, loses
/// the close frame with it (T-5931, RFC 6455 7.1.1). After a read error (a message over the
/// bound, a broken frame) the stream yields nothing more, tokio-tungstenite ends it at the first
/// error, while the rest of the offending message is still unread: the socket is then held for
/// [`CLOSING`] unread, so the close frame reaches the client before the drop resets it (T-6191).
async fn linger(ended: &Ended, from_client: &mut SplitStream<WebSocket>) {
    match ended {
        Ended::Closed => return,
        Ended::TooBig | Ended::Broken(..) => return tokio::time::sleep(CLOSING).await,
        _ => {}
    }
    let drain = async {
        while let Some(Ok(message)) = from_client.next().await {
            if matches!(message, ws::Message::Close(_)) {
                break;
            }
        }
    };
    let _ = tokio::time::timeout(CLOSING, drain).await;
}

/// What a ping tick decides: a client that left a ping unanswered for longer than [`PING`] ends,
/// so the next tick after that one (60 s after the ping, API/07 §3), and one silent past [`IDLE`]
/// with no subscription. The deadline runs from a ping sent: a tick the gateway ran late sent
/// none, and a client is never ended for a ping it was not sent (T-5351). `subscriptions` takes the hub's lock, which
/// `Hub::apply` holds through a change's whole fan-out: it is asked only of a silent client, since
/// every connection asking on each tick parked the runtime's threads behind a large hub and no
/// Pong was read in time (T-5351).
fn tick(
    unanswered: Option<Instant>,
    last_message: Instant,
    subscriptions: impl FnOnce() -> usize,
) -> Option<Ended> {
    if unanswered.is_some_and(|sent| sent.elapsed() > PING) {
        Some(Ended::NoPong)
    } else if last_message.elapsed() > IDLE && subscriptions() == 0 {
        Some(Ended::Closing("idle"))
    } else {
        None
    }
}

/// Greets the client with its connection name and resume token: how the session ends when it
/// cannot, with no name (no randomness) or with the client gone.
async fn welcome(
    to_client: &mut (impl futures_util::Sink<ws::Message> + Unpin),
    named: Option<&(String, String)>,
) -> Option<Ended> {
    let Some((name, token)) = named else {
        return Some(Ended::Closing("unavailable"));
    };
    let welcome =
        json!({"metadata": {"x-op": "x-welcome", "x-connection": name, "x-resume-token": token}});
    match to_client.send(ws::Message::text(welcome.to_string())).await {
        Ok(()) => None,
        Err(_) => Some(Ended::Closed),
    }
}

/// Sends one frame within [`STALL`]: how the session ends when it cannot.
async fn deliver(
    to_client: &mut (impl futures_util::Sink<ws::Message> + Unpin),
    message: ws::Message,
) -> Option<Ended> {
    match tokio::time::timeout(STALL, to_client.send(message)).await {
        Ok(Ok(())) => None,
        Ok(Err(_)) => Some(Ended::Closed),
        Err(_) => Some(Ended::Closing("overflow")),
    }
}

/// The close frame that ends a session. After the client's own close, the answering close frame
/// tungstenite queued (RFC 6455 5.5.1) reaches the client only when the sink is flushed.
async fn finish(to_client: &mut SplitSink<WebSocket, ws::Message>, ended: &Ended) {
    if let Ended::Closing(reason) = ended {
        let closing = json!({"metadata": {"x-op": "x-closing", "x-reason": reason}});
        let _ = to_client.send(ws::Message::text(closing.to_string())).await;
    }
    let close = close_frame(ended);
    if let Some((code, reason)) = close {
        let frame = ws::CloseFrame {
            code,
            reason: reason.into(),
        };
        let _ = to_client.send(ws::Message::Close(Some(frame))).await;
    } else {
        let _ = to_client.flush().await;
    }
}

/// The code and reason of the close frame `ended` sends; `None` when the client is gone.
fn close_frame(ended: &Ended) -> Option<(u16, &'static str)> {
    match ended {
        Ended::Closing(reason) => Some((if *reason == "rate" { POLICY } else { NORMAL }, *reason)),
        Ended::TooBig => Some((TOO_BIG, "a message over 64 KiB")),
        Ended::Binary => Some((UNSUPPORTED, "text frames only")),
        Ended::NoPong => Some((NORMAL, "no pong")),
        Ended::Refused => Some((
            INTERNAL,
            "the context broker refuses this Endpoint's fan-out",
        )),
        Ended::Broken(code, reason) => Some((*code, *reason)),
        Ended::Closed => None,
    }
}

fn gap(from: u64, to: u64) -> String {
    json!({"metadata": {"x-op": "x-gap", "x-from": from, "x-to": to}}).to_string()
}

/// How a failed read ends the session: a broken bound or frame closes with its code, any other
/// error, a dropped connection included, means the client is gone.
fn read_error(error: &axum::Error) -> Ended {
    use tokio_tungstenite::tungstenite::{error::ProtocolError, Error};
    match std::error::Error::source(error).and_then(|s| s.downcast_ref::<Error>()) {
        Some(Error::Capacity(_)) => Ended::TooBig,
        Some(Error::Protocol(p)) if !matches!(p, ProtocolError::ResetWithoutClosingHandshake) => {
            Ended::Broken(1002, "protocol error")
        }
        Some(Error::Utf8(_)) => Ended::Broken(1007, "text is not UTF-8"),
        _ => Ended::Closed,
    }
}

/// The messages that answer one client message, in order.
fn answer(
    hub: &Hub,
    conn: u64,
    principal: &str,
    text: &str,
    first: bool,
    seq: &mut u64,
    sent: &mut Kept,
) -> Vec<String> {
    let (metadata, body) = match hub::envelope(text) {
        Ok(message) => message,
        Err(why) => return vec![result(&Map::new(), &bad(&why))],
    };
    let op = metadata.get("x-op").and_then(Value::as_str).unwrap_or("");
    let named = metadata.get("x-subscription").and_then(Value::as_str);
    let empty = Value::Object(Map::new());
    let answered = match (op, named) {
        ("x-subscribe", _) => hub.subscribe(conn, None, body.as_ref().unwrap_or(&empty), false),
        ("x-update", Some(id)) => {
            hub.subscribe(conn, Some(id), body.as_ref().unwrap_or(&empty), true)
        }
        ("x-unsubscribe", Some(id)) => hub.unsubscribe(conn, id),
        ("x-update" | "x-unsubscribe", None) => {
            bad("metadata.x-subscription names the subscription")
        }
        ("x-resume", _) if first => return resume(hub, conn, principal, &metadata, seq, sent),
        ("x-resume", _) => bad("x-resume is the first message of a connection"),
        _ => bad("metadata.x-op is x-subscribe, x-update, x-unsubscribe or x-resume"),
    };
    vec![result(&metadata, &answered)]
}

fn bad(detail: &str) -> Answer {
    Answer {
        status: 400,
        body: Some(json!({
            "type": "https://uri.etsi.org/ngsi-ld/errors/BadRequestData",
            "title": "BadRequestData",
            "status": 400,
            "detail": detail,
        })),
    }
}

fn result(metadata: &Map<String, Value>, answer: &Answer) -> String {
    let mut out = Map::new();
    out.insert("x-op".into(), json!("x-result"));
    if let Some(id) = metadata.get("x-id") {
        out.insert("x-id".into(), id.clone());
    }
    out.insert("x-status".into(), json!(answer.status));
    let mut message = Map::new();
    message.insert("metadata".into(), Value::Object(out));
    if let Some(body) = &answer.body {
        message.insert("body".into(), body.clone());
    }
    Value::Object(message).to_string()
}

/// `x-resume`: the closed connection's subscriptions move to this one, and what it sent after
/// the client's `x-seq` is sent again, an `x-gap` first when some of it is no longer held.
fn resume(
    hub: &Hub,
    conn: u64,
    principal: &str,
    metadata: &Map<String, Value>,
    seq: &mut u64,
    sent: &mut Kept,
) -> Vec<String> {
    let name = text_of(metadata, "x-connection");
    let token = text_of(metadata, "x-resume-token");
    let after = metadata.get("x-seq").and_then(Value::as_u64).unwrap_or(0);
    let Some(closed) = hub.take_closed(&name, &token, principal) else {
        let refused = Answer {
            status: 404,
            body: Some(json!({
                "type": "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound",
                "title": "ResourceNotFound",
                "status": 404,
                "detail": "no connection of yours to resume here; read again over REST",
            })),
        };
        return vec![result(metadata, &refused)];
    };
    for (id, body) in &closed.subs {
        hub.subscribe(conn, Some(id), body, false);
    }
    let mut out = Vec::new();
    // a client cannot have read past what was sent
    let last = closed.sent.last();
    let after = after.min(last);
    let missed: Vec<&(u64, String)> = closed
        .sent
        .items()
        .iter()
        .filter(|(n, _)| *n > after)
        .collect();
    if missed.first().is_some_and(|(n, _)| *n > after + 1) || (missed.is_empty() && last > after) {
        out.push(gap(after + 1, missed.first().map_or(last, |(n, _)| n - 1)));
    }
    for (n, text) in missed {
        out.push(text.clone());
        sent.push(*n, text.clone());
    }
    sent.mark(last);
    *seq = last;
    out.insert(
        0,
        result(
            metadata,
            &Answer {
                status: 200,
                body: None,
            },
        ),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::IntoFuture;
    use tokio_tungstenite::tungstenite::Error;

    fn hub_with_closed(sent: &[u64]) -> (Hub, u64) {
        let hub = Hub::new("s", 10);
        let endpoint = crate::fanout::hub::tests_support::endpoint();
        let (old, _) = hub.open(
            Arc::clone(&endpoint),
            "anonymous".into(),
            Subject::default(),
        );
        let mut held = Kept::default();
        for n in sent {
            held.push(*n, format!("n{n}"));
        }
        hub.close(old, "name", "token", held);
        let (new, _) = hub.open(endpoint, "anonymous".into(), Subject::default());
        (hub, new)
    }

    fn resumed(hub: &Hub, conn: u64, after: u64) -> (Vec<String>, u64) {
        let metadata: Map<String, Value> = serde_json::from_value(json!({
            "x-op": "x-resume", "x-id": "r", "x-connection": "name", "x-resume-token": "token", "x-seq": after
        }))
        .expect("metadata");
        let mut seq = 0;
        let mut sent = Kept::default();
        (
            resume(hub, conn, "anonymous", &metadata, &mut seq, &mut sent),
            seq,
        )
    }

    /// T-4755: the upstream speaks as the caller, as a REST read through the view does, so the
    /// broker cuts each delivery; callers share a hub only when the broker cannot tell them apart.
    #[test]
    fn the_upstream_speaks_as_the_caller_and_hubs_are_shared_by_identity() {
        let anonymous = upstream_headers(
            tenancy::Tenant::segment("helsinki").ok().as_ref(),
            &Subject::anonymous(),
        )
        .expect("headers");
        assert_eq!(tenancy::pinned_tenant(&anonymous), Some("helsinki"));
        assert_eq!(anonymous["x-jc-roles"], "public");
        assert!(anonymous.get("x-jc-client").is_none(), "{anonymous:?}");
        assert!(
            anonymous.get(tenancy::PROVENANCE_ACTOR).is_none(),
            "{anonymous:?}"
        );
        let jana = Subject {
            user: Some("jana".to_owned()),
            id: Some("3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14".to_owned()),
            roles: ["steward".to_owned()].into(),
            ..Subject::default()
        };
        let person = upstream_headers(tenancy::Tenant::segment("helsinki").ok().as_ref(), &jana)
            .expect("headers");
        // The broker matches a `user` assignee against the person's Keycloak id (PF-126).
        assert_eq!(
            person["x-jc-subject"],
            "3f2a9c1e-7b4d-4e8a-9c61-0d5e2b7f8a14"
        );
        let reconciler = Subject {
            user: Some("service-account-portal-reconciler".to_owned()),
            id: Some("0d5e2b7f-8a14-4e8a-9c61-3f2a9c1e7b4d".to_owned()),
            ..Subject::default()
        };
        let reconciler = upstream_headers(
            tenancy::Tenant::segment("helsinki").ok().as_ref(),
            &reconciler,
        )
        .expect("headers");
        assert_eq!(
            reconciler["x-jc-subject"],
            "service-account-portal-reconciler"
        );
        assert_eq!(person["x-jc-roles"], "steward");
        // A token of the gateway's own client never makes the upstream the gateway's.
        let posing = Subject {
            service_account: Some(tenancy::GATEWAY_CLIENT.to_owned()),
            ..Subject::default()
        };
        let posing = upstream_headers(tenancy::Tenant::segment("helsinki").ok().as_ref(), &posing)
            .expect("headers");
        assert!(posing.get("x-jc-client").is_none(), "{posing:?}");

        let again = upstream_headers(
            tenancy::Tenant::segment("helsinki").ok().as_ref(),
            &Subject::anonymous(),
        )
        .expect("headers");
        assert_eq!(hub_key("e", &anonymous), hub_key("e", &again));
        assert_ne!(hub_key("e", &anonymous), hub_key("e", &person));
        assert_ne!(hub_key("e", &anonymous), hub_key("f", &anonymous));
        assert!(upstream_headers(None, &Subject::anonymous()).is_none());

        let subscription = upstream_subscription();
        assert_eq!(subscription["body"]["entities"], json!([{"type": "*"}]));
        assert!(subscription["body"]["notification"]
            .get("endpoint")
            .is_none());
    }

    /// API/07 §3: at either hub cap the hub idle longest goes; one hub past the cap with every hub
    /// holding a client is refused, and the other space's hubs stay.
    #[test]
    fn hubs_are_capped_per_space_and_in_all_and_idle_ones_make_room() {
        let endpoint = crate::fanout::hub::tests_support::endpoint();
        let busy = |slug: &str| {
            let hub = Arc::new(Hub::new(slug, 10));
            hub.open(
                Arc::clone(&endpoint),
                "anonymous".into(),
                Subject::default(),
            );
            hub
        };
        let mut hubs: HashMap<String, Held> = HashMap::new();
        hubs.insert("a1".into(), ("a".into(), None, busy("a1")));
        hubs.insert("b1".into(), ("b".into(), None, busy("b1")));
        hubs.insert("a2".into(), ("a".into(), None, busy("a2")));
        assert_eq!(
            make_room(&mut hubs, ("a", None), (10, 2, 8)),
            Err(Refused::HubsFull)
        );
        assert_eq!(
            make_room(&mut hubs, ("b", None), (3, 2, 8)),
            Err(Refused::HubsFull)
        );
        assert_eq!(make_room(&mut hubs, ("b", None), (4, 2, 8)), Ok(()));

        hubs.insert(
            "a3".into(),
            ("a".into(), None, Arc::new(Hub::new("a3", 10))),
        );
        assert_eq!(
            make_room(&mut hubs, ("a", None), (10, 3, 8)),
            Ok(()),
            "a3 lingers"
        );
        assert!(!hubs.contains_key("a3"), "{:?}", hubs.keys());
        assert_eq!(hubs.len(), 3);
    }

    /// PRF-03: one caller holds at most its share of hubs, each with a client; another caller,
    /// and an anonymous one, still joins the same space. The caller's own idle hub makes room.
    #[test]
    fn one_caller_cannot_hold_every_hub_of_a_space() {
        let endpoint = crate::fanout::hub::tests_support::endpoint();
        let busy = |slug: &str| {
            let hub = Arc::new(Hub::new(slug, 10));
            hub.open(Arc::clone(&endpoint), "user:ana".into(), Subject::default());
            hub
        };
        let mut hubs: HashMap<String, Held> = HashMap::new();
        for n in 0..2 {
            let key = format!("e{n}");
            hubs.insert(key.clone(), ("a".into(), Some("ana".into()), busy(&key)));
        }
        let caps = (64, 32, 2);
        assert_eq!(
            make_room(&mut hubs, ("a", Some("ana")), caps),
            Err(Refused::CallerFull)
        );
        assert_eq!(make_room(&mut hubs, ("a", Some("ola")), caps), Ok(()));
        assert_eq!(make_room(&mut hubs, ("a", None), caps), Ok(()));
        hubs.insert(
            "idle".into(),
            (
                "b".into(),
                Some("ola".into()),
                Arc::new(Hub::new("idle", 10)),
            ),
        );
        hubs.insert(
            "e1".into(),
            ("a".into(), Some("ana".into()), Arc::new(Hub::new("e1", 10))),
        );
        assert_eq!(
            make_room(&mut hubs, ("a", Some("ana")), caps),
            Ok(()),
            "e1 lingers"
        );
        let keys = format!("{:?}", hubs.keys());
        assert!(
            !hubs.contains_key("e1") && hubs.contains_key("idle"),
            "{keys}"
        );

        let person = upstream_headers(
            tenancy::Tenant::segment("a").ok().as_ref(),
            &Subject {
                user: Some("ana".into()),
                id: Some("5b1e".into()),
                ..Subject::default()
            },
        )
        .expect("headers");
        assert_eq!(caller_of(&person).as_deref(), Some("5b1e"));
        let anonymous = upstream_headers(
            tenancy::Tenant::segment("a").ok().as_ref(),
            &Subject::anonymous(),
        )
        .expect("headers");
        assert_eq!(caller_of(&anonymous), None);
    }

    /// API/07 §3: the burst passes, the next frame is refused, a twentieth of a second earns one
    /// back, and idle time saves no more than the burst.
    #[test]
    fn the_message_rate_holds_a_burst_then_the_pace() {
        let start = Instant::now();
        let mut rate = Rate::new(start);
        assert!((0..100).all(|_| rate.take(start)));
        assert!(!rate.take(start), "one past the burst");
        let later = start + Duration::from_millis(50);
        assert!(rate.take(later));
        assert!(!rate.take(later));
        let idle = later + Duration::from_secs(60);
        assert!((0..100).all(|_| rate.take(idle)));
        assert!(!rate.take(idle));
    }

    #[test]
    fn a_resume_sends_what_was_missed_after_the_clients_seq() {
        let (hub, conn) = hub_with_closed(&[3, 4, 5]);
        let (out, seq) = resumed(&hub, conn, 3);
        assert!(out[0].contains("\"x-status\":200"), "{out:?}");
        assert_eq!(&out[1..], ["n4", "n5"]);
        assert_eq!(seq, 5);
    }

    /// S2-24: a notification past the byte bound is not kept, and a resume after it answers a
    /// gap instead of nothing.
    #[test]
    fn a_resume_past_an_evicted_notification_answers_a_gap() {
        let hub = Hub::new("s", 10);
        let endpoint = crate::fanout::hub::tests_support::endpoint();
        let (old, _) = hub.open(
            Arc::clone(&endpoint),
            "anonymous".into(),
            Subject::default(),
        );
        let mut held = Kept::default();
        held.push(3, "n3".to_owned());
        held.push(4, "y".repeat(hub::KEPT_BYTES + 1));
        hub.close(old, "name", "token", held);
        let (conn, _) = hub.open(endpoint, "anonymous".into(), Subject::default());
        let (out, seq) = resumed(&hub, conn, 3);
        assert!(
            out[1].contains("x-gap")
                && out[1].contains("\"x-from\":4")
                && out[1].contains("\"x-to\":4"),
            "{out:?}"
        );
        assert_eq!((out.len(), seq), (2, 4));
    }

    #[test]
    fn a_resume_from_before_what_is_held_starts_with_a_gap() {
        let (hub, conn) = hub_with_closed(&[3, 4, 5]);
        let (out, _) = resumed(&hub, conn, 1);
        assert!(
            out[1].contains("x-gap")
                && out[1].contains("\"x-from\":2")
                && out[1].contains("\"x-to\":2"),
            "{out:?}"
        );
        assert_eq!(&out[2..], ["n3", "n4", "n5"]);
    }

    #[test]
    fn a_resume_of_an_unknown_connection_is_404() {
        let (hub, conn) = hub_with_closed(&[1]);
        let metadata: Map<String, Value> = serde_json::from_value(
            json!({"x-op": "x-resume", "x-connection": "other", "x-resume-token": "token"}),
        )
        .expect("metadata");
        let out = resume(
            &hub,
            conn,
            "anonymous",
            &metadata,
            &mut 0,
            &mut Kept::default(),
        );
        assert!(out[0].contains("\"x-status\":404"), "{out:?}");
    }

    #[test]
    fn a_message_out_of_place_is_answered_400_and_the_connection_stays() {
        let (hub, conn) = hub_with_closed(&[]);
        for (text, says) in [
            ("{", "one JSON object"),
            (
                r#"{"metadata": {"x-op": "x-dance", "x-id": "a"}}"#,
                "metadata.x-op",
            ),
            (
                r#"{"metadata": {"x-op": "x-update"}, "body": {}}"#,
                "x-subscription",
            ),
        ] {
            let out = answer(
                &hub,
                conn,
                "anonymous",
                text,
                false,
                &mut 0,
                &mut Kept::default(),
            );
            assert!(
                out[0].contains("\"x-status\":400") && out[0].contains(says),
                "{text}: {out:?}"
            );
        }
        let late = answer(
            &hub,
            conn,
            "anonymous",
            r#"{"metadata": {"x-op": "x-resume", "x-id": "r"}}"#,
            false,
            &mut 0,
            &mut Kept::default(),
        );
        assert!(late[0].contains("first message"), "{late:?}");
        let echoed = answer(
            &hub,
            conn,
            "anonymous",
            r#"{"metadata": {"x-op": "x-subscribe", "x-id": "s1"}, "body": {"type": "Subscription", "entities": [{"type": "Vehicle"}]}}"#,
            true,
            &mut 0,
            &mut Kept::default(),
        );
        assert!(
            echoed[0].contains("\"x-id\":\"s1\"") && echoed[0].contains("\"x-status\":201"),
            "{echoed:?}"
        );
    }

    /// A ping tick asks the hub only about a silent client, and ends one only for a ping it was
    /// sent: the gateway's own late tick sent none (T-5351).
    #[test]
    fn a_tick_asks_the_hub_only_about_a_silent_client() {
        let now = Instant::now();
        let long_ago = now.checked_sub(PING * 3).expect("an instant");
        let asked = std::cell::Cell::new(0);
        let unasked = || -> usize {
            asked.set(asked.get() + 1);
            0
        };
        assert_eq!(tick(None, now, unasked), None);
        assert_eq!(tick(Some(now), now, unasked), None);
        assert_eq!(asked.get(), 0, "the hub's lock was taken for a live client");
        assert_eq!(tick(None, long_ago, unasked), Some(Ended::Closing("idle")));
        assert_eq!(asked.get(), 1, "a silent client's subscriptions are asked");
        assert_eq!(tick(Some(long_ago), now, unasked), Some(Ended::NoPong));
        assert_eq!(tick(None, long_ago, || 0), Some(Ended::Closing("idle")));
        assert_eq!(tick(None, long_ago, || 1), None);
    }

    /// A session behind a real upgrade, for the RFC 6455 tests.
    async fn served() -> String {
        served_with(|_| {}).await
    }

    /// [`served`], its connection's queue handed to `prepare` before the session starts.
    async fn served_with(prepare: fn(&Queue)) -> String {
        let app = axum::Router::new().route(
            "/ws",
            axum::routing::get(move |up: ws::WebSocketUpgrade| async move {
                up.on_upgrade(move |client| async move {
                    let hub = Arc::new(Hub::new("s", 10));
                    let endpoint = crate::fanout::hub::tests_support::endpoint();
                    let (conn, queue) = hub.open(endpoint, "anonymous".into(), Subject::default());
                    prepare(&queue);
                    let lifetime = Duration::from_secs(60);
                    session(client, hub, conn, queue, "anonymous".into(), lifetime).await;
                })
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, app).await });
        format!("ws://{addr}/ws")
    }

    /// The code of the close frame the gateway answers `message` with, skipping its own messages.
    async fn closes_with(url: &str, message: Message) -> Option<u16> {
        let (mut ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("connect");
        ws.send(message).await.expect("send");
        close_code(&mut ws).await.0
    }

    /// The code of the next close frame on `ws` and the texts the gateway sent before it.
    async fn close_code(ws: &mut Upstream) -> (Option<u16>, Vec<String>) {
        let mut texts = Vec::new();
        loop {
            let message = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .expect("an answer in time")
                .expect("a frame")
                .expect("a readable frame");
            match message {
                Message::Close(frame) => return (frame.map(|f| f.code.into()), texts),
                Message::Text(text) => texts.push(text.to_string()),
                _ => {}
            }
        }
    }

    /// RFC 6455 5.5.1 and 7.4.1: a client's close is answered with a close frame, and a frame
    /// that breaks the protocol closes with 1002, text that is not UTF-8 with 1007.
    #[tokio::test]
    async fn a_close_is_answered_and_a_broken_frame_closes_with_its_code() {
        use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};
        use tokio_tungstenite::tungstenite::protocol::frame::Frame;
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        let url = served().await;
        let bye = CloseFrame {
            code: NORMAL.into(),
            reason: "bye".into(),
        };
        assert_eq!(
            closes_with(&url, Message::Close(Some(bye))).await,
            Some(NORMAL)
        );
        let mut rsv = Frame::message(b"{}".to_vec(), OpCode::Data(Data::Text), true);
        rsv.header_mut().rsv1 = true;
        assert_eq!(closes_with(&url, Message::Frame(rsv)).await, Some(1002));
        let reserved = Frame::message(Vec::new(), OpCode::Data(Data::Reserved(3)), true);
        assert_eq!(
            closes_with(&url, Message::Frame(reserved)).await,
            Some(1002)
        );
        let latin = Frame::message(vec![0xce, 0xba, 0xff], OpCode::Data(Data::Text), true);
        assert_eq!(closes_with(&url, Message::Frame(latin)).await, Some(1007));
    }

    /// F-PLT1-015: a snapshot page past the gateway's body cap is read again in quarter pages
    /// instead of being buffered whole.
    #[tokio::test]
    async fn a_snapshot_page_past_the_body_cap_is_read_again_in_smaller_pages() {
        let asked = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&asked);
        let broker = axum::Router::new().fallback(move |uri: axum::http::Uri| {
            let seen = Arc::clone(&seen);
            async move {
                let query = uri.query().unwrap_or_default().to_owned();
                seen.lock().expect("asked").push(query.clone());
                let big = "x".repeat(10_000);
                let page: Vec<Value> = if query.contains("offset=0") && query.contains("limit=1000")
                {
                    (0..1000)
                        .map(|n| json!({ "id": format!("urn:ngsi-ld:A:{n}"), "big": big }))
                        .collect()
                } else {
                    vec![json!({ "id": "urn:ngsi-ld:A:1" })]
                };
                axum::Json(Value::Array(page))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        tokio::spawn(async move { axum::serve(listener, broker).await });
        let gateway = Arc::new(Gateway::new(
            crate::proxy::Broker::new(url),
            Box::new(crate::pdp::PolicyPdp),
            "hel.fi",
        ));
        snapshot(gateway, Arc::new(Hub::new("s", 10)), HeaderMap::new()).await;
        let asked = asked.lock().expect("asked").clone();
        assert_eq!(
            asked,
            ["type=*&limit=1000&offset=0", "type=*&limit=250&offset=0"],
            "{asked:?}"
        );
    }

    /// Every end of a session names its metrics reason and the close frame it sends.
    #[test]
    fn every_end_names_its_reason_and_close_frame() {
        for (ended, reason, frame) in [
            (Ended::Closed, "closed", None),
            (Ended::TooBig, "too-big", Some(TOO_BIG)),
            (Ended::Binary, "binary", Some(UNSUPPORTED)),
            (Ended::Closing("rate"), "rate", Some(POLICY)),
            (Ended::Closing("idle"), "idle", Some(NORMAL)),
            (Ended::NoPong, "no-pong", Some(NORMAL)),
            (Ended::Refused, "refused", Some(INTERNAL)),
            (Ended::Broken(1007, "utf8"), "not-utf8", Some(1007)),
            (Ended::Broken(1002, "frame"), "protocol-error", Some(1002)),
        ] {
            assert_eq!(ended.reason(), reason);
            assert_eq!(
                close_frame(&ended).map(|(code, _)| code),
                frame,
                "{ended:?}"
            );
        }
    }

    /// A subscription a client named is updated and removed by its id; an unknown id is 404.
    #[test]
    fn a_named_subscription_is_updated_and_removed() {
        let (hub, conn) = hub_with_closed(&[]);
        let ask = |text: &str| {
            answer(
                &hub,
                conn,
                "anonymous",
                text,
                false,
                &mut 0,
                &mut Kept::default(),
            )
            .remove(0)
        };
        let created = ask(
            r#"{"metadata": {"x-op": "x-subscribe", "x-id": "a"}, "body": {"type": "Subscription", "entities": [{"type": "Vehicle"}]}}"#,
        );
        let created: Value = serde_json::from_str(&created).expect("a result");
        let id = created["body"]["id"].as_str().expect("an id").to_owned();
        let update = json!({"metadata": {"x-op": "x-update", "x-id": "b", "x-subscription": id},
            "body": {"type": "Subscription", "entities": [{"type": "Bus"}]}});
        assert!(ask(&update.to_string()).contains("\"x-status\":204"));
        let remove =
            json!({"metadata": {"x-op": "x-unsubscribe", "x-id": "c", "x-subscription": id}});
        assert!(ask(&remove.to_string()).contains("\"x-status\":204"));
        assert!(ask(&remove.to_string()).contains("\"x-status\":404"));
        assert_eq!(hub.subscriptions(conn), 0);
    }

    /// A resume moves the closed connection's subscriptions to the new one.
    #[test]
    fn a_resume_takes_the_closed_connections_subscriptions_along() {
        let hub = Hub::new("s", 10);
        let endpoint = crate::fanout::hub::tests_support::endpoint();
        let (old, _) = hub.open(
            Arc::clone(&endpoint),
            "anonymous".into(),
            Subject::default(),
        );
        let body = json!({"type": "Subscription", "entities": [{"type": "Vehicle"}]});
        assert_eq!(hub.subscribe(old, None, &body, false).status, 201);
        hub.close(old, "name", "token", Kept::default());
        let (new, _) = hub.open(endpoint, "anonymous".into(), Subject::default());
        let (out, _) = resumed(&hub, new, 0);
        assert!(out[0].contains("\"x-status\":200"), "{out:?}");
        assert_eq!(hub.subscriptions(new), 1);
    }

    fn busy_hub() -> (Arc<Hub>, Arc<Queue>) {
        let hub = Arc::new(Hub::new("s", 10));
        let endpoint = crate::fanout::hub::tests_support::endpoint();
        let (_, queue) = hub.open(endpoint, "anonymous".into(), Subject::default());
        (hub, queue)
    }

    fn hold_hub(hubs: &Hubs, key: &str, hub: &Arc<Hub>) {
        hubs.0
            .lock()
            .expect("hubs")
            .insert(key.into(), ("city".into(), None, Arc::clone(hub)));
    }

    fn closing(queue: &Queue) -> Vec<&'static str> {
        let (items, _) = queue.take();
        items
            .into_iter()
            .filter_map(|item| match item {
                Out::Closing(reason) => Some(reason),
                _ => None,
            })
            .collect()
    }

    fn gateway_at(url: String) -> Gateway {
        let mut gateway = Gateway::new(
            crate::proxy::Broker::new(url),
            Box::new(crate::pdp::PolicyPdp),
            "hel.fi",
        );
        gateway.ws_linger = Duration::from_secs(3600);
        gateway
    }

    fn nowhere() -> String {
        format!("http://{}:1", std::net::Ipv4Addr::LOCALHOST)
    }

    /// A stale table closes every connection of every hub, an idle hub's none.
    #[test]
    fn close_all_ends_every_open_connection() {
        let hubs = Hubs::default();
        let (hub, queue) = busy_hub();
        hold_hub(&hubs, "busy", &hub);
        hold_hub(&hubs, "idle", &Arc::new(Hub::new("t", 10)));
        queue.push(Out::Gap);
        hubs.close_all();
        assert_eq!(closing(&queue).len(), 1);
    }

    /// API/07 §3: a space holding its cap of connections takes no more.
    #[test]
    fn a_full_space_refuses_the_next_connection() {
        let gateway = Arc::new(gateway_at(nowhere()));
        let (hub, _) = busy_hub();
        let endpoint = crate::fanout::hub::tests_support::endpoint();
        for _ in 1..per_space() {
            hub.open(
                Arc::clone(&endpoint),
                "anonymous".into(),
                Subject::default(),
            );
        }
        hold_hub(&gateway.hubs, "full", &hub);
        let joined = gateway.hubs.join(
            &gateway,
            endpoint,
            (Subject::default(), None),
            "anonymous".into(),
        );
        assert!(matches!(joined, Err(Refused::SpaceFull)));
    }

    /// S-3-12 and T-4874: a recheck ends a connection whose account left and asks the account
    /// table about a client's `azp`.
    #[test]
    fn a_recheck_ends_a_gone_account_and_asks_about_a_client() {
        let gateway = gateway_at(nowhere());
        let endpoint = crate::fanout::hub::tests_support::endpoint();
        gateway.resolver.replace([(*endpoint).clone()]);
        let hub = Hub::new("s", 10);
        let account = Subject {
            service_account: Some("kpi".into()),
            service_account_project: Some("espoo".into()),
            ..Subject::default()
        };
        let (_, gone) = hub.open(Arc::clone(&endpoint), "kpi".into(), account);
        let client = Subject {
            client: Some("app".into()),
            ..Subject::default()
        };
        let (_, kept) = hub.open(endpoint, "app".into(), client);
        recheck(&gateway, &hub);
        assert_eq!(closing(&gone).len(), 1);
        assert!(
            closing(&kept).is_empty(),
            "an azp no account names stays admitted"
        );
    }

    /// The upstream refuses a broker it has no address or no valid URL for.
    #[tokio::test]
    async fn the_upstream_does_not_open_without_a_broker_address() {
        let gateway = gateway_at("http://bad host".into());
        let error = connect(&gateway, "s", &HeaderMap::new()).await.err();
        assert!(error.is_some_and(|e| !e.is_empty()));
        let gateway = gateway_at(nowhere());
        gateway.broker.route(&crate::store::SpaceBrokers {
            by_view: [("s".to_owned(), String::new())].into(),
            ..Default::default()
        });
        let error = connect(&gateway, "s", &HeaderMap::new()).await.err();
        assert!(error.is_some_and(|e| e.contains("no address")));
    }

    /// The upstream retries a broker it cannot reach, with backoff, until its hub leaves.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn the_upstream_retries_until_its_hub_leaves() {
        let gateway = Arc::new(gateway_at("http://bad host".into()));
        let (hub, _queue) = busy_hub();
        hold_hub(&gateway.hubs, "k", &hub);
        let running = tokio::spawn(upstream(
            Arc::clone(&gateway),
            Arc::clone(&hub),
            HeaderMap::new(),
        ));
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(!running.is_finished());
        gateway.hubs.0.lock().expect("hubs").clear();
        tokio::time::timeout(Duration::from_secs(120), running)
            .await
            .expect("stopped")
            .expect("joined");
    }

    /// A broker view WebSocket for one upstream: greets with `x-welcome`, reads the gateway's
    /// first message, sends `then`, and hands back the texts it read until the gateway left.
    async fn scripted_broker(then: Vec<Message>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        let read = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(tcp).await.expect("upgrade");
            let welcome = json!({"metadata": {"x-op": "x-welcome", "x-connection": "c1", "x-resume-token": "t1"}});
            ws.send(Message::text(welcome.to_string()))
                .await
                .expect("welcome");
            let first = ws.next().await.and_then(Result::ok);
            let mut read: Vec<String> = first
                .and_then(|message| message.into_text().ok())
                .map(|text| text.to_string())
                .into_iter()
                .collect();
            for message in then {
                ws.send(message).await.expect("send");
            }
            while let Ok(Some(Ok(message))) =
                tokio::time::timeout(Duration::from_secs(2), ws.next()).await
            {
                read.extend(message.into_text().ok().map(|text| text.to_string()));
            }
            read
        });
        (format!("http://{address}"), read)
    }

    fn text(message: Value) -> Message {
        Message::text(message.to_string())
    }

    /// One upstream run against `then`: what it returned, whether it started, what the broker read.
    async fn run_against(
        then: Vec<Message>,
        resume: Option<(String, String, u64)>,
        held: bool,
    ) -> (Option<(String, String, u64)>, bool, Vec<String>, Arc<Hub>) {
        let (url, read) = scripted_broker(then).await;
        let gateway = Arc::new(gateway_at(url));
        let (hub, _queue) = busy_hub();
        if held {
            hold_hub(&gateway.hubs, "k", &hub);
        }
        let socket = connect(&gateway, "s", &HeaderMap::new())
            .await
            .expect("connect");
        let (to_broker, from_broker) = socket.split();
        let mut started = false;
        let ended = run(
            &gateway,
            &hub,
            &HeaderMap::new(),
            to_broker,
            from_broker,
            resume,
            &mut started,
        )
        .await;
        (ended, started, read.await.expect("broker"), hub)
    }

    /// ADR-N-049 §2: the upstream skips a binary frame, a message that is no envelope and an
    /// unknown op, takes a notification's x-seq and a gap, and ends resumable at `x-closing`.
    #[tokio::test]
    async fn the_upstream_reads_its_broker_until_a_closing() {
        let (ended, started, read, hub) = run_against(
            vec![
                Message::Binary(vec![1].into()),
                Message::text("not json"),
                text(json!({"metadata": {"x-op": "x-result", "x-id": "up", "x-status": 201}})),
                text(json!({"metadata": {"x-op": "x-notification", "x-seq": 7}, "body": {"data": []}})),
                text(json!({"metadata": {"x-op": "x-gap"}})),
                text(json!({"metadata": {"x-op": "x-dance"}})),
                text(json!({"metadata": {"x-op": "x-closing"}})),
            ],
            None,
            true,
        )
        .await;
        assert_eq!(ended, Some(("c1".into(), "t1".into(), 7)));
        assert!(started);
        assert!(read[0].contains("x-subscribe"), "{read:?}");
        assert_eq!(hub.refused(), None);
    }

    /// A resume the broker no longer holds subscribes again; a refused subscription is kept
    /// for the clients and ends the run.
    #[tokio::test]
    async fn a_refused_resume_subscribes_again_and_a_refusal_ends_the_run() {
        let (ended, started, read, hub) = run_against(
            vec![
                text(json!({"metadata": {"x-op": "x-result", "x-id": "resume", "x-status": 410}})),
                text(
                    json!({"metadata": {"x-op": "x-result", "x-id": "up", "x-status": 503},
                    "body": {"detail": "busy"}}),
                ),
            ],
            Some(("c0".into(), "t0".into(), 3)),
            true,
        )
        .await;
        assert_eq!(ended, None);
        assert!(!started);
        assert!(
            read[0].contains("x-resume") && read[0].contains("c0"),
            "{read:?}"
        );
        assert!(read.iter().any(|r| r.contains("x-subscribe")), "{read:?}");
        assert_eq!(hub.refused(), Some(503));
    }

    /// A sink that answers its sends with `results`, one each, in order.
    fn scripted_sink<T>(
        results: Vec<Result<(), Error>>,
    ) -> std::pin::Pin<Box<impl futures_util::Sink<T, Error = Error>>> {
        Box::pin(futures_util::sink::unfold(
            results.into_iter(),
            |mut results, _: T| async move {
                results.next().expect("a scripted send").map(|()| results)
            },
        ))
    }

    /// The broker's frames `frames`, then nothing more: the run never reads the end.
    fn broker_frames(
        frames: Vec<Value>,
    ) -> impl futures_util::Stream<Item = Result<Message, Error>> + Unpin {
        futures_util::stream::iter(frames.into_iter().map(|frame| Ok(text(frame))))
            .chain(futures_util::stream::pending())
    }

    /// A broker gone when the upstream sends its first message, or when it subscribes again after
    /// a resume the broker no longer holds, ends the run with nothing to resume; a repeated
    /// welcome is read past.
    #[tokio::test]
    async fn a_broker_gone_under_a_send_ends_the_run() {
        let gateway = Arc::new(gateway_at(nowhere()));
        let (hub, _queue) = busy_hub();
        let welcome = json!({"metadata": {"x-op": "x-welcome", "x-connection": "c1", "x-resume-token": "t1"}});
        let mut started = false;
        let ended = run(
            &gateway,
            &hub,
            &HeaderMap::new(),
            scripted_sink(vec![Err(Error::ConnectionClosed)]),
            broker_frames(vec![welcome.clone()]),
            None,
            &mut started,
        )
        .await;
        assert_eq!(ended, None);
        // A second welcome on the same upstream sends the first message no second time (the
        // sink takes one send only): the run reads on, and the broker's close is resumable
        // under the name the last welcome gave.
        let again = json!({"metadata": {"x-op": "x-welcome", "x-connection": "c2", "x-resume-token": "t2"}});
        let ended = run(
            &gateway,
            &hub,
            &HeaderMap::new(),
            scripted_sink(vec![Ok(())]),
            futures_util::stream::iter([
                Ok(text(welcome.clone())),
                Ok(text(again)),
                Ok(Message::Close(None)),
            ]),
            None,
            &mut started,
        )
        .await;
        assert_eq!(ended, Some(("c2".into(), "t2".into(), 0)));
        let lost = json!({"metadata": {"x-op": "x-result", "x-id": "resume", "x-status": 410}});
        let ended = run(
            &gateway,
            &hub,
            &HeaderMap::new(),
            scripted_sink(vec![Ok(()), Err(Error::ConnectionClosed)]),
            broker_frames(vec![welcome, lost]),
            Some(("c0".into(), "t0".into(), 3)),
            &mut started,
        )
        .await;
        assert_eq!(ended, None);
        assert!(!started);
    }

    /// A broker that closes ends the run resumable; a hub no longer held ends it on the tick.
    #[tokio::test]
    async fn the_upstream_ends_with_its_broker_or_its_hub() {
        let (ended, ..) = run_against(vec![Message::Close(None)], None, true).await;
        assert_eq!(ended, Some(("c1".into(), "t1".into(), 0)));
        let (ended, ..) = run_against(Vec::new(), None, false).await;
        assert_eq!(ended, None);
    }

    /// A refusal that may pass is retried after the longest backoff, until the hub leaves.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_passing_refusal_is_retried_after_the_backoff() {
        let (url, _read) = scripted_broker(vec![text(
            json!({"metadata": {"x-op": "x-result", "x-id": "up", "x-status": 500}}),
        )])
        .await;
        let gateway = Arc::new(gateway_at(url));
        let (hub, _queue) = busy_hub();
        hold_hub(&gateway.hubs, "k", &hub);
        let running = tokio::spawn(upstream(
            Arc::clone(&gateway),
            Arc::clone(&hub),
            HeaderMap::new(),
        ));
        while hub.refused().is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        gateway.hubs.0.lock().expect("hubs").clear();
        tokio::time::timeout(Duration::from_secs(120), running)
            .await
            .expect("stopped")
            .expect("joined");
    }

    /// A view read is false when the broker refuses, fails, answers no array or a page past
    /// the body cap at the smallest page; full pages are read up to the table's bound.
    #[tokio::test]
    async fn a_view_read_fails_on_any_bad_page_and_stops_at_the_bound() {
        let broker = axum::Router::new().fallback(|uri: axum::http::Uri| async move {
            use axum::response::IntoResponse;
            let path = uri.path();
            let query = uri.query().unwrap_or_default();
            let limit: usize = query
                .split('&')
                .find_map(|pair| pair.strip_prefix("limit="))
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            if path.contains("/refused/") {
                axum::http::StatusCode::FORBIDDEN.into_response()
            } else if path.contains("/huge/") {
                "x".repeat(crate::app::MAX_BODY + 1).into_response()
            } else if path.contains("/object/") {
                axum::Json(json!({})).into_response()
            } else {
                axum::Json(vec![json!({"id": "urn:ngsi-ld:A:1"}); limit]).into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        tokio::spawn(axum::serve(listener, broker).into_future());
        let gateway = Arc::new(gateway_at(url));
        let mut read = 0;
        fn count(read: &mut usize) -> impl FnMut(Value) + '_ {
            move |_| *read += 1
        }
        for slug in ["refused", "huge", "object"] {
            assert!(
                !read_view(&gateway, slug, HeaderMap::new(), count(&mut read)).await,
                "{slug}"
            );
        }
        let unreachable = gateway_at(nowhere());
        assert!(!read_view(&unreachable, "s", HeaderMap::new(), count(&mut read)).await);
        assert_eq!(read, 0, "a failed read hands over nothing");
        assert!(read_view(&gateway, "full", HeaderMap::new(), count(&mut read)).await);
        assert_eq!(read, bound());
        let refused = Arc::new(Hub::new("refused", 10));
        let (_, refused_queue) = refused.open(
            crate::fanout::hub::tests_support::endpoint(),
            "anonymous".into(),
            Subject::default(),
        );
        rescope(gateway, refused, HeaderMap::new()).await;
        assert_eq!(closing(&refused_queue), ["endpoint-changed"]);
    }

    /// A binary frame closes with 1003; a client's ping is no message and leaves it open.
    #[tokio::test]
    async fn a_binary_frame_closes_and_a_ping_does_not() {
        let url = served().await;
        assert_eq!(
            closes_with(&url, Message::Binary(vec![1].into())).await,
            Some(UNSUPPORTED)
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect");
        ws.send(Message::Ping(Vec::new().into()))
            .await
            .expect("ping");
        let bye = tokio_tungstenite::tungstenite::protocol::CloseFrame {
            code: NORMAL.into(),
            reason: "bye".into(),
        };
        ws.send(Message::Close(Some(bye))).await.expect("close");
        let (code, texts) = close_code(&mut ws).await;
        assert_eq!(code, Some(NORMAL));
        assert!(texts[0].contains("x-welcome"), "{texts:?}");
    }

    /// T-4902: a queue that overflowed sends a gap and closes with `overflow`.
    #[tokio::test]
    async fn an_overflowed_queue_sends_a_gap_and_closes() {
        let url = served_with(|queue| {
            for _ in 0..=hub::MAX_QUEUED {
                queue.push(Out::Gap);
            }
        })
        .await;
        let (mut ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect");
        let (code, texts) = close_code(&mut ws).await;
        assert_eq!(code, Some(NORMAL));
        assert!(texts.iter().any(|t| t.contains("overflow")), "{texts:?}");
    }

    /// The welcome is not sent without a name, and a client gone before it ends the session; a
    /// frame the client does not take within [`STALL`] ends it with `overflow`, one it cannot
    /// take ends it as closed.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_session_ends_when_it_cannot_greet_or_deliver() {
        let named = ("c1".to_owned(), "t1".to_owned());
        let mut taken = futures_util::sink::drain();
        assert_eq!(
            welcome(&mut taken, None).await,
            Some(Ended::Closing("unavailable"))
        );
        assert_eq!(welcome(&mut taken, Some(&named)).await, None);
        let mut gone = scripted_sink(vec![Err(Error::ConnectionClosed)]);
        assert_eq!(welcome(&mut gone, Some(&named)).await, Some(Ended::Closed));
        let mut gone = scripted_sink(vec![Err(Error::ConnectionClosed)]);
        let frame = ws::Message::text("x");
        assert_eq!(deliver(&mut gone, frame.clone()).await, Some(Ended::Closed));
        let mut stalled = Box::pin(futures_util::sink::unfold((), |(), _: ws::Message| {
            std::future::pending::<Result<(), Error>>()
        }));
        assert_eq!(
            deliver(&mut stalled, frame).await,
            Some(Ended::Closing("overflow"))
        );
    }

    /// API/07 §3: a client silent past [`IDLE`] with no subscription ends `idle` at the next
    /// tick, without another ping.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_silent_client_without_a_subscription_ends_idle() {
        let url = served().await;
        let (mut ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect");
        let greeted = ws.next().await.expect("a frame").expect("a readable frame");
        assert!(greeted.to_text().expect("text").contains("x-welcome"));
        // Nothing is read while the clock jumps past the idle bound, so no ping is answered:
        // the first tick finds the client silent and no ping outstanding. The frames after it
        // are read without a deadline of their own, which the paused clock would run out.
        tokio::time::advance(IDLE + PING / 2).await;
        let mut texts = Vec::new();
        let code = loop {
            match ws.next().await.expect("a frame").expect("a readable frame") {
                Message::Close(frame) => break frame.map(|f| u16::from(f.code)),
                // a ping or a binary frame reads as no text
                other => texts.push(other.to_text().unwrap_or_default().to_owned()),
            }
        };
        assert_eq!(code, Some(NORMAL));
        assert!(texts.iter().any(|t| t.contains("\"idle\"")), "{texts:?}");
    }

    /// API/07 §3: a silent client is pinged.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_silent_client_is_pinged() {
        let url = served().await;
        let (mut ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect");
        while !matches!(
            ws.next().await.expect("a frame").expect("a readable frame"),
            Message::Ping(_)
        ) {}
    }

    /// S-3-12: an open session ends when its person is revoked at or after the second its
    /// token was issued (a token issued later is a new session's), its ServiceAccount leaves the
    /// repository, or its agreement stops serving; an anonymous caller has nothing to end.
    #[test]
    fn a_session_ends_with_its_callers_access() {
        let verifier = Arc::new(jc_core::auth::token::Verifier::new("https://realm.test"));
        let gateway = Gateway::new(
            crate::proxy::Broker::new("http://127.0.0.1:1"),
            Box::new(crate::pdp::PolicyPdp),
            "hel.fi",
        )
        .authenticate(
            Arc::clone(&verifier),
            crate::auth::accounts::ServiceAccounts::new(),
            None,
        );
        let accounts = gateway.accounts.load();
        let person = Subject {
            id: Some("jana".into()),
            ..Subject::default()
        };
        assert!(!ended(&gateway, &accounts, &person, 100), "no mark");
        verifier.replace_revocations(HashMap::from([("jana".to_owned(), 100)]));
        assert!(
            ended(&gateway, &accounts, &person, 100),
            "marked the second it was issued"
        );
        assert!(
            ended(&gateway, &accounts, &person, 99),
            "marked after it was issued"
        );
        assert!(
            !ended(&gateway, &accounts, &person, 101),
            "issued after the mark"
        );
        let account = Subject {
            service_account: Some("kpi".into()),
            service_account_project: Some("espoo".into()),
            ..Subject::default()
        };
        assert!(
            ended(&gateway, &accounts, &account, 100),
            "the account left"
        );
        let consumer = Subject {
            agreement: Some("agreement-1".into()),
            ..Subject::default()
        };
        assert!(
            ended(&gateway, &accounts, &consumer, 100),
            "the agreement serves no more"
        );
        assert!(!ended(&gateway, &accounts, &Subject::default(), 100));
    }
}
