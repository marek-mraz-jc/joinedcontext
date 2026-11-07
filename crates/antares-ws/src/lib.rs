// SPDX-License-Identifier: EUPL-1.2
//! The WebSocket notification channel (ADR-0026): one more sink, one more
//! surface, no new delivery path.
//!
//! - `GET /x/ws/ngsi-ld/v1` upgrades a connection for the Tenant the
//!   request names ([`Ws`]); `GET /x/views/{view}/ws` one bound to a view
//!   ([`ViewWs`]).
//! - Every message is the TS 104 243 envelope `{"metadata", "body"}`
//!   ([`envelope`]); a notification's is the MQTT binding's, byte for byte.
//! - The client makes its Subscriptions over the socket; each is an
//!   ordinary Create, Update or Delete Subscription run through the
//!   operation layer, with the endpoint `urn:antares:ws:<connection>`,
//!   which only that socket can name (W6).
//! - The [`Sink`] serves that endpoint: matching, throttling, the delivery
//!   policy and `pre_notify` already ran when it is asked, and it only
//!   queues into the connection's bounded queue (W4).
//!
//! ```
//! use antares_ws::{Hub, Limits, Sink, Ws};
//!
//! let hub = Hub::new(Limits::default());
//! let sink = Sink::new(hub.clone());
//! let surface = antares_ops::mount(Ws::new(hub));
//! assert_eq!(surface.prefix(), "/x/ws");
//! # let _ = sink;
//! ```
#![forbid(unsafe_code)]

pub mod envelope;
mod hub;
mod limits;
mod mqtt;
mod random;
mod socket;
mod sta_mqtt;

pub use hub::{Answer, Forwarded, Hub, Refused, Relay, ENDPOINT_PREFIX};
pub use limits::{
    Limits, ALLOWED_ORIGINS_ENV, ENV_KEYS, MAX_CONNECTIONS_ENV, MAX_CONNECTIONS_PER_TENANT_ENV,
    MAX_LIFETIME_ENV, MAX_NOTIFICATION_BYTES, MAX_SUBSCRIPTIONS_ENV, QUEUE_BYTES_ENV, QUEUE_ENV,
    RESUME_GRACE_ENV,
};

use antares_ops::{
    Caller, DeliveryError, DeliveryFuture, NgsiError, NotificationSink, Ops, Outbound, Surface,
};
use axum::extract::{Path, Request};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::json;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

/// The view root ADR-0023 rule 10 names, shared with the other view
/// surfaces.
const VIEW_ROOT: &str = "/x/views";

impl Hub {
    /// The hub of this process, under the limits the environment names
    /// (the broker checks them with [`Limits::from_env`] at startup, so a
    /// mistyped one never reaches here).
    pub fn process() -> Arc<Self> {
        static HUB: OnceLock<Arc<Hub>> = OnceLock::new();
        HUB.get_or_init(|| {
            Self::new(
                Limits::from_env(|k| std::env::var(k).ok()).unwrap_or_else(|e| {
                    tracing::error!("{e}; the WebSocket surface runs on its defaults");
                    Limits::default()
                }),
            )
        })
        .clone()
    }
}

/// The WebSocket surface of an NGSI-LD Tenant: `/x/ws/ngsi-ld/v1`.
#[derive(Clone)]
pub struct Ws(Arc<Hub>);

impl Ws {
    /// The surface over `hub`.
    #[must_use]
    pub fn new(hub: Arc<Hub>) -> Self {
        Self(hub)
    }

    /// The surface over [`Hub::process`].
    #[must_use]
    pub fn process() -> Self {
        Self(Hub::process())
    }
}

/// The WebSocket surface of a view: `/x/views/{view}/ws`.
#[derive(Clone)]
pub struct ViewWs(Arc<Hub>);

impl ViewWs {
    /// The surface over `hub`.
    #[must_use]
    pub fn new(hub: Arc<Hub>) -> Self {
        Self(hub)
    }

    /// The surface over [`Hub::process`].
    #[must_use]
    pub fn process() -> Self {
        Self(Hub::process())
    }
}

fn health(hub: &Hub, endpoint: &str) -> serde_json::Value {
    json!({
        "endpoint": endpoint,
        "limits": hub.limits.health(),
        "connections": hub.connections(),
        "overflows": hub.overflows(),
        "queued": hub.queued(),
    })
}

impl Surface for Ws {
    fn name(&self) -> &'static str {
        "ws"
    }

    fn prefix(&self) -> &'static str {
        "/x/ws"
    }

    fn version_info(&self) -> serde_json::Value {
        health(&self.0, "/x/ws/ngsi-ld/v1")
    }

    /// The channel of a Tenant serves with the feature alone (rule 2).
    fn needs_view(&self) -> bool {
        false
    }

    fn router(&self, ops: Ops) -> Router {
        let hub = self.0.clone();
        Router::new().route(
            "/ngsi-ld/v1",
            get(move |caller: Caller, req: Request| async move {
                upgrade(hub, ops, None, caller, req, Channel::Envelope)
            }),
        )
    }
}

impl Surface for ViewWs {
    fn name(&self) -> &'static str {
        "ws-views"
    }

    fn prefix(&self) -> &'static str {
        VIEW_ROOT
    }

    fn version_info(&self) -> serde_json::Value {
        health(&self.0, "/x/views/{view}/ws")
    }

    fn router(&self, ops: Ops) -> Router {
        let hub = self.0.clone();
        Router::new().route(
            "/{view}/ws",
            get(
                move |Path(view): Path<String>, caller: Caller, req: Request| async move {
                    upgrade(hub, ops, Some(view), caller, req, Channel::Envelope)
                },
            ),
        )
    }
}

/// The `SensorThings` MQTT extension of a view (OGC 18-088 section 14, read side):
/// MQTT 3.1.1 over a WebSocket at `/x/views/{view}/sta/mqtt` (T-5911).
#[derive(Clone)]
pub struct StaMqtt(Arc<Hub>);

impl StaMqtt {
    /// The surface over `hub`.
    #[must_use]
    pub fn new(hub: Arc<Hub>) -> Self {
        Self(hub)
    }

    /// The surface over [`Hub::process`].
    #[must_use]
    pub fn process() -> Self {
        Self(Hub::process())
    }
}

impl Surface for StaMqtt {
    fn name(&self) -> &'static str {
        "sta-mqtt"
    }

    fn prefix(&self) -> &'static str {
        VIEW_ROOT
    }

    fn version_info(&self) -> serde_json::Value {
        health(&self.0, "/x/views/{view}/sta/mqtt")
    }

    fn router(&self, ops: Ops) -> Router {
        let hub = self.0.clone();
        Router::new().route(
            "/{view}/sta/mqtt",
            get(
                move |Path(view): Path<String>, caller: Caller, req: Request| async move {
                    if !offers_mqtt(req.headers()) {
                        return problem(
                            StatusCode::BAD_REQUEST,
                            "BadRequestData",
                            "MQTT over WebSocket: offer the subprotocol mqtt in Sec-WebSocket-Protocol",
                        );
                    }
                    upgrade(hub, ops, Some(view), caller, req, Channel::StaMqtt)
                },
            ),
        )
    }
}

/// Whether the handshake offers the `mqtt` subprotocol (MQTT 3.1.1 section 6).
fn offers_mqtt(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|p| p.trim() == "mqtt")
}

/// What an upgraded socket speaks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Channel {
    /// The TS 104 243 envelope (ADR-0026).
    Envelope,
    /// MQTT 3.1.1, the `SensorThings` extension.
    StaMqtt,
}

fn problem(status: StatusCode, title: &str, detail: &str) -> Response {
    let kind = format!("https://uri.etsi.org/ngsi-ld/errors/{title}");
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        json!({"type": kind, "title": title, "status": status.as_u16(), "detail": detail})
            .to_string(),
    )
        .into_response()
}

/// A refused handshake: status, problem title and detail.
type Refusal = (StatusCode, &'static str, &'static str);

/// The opening handshake of RFC 6455 section 4.2.2 on an HTTP/1.1 GET: the
/// `Sec-WebSocket-Accept` value and the connection hyper hands over after
/// the 101. axum's `ws` feature does the same, and is not used: cargo
/// unifies it across the workspace, which put tungstenite into the vanilla
/// broker's axum (T-3704).
fn handshake(req: &mut Request) -> Result<(String, hyper::upgrade::OnUpgrade), Refusal> {
    let headers = req.headers();
    let lists = |name: header::HeaderName, token: &str| {
        headers.get_all(name).iter().any(|v| {
            v.to_str()
                .is_ok_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
        })
    };
    if !lists(header::CONNECTION, "upgrade") || !lists(header::UPGRADE, "websocket") {
        return Err((
            StatusCode::BAD_REQUEST,
            "BadRequestData",
            "this is a WebSocket endpoint: send Connection: upgrade and Upgrade: websocket",
        ));
    }
    if headers
        .get(header::SEC_WEBSOCKET_VERSION)
        .map(axum::http::HeaderValue::as_bytes)
        != Some(b"13")
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "BadRequestData",
            "Sec-WebSocket-Version is 13",
        ));
    }
    let Some(key) = headers.get(header::SEC_WEBSOCKET_KEY) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "BadRequestData",
            "no Sec-WebSocket-Key",
        ));
    };
    let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
    let on_upgrade = req
        .extensions_mut()
        .remove::<hyper::upgrade::OnUpgrade>()
        .ok_or((
            StatusCode::UPGRADE_REQUIRED,
            "OperationNotSupported",
            "this connection cannot be upgraded",
        ))?;
    Ok((accept, on_upgrade))
}

/// The Tenant the client named (6.3.14): one valid client Tenant, or none;
/// else the `BadRequestData` detail.
fn asked_tenant(headers: &axum::http::HeaderMap) -> Result<Option<String>, &'static str> {
    let mut named = headers.get_all("ngsild-tenant").iter();
    Ok(match (named.next(), named.next()) {
        (None, _) => None,
        (Some(t), None) => match t.to_str() {
            // 6.3.14 grammar, and no client names one of the broker's own
            // tenants (`snap-…`, the view and grant stores): F-ANT3-005
            Ok(t) if antares_ops::TenantId::new(t).is_err() => {
                return Err("NGSILD-Tenant is not a valid client Tenant (6.3.14)")
            }
            Ok(t) => Some(t.to_owned()),
            Err(_) => return Err("NGSILD-Tenant is not text"),
        },
        (Some(_), Some(_)) => return Err("one NGSILD-Tenant (6.3.14)"),
    })
}

/// The upgrade (rules 2, 8 and 10): the handshake, the `Origin` allowlist,
/// the Tenant (or the view's), the admission caps, then the socket.
fn upgrade(
    hub: Arc<Hub>,
    ops: Ops,
    view: Option<String>,
    caller: Caller,
    mut req: Request,
    channel: Channel,
) -> Response {
    let (accept, on_upgrade) = match handshake(&mut req) {
        Ok(h) => h,
        Err((status, title, detail)) => return problem(status, title, detail),
    };
    let headers = req.headers();
    if let Some(origin) = headers.get(header::ORIGIN) {
        let allowed = hub
            .limits
            .allowed_origins
            .iter()
            .any(|o| o.as_bytes() == origin.as_bytes());
        if !allowed {
            return problem(
                StatusCode::FORBIDDEN,
                "AccessDenied",
                "this Origin may not open a WebSocket",
            );
        }
    }
    let asked = match asked_tenant(headers) {
        Ok(asked) => asked,
        Err(detail) => return problem(StatusCode::BAD_REQUEST, "BadRequestData", detail),
    };
    let (mqtt_ops, view_id) = (ops.clone(), view.clone().unwrap_or_default());
    let (tenant, runner) = match &view {
        None => (
            asked.unwrap_or_else(|| "default".to_owned()),
            hub::Runner::Root(ops),
        ),
        Some(id) => {
            let Some(v) = ops.view(id) else {
                return problem(StatusCode::NOT_FOUND, "ResourceNotFound", "no such view");
            };
            let tenant = v.tenant().to_owned();
            if asked.is_some_and(|a| a != tenant) {
                return problem(
                    StatusCode::BAD_REQUEST,
                    "BadRequestData",
                    "a view serves one tenant",
                );
            }
            (tenant, hub::Runner::View(ops, id.clone()))
        }
    };
    let subject = caller.subject();
    let conn = match hub.open(tenant, view, caller, subject, runner) {
        Ok(conn) => conn,
        Err(Refused::Process | Refused::Tenant) => {
            return problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "OperationNotSupported",
                "no WebSocket connection is admitted now",
            )
        }
    };
    // tungstenite reads into a 128 KiB buffer by default, four times the
    // budget of an idle connection (ADR-0027 section 6); client messages
    // are small, and a large one is read in more chunks.
    let config = WebSocketConfig::default()
        .read_buffer_size(READ_BUFFER_BYTES)
        .max_message_size(Some(envelope::MAX_MESSAGE_BYTES))
        .max_frame_size(Some(envelope::MAX_FRAME_BYTES))
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_NOTIFICATION_BYTES.saturating_mul(2));
    tokio::spawn(async move {
        match on_upgrade.await {
            Ok(upgraded) => {
                let socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
                    hyper_util::rt::TokioIo::new(upgraded),
                    Role::Server,
                    Some(config),
                )
                .await;
                match channel {
                    Channel::Envelope => socket::serve(hub, conn, socket).await,
                    Channel::StaMqtt => {
                        sta_mqtt::serve(hub, conn, socket, mqtt_ops, view_id).await;
                    }
                }
            }
            Err(_) => hub.close(&conn.id),
        }
    });
    let mut answer = (
        StatusCode::SWITCHING_PROTOCOLS,
        [
            (header::CONNECTION, "upgrade".to_owned()),
            (header::UPGRADE, "websocket".to_owned()),
            (header::SEC_WEBSOCKET_ACCEPT, accept),
        ],
    )
        .into_response();
    if channel == Channel::StaMqtt {
        answer.headers_mut().insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            axum::http::HeaderValue::from_static("mqtt"),
        );
    }
    answer
}

/// The read buffer of one socket.
const READ_BUFFER_BYTES: usize = 4 * 1024;

/// The binding of `urn:antares:ws:` endpoints (rule 4): registered with the
/// broker's sink registry beside HTTP and MQTT.
pub struct Sink(Arc<Hub>);

impl Sink {
    /// The sink over `hub`.
    #[must_use]
    pub fn new(hub: Arc<Hub>) -> Self {
        Self(hub)
    }
}

impl NotificationSink for Sink {
    fn schemes(&self) -> &'static [&'static str] {
        &["urn"]
    }

    /// Only `urn:antares:ws:<id>`, and only while the socket `<id>` makes
    /// the Subscription (W6): the same URI over HTTP, or another socket's,
    /// is `BadRequestData`.
    fn parse_endpoint(&self, uri: &str, _notifier_info: &[(&str, &str)]) -> Result<(), NgsiError> {
        let Some(id) = uri.strip_prefix(ENDPOINT_PREFIX) else {
            return Err(NgsiError::BadRequestData(format!(
                "a urn endpoint is served only as {ENDPOINT_PREFIX}<connection> (ADR-0026)"
            )));
        };
        let own = socket::CREATING.try_with(|c| c == id).unwrap_or(false);
        if own {
            Ok(())
        } else {
            Err(NgsiError::BadRequestData(
                "a WebSocket endpoint is set by its own connection, never named (ADR-0026 rule 4)"
                    .into(),
            ))
        }
    }

    /// It writes to an inbound connection and opens no socket.
    fn network(&self) -> bool {
        false
    }

    /// The connection writes an `x-gap` over the lost notification.
    fn lost(&self, uri: &str, tenant: &str, subscription: &str) {
        if let Some(id) = uri.strip_prefix(ENDPOINT_PREFIX) {
            let _answer = self.0.lose(id, tenant, subscription);
        }
    }

    fn deliver<'a>(
        &'a self,
        uri: &'a str,
        out: &'a Outbound,
        _timeout: Duration,
    ) -> DeliveryFuture<'a> {
        Box::pin(async move {
            let id = uri
                .strip_prefix(ENDPOINT_PREFIX)
                .ok_or_else(|| DeliveryError::failed("not a WebSocket endpoint"))?;
            let n = Forwarded::of(out)
                .ok_or_else(|| DeliveryError::failed("a notification without subscriptionId"))?;
            match self.0.deliver(id, n).await {
                Answer::Queued => Ok(()),
                Answer::Overflow => Err(DeliveryError::failed("the connection's queue overflowed")),
                Answer::TooLarge => Err(DeliveryError::failed(format!(
                    "the notification is larger than {MAX_NOTIFICATION_BYTES} bytes"
                ))),
                Answer::Unknown => Err(DeliveryError::failed("no such WebSocket connection")),
            }
        })
    }
}
