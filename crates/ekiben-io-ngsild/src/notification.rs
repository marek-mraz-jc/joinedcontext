//! The `notification` mode of the `ngsi_ld` input (E6): it receives the NGSI-LD Notifications
//! (ETSI GS CIM 009 5.3.1) of a subscription the Portal reconciler created. The input only
//! listens; it never creates, renews or deletes a subscription.
//!
//! A running input registers its route `(organization, stream, path)`. The ingress (E4) mounts
//! [`router`] (`POST /{org}/{stream}/{*path}`) or calls [`dispatch`] itself. One Notification is
//! one batch, one message per entity of `data`, and is answered only once the stream's output
//! acked it, so the broker redelivers what was not written:
//! - 204 written (or a Notification `id` already written: redelivery is deduplicated);
//! - 400 not a Notification, nothing emitted; 401 no or a wrong bearer token; 404 no such route;
//!   413 larger than `max_body_bytes`; 415 not JSON;
//! - 502 the output refused it (Bento `http_server`'s "Server error"); 408 the output did not
//!   answer within `timeout`; 503 the input is full or the same Notification is in flight.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Path};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use ekiben_bloblang::Value;
use ekiben_core::{AckResponder, Batch, BatchError, CoreError, Part, SecretString, StreamContext};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, oneshot};

use crate::config::Fields;
use crate::input::BoundedMap;

pub const NOTIFICATION_FIELDS: &[&str] = &[
    "mode",
    "path",
    "auth",
    "max_body_bytes",
    "max_in_flight",
    "timeout",
    "dedupe_cap",
];

pub const DEFAULT_MAX_BODY: usize = 1024 * 1024;
/// The ceiling of `max_body_bytes`, and the body limit of [`router`].
pub const MAX_BODY: usize = 16 * 1024 * 1024;
pub const DEFAULT_IN_FLIGHT: usize = 64;
pub const MAX_IN_FLIGHT: usize = 1024;
pub const DEFAULT_DEDUPE_CAP: usize = 10_000;
pub const MAX_DEDUPE_CAP: usize = 1_000_000;

/// What the ingress answers the broker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestAnswer {
    pub status: u16,
    pub body: String,
    pub retry_after: Option<u64>,
}

impl IngestAnswer {
    fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            retry_after: None,
        }
    }

    fn busy(body: &str) -> Self {
        Self {
            status: 503,
            body: body.to_string(),
            retry_after: Some(1),
        }
    }
}

impl IntoResponse for IngestAnswer {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut resp = (status, self.body).into_response();
        if let Some(s) = self.retry_after {
            resp.headers_mut()
                .insert(axum::http::header::RETRY_AFTER, HeaderValue::from(s));
        }
        resp
    }
}

enum Auth {
    /// The SHA-256 of the expected bearer token: compared in constant time, and the token's
    /// length leaks nothing.
    Bearer([u8; 32]),
    Public,
}

struct Delivery {
    batch: Batch,
    done: oneshot::Sender<Option<BatchError>>,
}

#[derive(Default)]
struct Dedupe {
    done: Option<BoundedMap>,
    in_flight: HashSet<String>,
}

struct Route {
    auth: Auth,
    max_body: usize,
    timeout: Duration,
    sender: mpsc::Sender<Delivery>,
    dedupe: Mutex<Dedupe>,
}

type RouteKey = (String, String, String);

fn routes() -> &'static Mutex<HashMap<RouteKey, Arc<Route>>> {
    static ROUTES: OnceLock<Mutex<HashMap<RouteKey, Arc<Route>>>> = OnceLock::new();
    ROUTES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn digest32(bytes: &[u8]) -> [u8; 32] {
    let d = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&d[..]);
    out
}

/// Whether `path` may name a route: starts with `/`, at most 256 characters, only unreserved
/// characters and `/`, no empty or dot segment.
pub fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 256
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/'))
        && (path == "/"
            || path[1..]
                .split('/')
                .all(|s| !s.is_empty() && s != "." && s != ".."))
}

/// Removes the route when the input that registered it goes, unless a newer input took it.
struct RouteGuard {
    key: RouteKey,
    route: Arc<Route>,
}

impl Drop for RouteGuard {
    fn drop(&mut self) {
        let mut map = lock(routes());
        if map
            .get(&self.key)
            .is_some_and(|r| Arc::ptr_eq(r, &self.route))
        {
            map.remove(&self.key);
        }
    }
}

/// The reader side of the mode, held by the input.
pub struct NotificationReader {
    path: String,
    auth: Option<SecretString>,
    max_body: usize,
    in_flight: usize,
    timeout: Duration,
    dedupe_cap: usize,
    guard: Option<RouteGuard>,
    rx: Option<mpsc::Receiver<Delivery>>,
}

impl NotificationReader {
    pub fn from_fields(f: &Fields<'_>, _ctx: &StreamContext) -> Result<Self, CoreError> {
        let path = f.string_or("path", "/")?;
        if !valid_path(&path) {
            return Err(f.err(format!(
                "path {path:?} must start with / and hold only letters, digits, - . _ ~ and /"
            )));
        }
        let auth = match f.object("auth")? {
            None => {
                return Err(f.err(
                    "auth is required in mode notification: auth.bearer (the token the subscription's endpoint sends), or auth.public: true",
                ))
            }
            Some(m) => {
                let wrapped = serde_json::Value::Object(m.clone());
                let a = Fields::new(&wrapped, f.component(), &["bearer", "public"])?;
                let bearer = a.str("bearer")?.filter(|s| !s.is_empty());
                let public = a.bool_or("public", false)?;
                match (bearer, public) {
                    (Some(_), true) => {
                        return Err(f.err("auth takes bearer or public: true, not both"))
                    }
                    (Some(b), false) => Some(SecretString::new(b)),
                    (None, true) => None,
                    (None, false) => {
                        return Err(f.err(
                            "auth needs bearer, or public: true to accept unauthenticated Notifications",
                        ))
                    }
                }
            }
        };
        let max_body = f.bytes_or("max_body_bytes", DEFAULT_MAX_BODY)?;
        if max_body == 0 || max_body > MAX_BODY {
            return Err(f.err(format!("max_body_bytes must be between 1 and {MAX_BODY}")));
        }
        let in_flight = f.u64_or("max_in_flight", DEFAULT_IN_FLIGHT as u64)?;
        if in_flight == 0 || in_flight > MAX_IN_FLIGHT as u64 {
            return Err(f.err(format!(
                "max_in_flight must be between 1 and {MAX_IN_FLIGHT}"
            )));
        }
        let timeout = f.duration_or("timeout", Duration::from_secs(5))?;
        if timeout.is_zero() {
            return Err(f.err("timeout must be above zero"));
        }
        let dedupe_cap = f.u64_or("dedupe_cap", DEFAULT_DEDUPE_CAP as u64)?;
        if dedupe_cap > MAX_DEDUPE_CAP as u64 {
            return Err(f.err(format!("dedupe_cap may be at most {MAX_DEDUPE_CAP}")));
        }
        Ok(Self {
            path,
            auth,
            max_body,
            in_flight: usize::try_from(in_flight).unwrap_or(DEFAULT_IN_FLIGHT),
            timeout,
            dedupe_cap: usize::try_from(dedupe_cap).unwrap_or(DEFAULT_DEDUPE_CAP),
            guard: None,
            rx: None,
        })
    }

    /// Registers the route; a second connect keeps the first registration.
    pub fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        if self.guard.is_some() {
            return Ok(());
        }
        let (tx, rx) = mpsc::channel(self.in_flight);
        let route = Arc::new(Route {
            auth: match &self.auth {
                Some(b) => Auth::Bearer(digest32(b.expose().as_bytes())),
                None => Auth::Public,
            },
            max_body: self.max_body,
            timeout: self.timeout,
            sender: tx,
            dedupe: Mutex::new(Dedupe {
                done: (self.dedupe_cap > 0).then(|| BoundedMap::new(self.dedupe_cap)),
                in_flight: HashSet::new(),
            }),
        });
        let key = (
            ctx.org.org_id.0.clone(),
            ctx.stream_id.clone(),
            self.path.clone(),
        );
        lock(routes()).insert(key.clone(), route.clone());
        self.guard = Some(RouteGuard { key, route });
        self.rx = Some(rx);
        Ok(())
    }

    pub async fn read(&mut self, ctx: &StreamContext) -> Result<(Batch, AckResponder), CoreError> {
        if self.rx.is_none() {
            self.connect(ctx)?;
        }
        let Some(rx) = self.rx.as_mut() else {
            return Err(CoreError::NotConnected);
        };
        let mut shutdown = ctx.shutdown_rx.clone();
        let mut watching = true;
        let got = loop {
            if *shutdown.borrow() {
                return Err(CoreError::EndOfInput);
            }
            if !watching {
                // The stream's stop signal is gone: listen until the channel ends.
                break rx.recv().await;
            }
            tokio::select! {
                got = rx.recv() => break got,
                changed = shutdown.changed() => {
                    if changed.is_err() {
                        watching = false;
                    }
                }
            }
        };
        match got {
            Some(d) => {
                let done = d.done;
                let ack = AckResponder::from_fn(move |res| {
                    let _ = done.send(res);
                });
                Ok((d.batch, ack))
            }
            None => Err(CoreError::EndOfInput),
        }
    }

    /// Unregisters the route: the ingress answers 404 from now on.
    pub fn close(&mut self) {
        self.guard = None;
        self.rx = None;
    }
}

/// The router E4 mounts below its ingress prefix (bodies over 16 MiB never reach a route).
pub fn router() -> Router {
    Router::new()
        .route("/{org}/{stream}", post(handle_root))
        .route("/{org}/{stream}/{*path}", post(handle_path))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

async fn handle_root(
    Path((org, stream)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> IngestAnswer {
    dispatch(&org, &stream, "/", &headers, body).await
}

async fn handle_path(
    Path((org, stream, path)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> IngestAnswer {
    dispatch(
        &org,
        &stream,
        &format!("/{}", path.trim_start_matches('/')),
        &headers,
        body,
    )
    .await
}

fn authorized(auth: &Auth, headers: &HeaderMap) -> bool {
    match auth {
        Auth::Public => true,
        Auth::Bearer(want) => {
            let Some(given) = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| {
                    v.strip_prefix("Bearer ")
                        .or_else(|| v.strip_prefix("bearer "))
                })
            else {
                return false;
            };
            let got = digest32(given.trim().as_bytes());
            bool::from(got[..].ct_eq(&want[..]))
        }
    }
}

fn json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .is_some_and(|m| m == "application/json" || m == "application/ld+json")
}

struct Notification {
    id: String,
    subscription_id: String,
    notified_at: String,
    data: Vec<Value>,
}

/// A Notification (ETSI GS CIM 009 5.3.1): `type` "Notification", `id`, `subscriptionId` and
/// `notifiedAt` strings, `data` an array of entities (objects with a string `id`).
fn parse_notification(body: &[u8]) -> Result<Notification, String> {
    let value = ekiben_bloblang::go_json::decode_with_numbers(body)
        .ok_or_else(|| "the body is not JSON".to_string())?;
    let Value::Object(m) = value else {
        return Err("the body is not a JSON object".to_string());
    };
    let text = |k: &str| match m.get(k) {
        Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
        _ => Err(format!("the Notification has no {k}")),
    };
    if text("type")? != "Notification" {
        return Err("the body's type is not Notification".to_string());
    }
    let id = text("id")?;
    let subscription_id = text("subscriptionId")?;
    let notified_at = text("notifiedAt")?;
    if chrono::DateTime::parse_from_rfc3339(&notified_at).is_err() {
        return Err("notifiedAt is not an RFC 3339 date-time".to_string());
    }
    let Some(Value::Array(items)) = m.get("data") else {
        return Err("the Notification has no data array".to_string());
    };
    let mut data = Vec::with_capacity(items.len());
    for (n, item) in items.iter().enumerate() {
        match item {
            Value::Object(e) if matches!(e.get("id"), Some(Value::String(s)) if !s.is_empty()) => {
                data.push(item.clone())
            }
            _ => return Err(format!("data item {n} is not an entity with an id")),
        }
    }
    Ok(Notification {
        id,
        subscription_id,
        notified_at,
        data,
    })
}

/// Hands one Notification to the input registered for `(org, stream, path)` and answers the
/// broker once the output acked it (or refused it, or did not answer in time).
pub async fn dispatch(
    org: &str,
    stream: &str,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> IngestAnswer {
    let route = {
        let key = (org.to_string(), stream.to_string(), path.to_string());
        lock(routes()).get(&key).cloned()
    };
    let Some(route) = route else {
        return IngestAnswer::new(404, "no Notification route here");
    };
    if !authorized(&route.auth, headers) {
        return IngestAnswer::new(401, "unauthorized");
    }
    if body.len() > route.max_body {
        return IngestAnswer::new(
            413,
            format!("the Notification is larger than {} bytes", route.max_body),
        );
    }
    if !json_content_type(headers) {
        return IngestAnswer::new(
            415,
            "the Notification must be application/json or application/ld+json",
        );
    }
    let n = match parse_notification(&body) {
        Ok(n) => n,
        Err(why) => return IngestAnswer::new(400, why),
    };
    {
        let mut d = lock(&route.dedupe);
        if d.done.as_ref().is_some_and(|m| m.contains(&n.id)) {
            return IngestAnswer::new(204, "");
        }
        if d.in_flight.contains(&n.id) {
            return IngestAnswer::busy("the same Notification is being written");
        }
        if n.data.is_empty() {
            if let Some(m) = d.done.as_mut() {
                m.insert(n.id.clone(), String::new());
            }
            return IngestAnswer::new(204, "");
        }
        d.in_flight.insert(n.id.clone());
    }
    let batch: Batch = n
        .data
        .into_iter()
        .map(|entity| {
            let mut p = Part::from_structured(entity);
            p.meta_set_str("ngsild_subscription_id", n.subscription_id.clone());
            p.meta_set_str("ngsild_notification_id", n.id.clone());
            p.meta_set_str("ngsild_notified_at", n.notified_at.clone());
            p
        })
        .collect();
    let (tx, mut rx) = oneshot::channel();
    if route.sender.try_send(Delivery { batch, done: tx }).is_err() {
        lock(&route.dedupe).in_flight.remove(&n.id);
        return IngestAnswer::busy("the input is full");
    }
    let settle = |route: &Route, id: &str, written: bool| {
        let mut d = lock(&route.dedupe);
        d.in_flight.remove(id);
        if written {
            if let Some(m) = d.done.as_mut() {
                m.insert(id.to_string(), String::new());
            }
        }
    };
    match tokio::time::timeout(route.timeout, &mut rx).await {
        Ok(Ok(None)) => {
            settle(&route, &n.id, true);
            IngestAnswer::new(204, "")
        }
        Ok(Ok(Some(_))) => {
            settle(&route, &n.id, false);
            IngestAnswer::new(502, "Server error")
        }
        Ok(Err(_)) => {
            settle(&route, &n.id, false);
            IngestAnswer::busy("the stream stopped before the Notification was written")
        }
        Err(_) => {
            // The broker retries; a late ack still marks the Notification written so the retry
            // is answered 204 without a second write.
            let late_route = route.clone();
            let id = n.id.clone();
            tokio::spawn(async move {
                let written = matches!(rx.await, Ok(None));
                settle(&late_route, &id, written);
            });
            IngestAnswer::new(408, "Request timed out")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        for ok in ["/", "/notify", "/a/b-c_d.e~f"] {
            assert!(valid_path(ok), "{ok}");
        }
        for bad in [
            "", "x", "/a//b", "/../x", "/a/./b", "/a b", "/a?b", "/é", "/a/",
        ] {
            assert!(!valid_path(bad), "{bad}");
        }
    }

    #[test]
    fn notification_shape() {
        let ok = br#"{"type":"Notification","id":"urn:n:1","subscriptionId":"urn:s:1","notifiedAt":"2026-01-01T00:00:00Z","data":[{"id":"urn:a","type":"T"}]}"#;
        let n = parse_notification(ok).expect("valid");
        assert_eq!(n.data.len(), 1);
        for bad in [
            &br#"{"type":"Other","id":"i","subscriptionId":"s","notifiedAt":"2026-01-01T00:00:00Z","data":[]}"#[..],
            br#"{"type":"Notification","subscriptionId":"s","notifiedAt":"2026-01-01T00:00:00Z","data":[]}"#,
            br#"{"type":"Notification","id":"i","subscriptionId":"s","notifiedAt":"yesterday","data":[]}"#,
            br#"{"type":"Notification","id":"i","subscriptionId":"s","notifiedAt":"2026-01-01T00:00:00Z","data":[1]}"#,
            br#"{"type":"Notification","id":"i","subscriptionId":"s","notifiedAt":"2026-01-01T00:00:00Z"}"#,
            br#"[]"#,
            br#"not json"#,
        ] {
            assert!(parse_notification(bad).is_err());
        }
    }

    #[test]
    fn bearer_compare() {
        let auth = Auth::Bearer(digest32(b"s3cret"));
        let mut h = HeaderMap::new();
        assert!(!authorized(&auth, &h));
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer s3cret"),
        );
        assert!(authorized(&auth, &h));
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer s3cre"),
        );
        assert!(!authorized(&auth, &h));
        h.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic s3cret"),
        );
        assert!(!authorized(&auth, &h));
        assert!(authorized(&Auth::Public, &HeaderMap::new()));
    }
}
