//! `http_server` input (Bento 1.22 `internal/impl/io/input_http_server.go`).
//!
//! Bento's fields, defaults and answers are kept: `path` (`/post`; a trailing `/` makes it a
//! prefix; `{name}` segments become metadata), `allowed_verbs` (`[POST]`), `timeout` (`5s`),
//! `rate_limit`, `sync_response` (`status` `"200"`, `headers` `{Content-Type:
//! application/octet-stream}`, `metadata_headers`), `prefix_metadata_keys`, `cors`. A multipart
//! body is one message per part. The answers are Bento's: 405 "Incorrect method", 429 "Too Many
//! Requests" with `Retry-After`, 400 "Bad request", 408 "Request timed out", 502 with the nack's
//! error, 503 "Server closing". A request is answered only once the output (or `sync_response`)
//! acked its messages.
//!
//! ekiben extensions (documented in the crate README):
//! - In `streams` mode every `http_server` input is served by the shared ingest listener
//!   (`--ingest-addr`) at `/{org}/{stream}{path}`; `address`, `cert_file`, `key_file` and
//!   `ws_path` are refused there, and `auth` is required.
//! - `auth`: `bearer: <token>` (`Authorization: Bearer <token>`), `hmac: {header, secret,
//!   algorithm: sha256}` (hex HMAC of the body, optionally `sha256=`-prefixed) or `public: true`.
//!   A missing and a wrong credential get the same 401; the credential never becomes metadata.
//! - `max_body_bytes` (default 1 MiB, at most 16 MiB; larger bodies get 413) and `max_in_flight`
//!   (default 64 requests waiting for their answer; more get 503 with `Retry-After: 1`).
//! - Websocket ingestion (`ws_path`) and TLS termination are not built in.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use ekiben_bloblang::FieldExpression;
use ekiben_core::config::{extract_metadata_filter, parse_duration_str, MetadataFilter};
use ekiben_core::{
    AckResponder, Batch, ComponentConfig, CoreError, Input, OrgId, Part, PartsView, Registry,
    ResourceManager, SecretString, StreamContext, SyncResponseStore,
};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, oneshot, watch, Semaphore};

/// Default `max_body_bytes`.
pub const DEFAULT_MAX_BODY_BYTES: usize = 1024 * 1024;
/// Largest `max_body_bytes` accepted.
pub const MAX_BODY_BYTES_LIMIT: usize = 16 * 1024 * 1024;
/// Default `max_in_flight`.
pub const DEFAULT_MAX_IN_FLIGHT: usize = 64;
/// Largest sync response body (all parts together); a larger one answers 502.
pub const MAX_SYNC_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

pub fn register(registry: &Registry) {
    registry.register_input("http_server", |conf, ctx| {
        Ok(Box::new(HttpServerInput::from_config(conf, ctx)?))
    });
}

/// How a request proves it may post (ekiben extension `auth`).
#[derive(Clone)]
pub enum IngestAuth {
    /// `public: true`: anyone may post.
    Public,
    /// `bearer: <token>`.
    Bearer(SecretString),
    /// `hmac: {header, secret, algorithm: sha256}`.
    Hmac {
        header: String,
        secret: SecretString,
    },
}

impl std::fmt::Debug for IngestAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestAuth::Public => f.write_str("Public"),
            IngestAuth::Bearer(_) => f.write_str("Bearer(REDACTED)"),
            IngestAuth::Hmac { header, .. } => f
                .debug_struct("Hmac")
                .field("header", header)
                .field("secret", &"REDACTED")
                .finish(),
        }
    }
}

/// A `path` with `{name}` segments; a trailing `/` makes it a prefix (gorilla/mux `PathPrefix`,
/// as Bento's `api.GetMuxRoute`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathPattern {
    raw: String,
    segments: Vec<Segment>,
    prefix: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Var(String),
}

fn split_segments(path: &str) -> Vec<&str> {
    path.trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect()
}

impl PathPattern {
    pub fn parse(path: &str) -> Result<Self, CoreError> {
        if !path.starts_with('/') {
            return Err(CoreError::config(format!(
                "http_server.path '{path}' must start with '/'"
            )));
        }
        let prefix = path.len() > 1 && path.ends_with('/');
        let mut segments = Vec::new();
        for seg in split_segments(path) {
            if let Some(inner) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                if inner.contains(':') {
                    return Err(CoreError::config(format!(
                        "http_server.path '{path}': path variable patterns ({{name:regex}}) are not supported by ekiben"
                    )));
                }
                if inner.is_empty() || inner.contains(['{', '}']) {
                    return Err(CoreError::config(format!(
                        "http_server.path '{path}': bad path variable '{seg}'"
                    )));
                }
                segments.push(Segment::Var(inner.to_string()));
            } else if seg.contains(['{', '}']) {
                return Err(CoreError::config(format!(
                    "http_server.path '{path}': bad path segment '{seg}'"
                )));
            } else {
                segments.push(Segment::Literal(seg.to_string()));
            }
        }
        Ok(Self {
            raw: path.to_string(),
            segments,
            prefix,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// The path variables when `path` (decoded) matches.
    pub fn matches(&self, path: &str) -> Option<Vec<(String, String)>> {
        let got = split_segments(path);
        if self.prefix {
            let fits = got.len() > self.segments.len()
                || (got.len() == self.segments.len() && path.ends_with('/'));
            if !fits {
                return None;
            }
        } else {
            if got.len() != self.segments.len() {
                return None;
            }
            if path.len() > 1 && path.ends_with('/') {
                return None;
            }
            if self.segments.is_empty() && path != "/" {
                return None;
            }
        }
        let mut vars = Vec::new();
        for (want, have) in self.segments.iter().zip(got.iter()) {
            match want {
                Segment::Literal(l) if l == have => {}
                Segment::Literal(_) => return None,
                Segment::Var(name) => vars.push((name.clone(), (*have).to_string())),
            }
        }
        Some(vars)
    }
}

/// The parsed config of one `http_server` input.
pub struct HttpServerConf {
    pub address: String,
    pub path: PathPattern,
    pub allowed_verbs: HashSet<String>,
    pub timeout: Duration,
    pub rate_limit: String,
    pub response_status: FieldExpression,
    pub response_headers: Vec<(String, FieldExpression)>,
    /// The raw `sync_response.headers` keys, for Bento's case-sensitive `content-type` lookup.
    pub response_header_keys: Vec<String>,
    pub metadata_headers: MetadataFilter,
    pub prefix_metadata_keys: bool,
    pub cors_enabled: bool,
    pub cors_origins: Vec<String>,
    /// `None` when the config has no `auth` (allowed outside streams mode only).
    pub auth: Option<IngestAuth>,
    pub max_body_bytes: usize,
    pub max_in_flight: usize,
}

impl std::fmt::Debug for HttpServerConf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpServerConf")
            .field("address", &self.address)
            .field("path", &self.path.raw)
            .field("allowed_verbs", &self.allowed_verbs)
            .field("timeout", &self.timeout)
            .field("rate_limit", &self.rate_limit)
            .field("prefix_metadata_keys", &self.prefix_metadata_keys)
            .field("auth", &self.auth)
            .field("max_body_bytes", &self.max_body_bytes)
            .field("max_in_flight", &self.max_in_flight)
            .finish()
    }
}

fn str_field<'a>(map: &'a serde_json::Map<String, JsonValue>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(JsonValue::as_str)
}

fn usize_field(
    map: &serde_json::Map<String, JsonValue>,
    key: &str,
    default: usize,
) -> Result<usize, CoreError> {
    match map.get(key) {
        None | Some(JsonValue::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| CoreError::config(format!("http_server.{key}: expected a number"))),
    }
}

fn parse_auth(val: &JsonValue) -> Result<IngestAuth, CoreError> {
    let Some(map) = val.as_object() else {
        return Err(CoreError::config(
            "http_server.auth: expected one of bearer, hmac or public: true",
        ));
    };
    let mut chosen: Vec<IngestAuth> = Vec::new();
    if let Some(b) = map.get("bearer") {
        let token = b
            .as_str()
            .ok_or_else(|| CoreError::config("http_server.auth.bearer: expected a string"))?;
        if token.is_empty() {
            return Err(CoreError::config(
                "http_server.auth.bearer must not be empty",
            ));
        }
        chosen.push(IngestAuth::Bearer(SecretString::new(token)));
    }
    if let Some(h) = map.get("hmac") {
        let hm = h
            .as_object()
            .ok_or_else(|| CoreError::config("http_server.auth.hmac: expected an object"))?;
        let header = str_field(hm, "header").unwrap_or("X-Signature").to_string();
        if HeaderName::from_bytes(header.as_bytes()).is_err() {
            return Err(CoreError::config(
                "http_server.auth.hmac.header is not a valid header name",
            ));
        }
        let secret = str_field(hm, "secret").unwrap_or_default();
        if secret.is_empty() {
            return Err(CoreError::config(
                "http_server.auth.hmac.secret must not be empty",
            ));
        }
        let algorithm = str_field(hm, "algorithm").unwrap_or("sha256");
        if algorithm != "sha256" {
            return Err(CoreError::config(format!(
                "http_server.auth.hmac.algorithm '{algorithm}' is not supported (sha256)"
            )));
        }
        chosen.push(IngestAuth::Hmac {
            header,
            secret: SecretString::new(secret),
        });
    }
    match map.get("public") {
        None | Some(JsonValue::Bool(false)) => {}
        Some(JsonValue::Bool(true)) => chosen.push(IngestAuth::Public),
        Some(_) => {
            return Err(CoreError::config(
                "http_server.auth.public: expected a boolean",
            ))
        }
    }
    for key in map.keys() {
        if !matches!(key.as_str(), "bearer" | "hmac" | "public") {
            return Err(CoreError::config(format!(
                "http_server.auth: field {key} is not recognised"
            )));
        }
    }
    match chosen.len() {
        1 => Ok(chosen.remove(0)),
        0 => Err(CoreError::config(
            "http_server.auth: set one of bearer, hmac or public: true",
        )),
        _ => Err(CoreError::config(
            "http_server.auth: set only one of bearer, hmac or public",
        )),
    }
}

impl HttpServerConf {
    pub fn from_json(
        val: &JsonValue,
        env: &ekiben_bloblang::Environment,
    ) -> Result<Self, CoreError> {
        let empty = serde_json::Map::new();
        let map = val.as_object().unwrap_or(&empty);
        let address = str_field(map, "address").unwrap_or_default().to_string();
        for key in ["cert_file", "key_file"] {
            if !str_field(map, key).unwrap_or_default().is_empty() {
                return Err(CoreError::config(format!(
                    "http_server.{key}: ekiben does not terminate TLS in http_server; terminate it in front of ekiben"
                )));
            }
        }
        if !str_field(map, "ws_path").unwrap_or_default().is_empty() {
            return Err(CoreError::config(
                "http_server.ws_path: websocket ingestion is not supported by ekiben (use the websocket input)",
            ));
        }
        let path = PathPattern::parse(str_field(map, "path").unwrap_or("/post"))?;
        let allowed_verbs: HashSet<String> = match map.get("allowed_verbs") {
            None | Some(JsonValue::Null) => HashSet::from(["POST".to_string()]),
            Some(JsonValue::Array(a)) => a
                .iter()
                .filter_map(JsonValue::as_str)
                .map(str::to_string)
                .collect(),
            Some(_) => {
                return Err(CoreError::config(
                    "http_server.allowed_verbs: expected a list of strings",
                ))
            }
        };
        let timeout = parse_duration_str(str_field(map, "timeout").unwrap_or("5s"))?;
        let rate_limit = str_field(map, "rate_limit").unwrap_or_default().to_string();

        let resp = map.get("sync_response").and_then(JsonValue::as_object);
        let status_src = resp.and_then(|r| str_field(r, "status")).unwrap_or("200");
        let response_status = env
            .compile_field(status_src)
            .map_err(|e| CoreError::config(format!("http_server.sync_response.status: {e}")))?;
        let mut response_headers = Vec::new();
        let mut response_header_keys = Vec::new();
        match resp.and_then(|r| r.get("headers")) {
            None | Some(JsonValue::Null) => {
                response_headers.push((
                    "Content-Type".to_string(),
                    env.compile_field("application/octet-stream").map_err(|e| {
                        CoreError::config(format!("http_server.sync_response.headers: {e}"))
                    })?,
                ));
                response_header_keys.push("Content-Type".to_string());
            }
            Some(JsonValue::Object(h)) => {
                for (k, v) in h {
                    let src = match v {
                        JsonValue::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    let expr = env.compile_field(&src).map_err(|e| {
                        CoreError::config(format!("http_server.sync_response.headers.{k}: {e}"))
                    })?;
                    response_headers.push((k.clone(), expr));
                    response_header_keys.push(k.clone());
                }
            }
            Some(_) => {
                return Err(CoreError::config(
                    "http_server.sync_response.headers: expected a map",
                ))
            }
        }
        let metadata_headers = resp
            .and_then(|r| r.get("metadata_headers"))
            .map(extract_metadata_filter)
            .unwrap_or_default();
        let prefix_metadata_keys = map
            .get("prefix_metadata_keys")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let cors = map.get("cors").and_then(JsonValue::as_object);
        let cors_enabled = cors
            .and_then(|c| c.get("enabled"))
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let cors_origins = cors
            .and_then(|c| c.get("allowed_origins"))
            .and_then(JsonValue::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(JsonValue::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let auth = match map.get("auth") {
            None | Some(JsonValue::Null) => None,
            Some(v) => Some(parse_auth(v)?),
        };
        let max_body_bytes = usize_field(map, "max_body_bytes", DEFAULT_MAX_BODY_BYTES)?;
        if max_body_bytes == 0 || max_body_bytes > MAX_BODY_BYTES_LIMIT {
            return Err(CoreError::config(format!(
                "http_server.max_body_bytes must be between 1 and {MAX_BODY_BYTES_LIMIT}"
            )));
        }
        let max_in_flight = usize_field(map, "max_in_flight", DEFAULT_MAX_IN_FLIGHT)?;
        if max_in_flight == 0 || max_in_flight > 4096 {
            return Err(CoreError::config(
                "http_server.max_in_flight must be between 1 and 4096",
            ));
        }
        Ok(Self {
            address,
            path,
            allowed_verbs,
            timeout,
            rate_limit,
            response_status,
            response_headers,
            response_header_keys,
            metadata_headers,
            prefix_metadata_keys,
            cors_enabled,
            cors_origins,
            auth,
            max_body_bytes,
            max_in_flight,
        })
    }
}

/// Lint errors of a stream config for `streams` mode: every `http_server` (input, broker child
/// or resource) is served by the ingest listener, so its own listener and TLS fields are refused
/// and `auth` is required.
pub fn lint_streams_mode(raw: &JsonValue) -> Vec<String> {
    let mut errors = Vec::new();
    walk_http_servers(raw, &mut errors);
    errors
}

fn walk_http_servers(val: &JsonValue, errors: &mut Vec<String>) {
    match val {
        JsonValue::Object(map) => {
            for (k, v) in map {
                if k == "http_server" {
                    if let Some(hs) = v.as_object() {
                        lint_one(hs, errors);
                    }
                }
                walk_http_servers(v, errors);
            }
        }
        JsonValue::Array(a) => {
            for v in a {
                walk_http_servers(v, errors);
            }
        }
        _ => {}
    }
}

fn lint_one(hs: &serde_json::Map<String, JsonValue>, errors: &mut Vec<String>) {
    for key in ["address", "cert_file", "key_file"] {
        if hs
            .get(key)
            .and_then(JsonValue::as_str)
            .is_some_and(|s| !s.is_empty())
        {
            errors.push(format!(
                "http_server.{key} is not allowed in streams mode: http_server inputs are served by the ingest listener (--ingest-addr) at /{{org}}/{{stream}}{{path}}"
            ));
        }
    }
    if hs.contains_key("ws_path") {
        errors.push(
            "http_server.ws_path is not allowed in streams mode: websocket ingestion is not supported"
                .to_string(),
        );
    }
    if !hs.contains_key("auth") {
        errors.push(
            "http_server needs auth in streams mode: bearer, hmac or public: true".to_string(),
        );
    }
}

static STREAMS_MODE: AtomicBool = AtomicBool::new(false);
static INGEST_ACTIVE: AtomicBool = AtomicBool::new(false);
static NEXT_ENDPOINT_ID: AtomicU64 = AtomicU64::new(1);

/// Marks the process as a `streams` server: an `http_server` may then only use the ingest
/// listener, and only with `auth` (also enforced when a chilled config skipped the lint).
pub fn set_streams_mode() {
    STREAMS_MODE.store(true, Ordering::SeqCst);
}

pub fn is_streams_mode() -> bool {
    STREAMS_MODE.load(Ordering::SeqCst)
}

pub(crate) fn mark_ingest_active() {
    INGEST_ACTIVE.store(true, Ordering::SeqCst);
}

pub fn is_ingest_active() -> bool {
    INGEST_ACTIVE.load(Ordering::SeqCst)
}

type Pending = (Batch, AckResponder);
type RouteKey = (String, String);

/// One served `http_server` input.
pub(crate) struct Endpoint {
    id: u64,
    conf: Arc<HttpServerConf>,
    tx: mpsc::Sender<Pending>,
    closing: AtomicBool,
    in_flight: Arc<Semaphore>,
    org_id: OrgId,
    resources: ResourceManager,
}

/// The ingest route table: `(org, stream)` to the stream's `http_server` endpoints.
fn routes() -> &'static RwLock<HashMap<RouteKey, Vec<Arc<Endpoint>>>> {
    static ROUTES: OnceLock<RwLock<HashMap<RouteKey, Vec<Arc<Endpoint>>>>> = OnceLock::new();
    ROUTES.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Path variables captured by a route, as `(name, value)` pairs.
pub(crate) type PathVars = Vec<(String, String)>;

/// The endpoint of `org`'s stream `stream` serving `path`, with the path variables.
pub(crate) fn lookup(org: &str, stream: &str, path: &str) -> Option<(Arc<Endpoint>, PathVars)> {
    let table = routes().read().unwrap_or_else(PoisonError::into_inner);
    let eps = table.get(&(org.to_string(), stream.to_string()))?;
    eps.iter()
        .find_map(|ep| ep.conf.path.matches(path).map(|vars| (ep.clone(), vars)))
}

/// Removes an endpoint from the route table when its input closes or is dropped.
struct RouteGuard {
    key: RouteKey,
    endpoint: Arc<Endpoint>,
}

impl Drop for RouteGuard {
    fn drop(&mut self) {
        self.endpoint.closing.store(true, Ordering::SeqCst);
        let mut table = routes().write().unwrap_or_else(PoisonError::into_inner);
        if let Some(eps) = table.get_mut(&self.key) {
            eps.retain(|e| e.id != self.endpoint.id);
            if eps.is_empty() {
                table.remove(&self.key);
            }
        }
    }
}

/// A listener of its own (`address`, outside streams mode).
struct OwnServer {
    endpoint: Arc<Endpoint>,
    stop: Option<oneshot::Sender<()>>,
}

impl Drop for OwnServer {
    fn drop(&mut self) {
        self.endpoint.closing.store(true, Ordering::SeqCst);
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// What keeps the input's endpoint served; the guards are held only for their `Drop`.
enum Serving {
    Ingest { _guard: RouteGuard },
    Own { _server: OwnServer },
}

/// The `http_server` input.
pub struct HttpServerInput {
    conf: Arc<HttpServerConf>,
    label: String,
    rx: Option<mpsc::Receiver<Pending>>,
    serving: Option<Serving>,
    shutdown: Option<watch::Receiver<bool>>,
}

impl HttpServerInput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let parsed = HttpServerConf::from_json(&conf.config, &ctx.blobl_env)?;
        if !parsed.rate_limit.is_empty()
            && !ctx
                .resources
                .has_rate_limit(ctx.org_id(), &parsed.rate_limit)
        {
            return Err(CoreError::config(format!(
                "rate limit resource '{}' was not found",
                parsed.rate_limit
            )));
        }
        Ok(Self {
            conf: Arc::new(parsed),
            label: conf.label.clone(),
            rx: None,
            serving: None,
            shutdown: None,
        })
    }

    /// The address of the input's own listener, once connected with `address`.
    pub fn conf(&self) -> &HttpServerConf {
        &self.conf
    }
}

#[async_trait]
impl Input for HttpServerInput {
    fn input_type(&self) -> &'static str {
        "http_server"
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        if self.serving.is_some() {
            return Ok(());
        }
        let streams_mode = is_streams_mode();
        if streams_mode {
            if !self.conf.address.is_empty() {
                return Err(CoreError::config(
                    "http_server.address is not allowed in streams mode (use the ingest listener)",
                ));
            }
            if self.conf.auth.is_none() {
                return Err(CoreError::config(
                    "http_server needs auth in streams mode: bearer, hmac or public: true",
                ));
            }
        }
        let (tx, rx) = mpsc::channel::<Pending>(1);
        let endpoint = Arc::new(Endpoint {
            id: NEXT_ENDPOINT_ID.fetch_add(1, Ordering::Relaxed),
            conf: self.conf.clone(),
            tx,
            closing: AtomicBool::new(false),
            in_flight: Arc::new(Semaphore::new(self.conf.max_in_flight)),
            org_id: ctx.org_id().clone(),
            resources: ctx.resources.clone(),
        });

        let serving = if !self.conf.address.is_empty() {
            let listener = tokio::net::TcpListener::bind(&self.conf.address)
                .await
                .map_err(|e| {
                    CoreError::other(format!(
                        "http_server: failed to listen on {}: {e}",
                        self.conf.address
                    ))
                })?;
            let (stop_tx, stop_rx) = oneshot::channel::<()>();
            let ep = endpoint.clone();
            let app = axum::Router::new().fallback(move |req: Request| {
                let ep = ep.clone();
                async move {
                    let remote = remote_addr(&req);
                    let path = percent_decode(req.uri().path());
                    match ep.conf.path.matches(&path) {
                        Some(vars) => serve_request(ep, req, path, vars, remote).await,
                        None => text_response(StatusCode::NOT_FOUND, "404 page not found"),
                    }
                }
            });
            tokio::spawn(async move {
                let _ = axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<SocketAddr>(),
                )
                .with_graceful_shutdown(async move {
                    let _ = stop_rx.await;
                })
                .await;
            });
            Serving::Own {
                _server: OwnServer {
                    endpoint,
                    stop: Some(stop_tx),
                },
            }
        } else if is_ingest_active() {
            let key = (ctx.org_id().0.clone(), ctx.stream_id.clone());
            {
                let mut table = routes().write().unwrap_or_else(PoisonError::into_inner);
                let eps = table.entry(key.clone()).or_default();
                if eps
                    .iter()
                    .any(|e| e.conf.path.as_str() == self.conf.path.as_str())
                {
                    return Err(CoreError::config(format!(
                        "http_server: path '{}' is already served for stream '{}'",
                        self.conf.path.as_str(),
                        ctx.stream_id
                    )));
                }
                eps.push(endpoint.clone());
            }
            Serving::Ingest {
                _guard: RouteGuard { key, endpoint },
            }
        } else if streams_mode {
            return Err(CoreError::config(
                "http_server: this server has no ingest listener; start it with --ingest-addr",
            ));
        } else {
            return Err(CoreError::config(
                "http_server: set address (for example 0.0.0.0:4195) to serve this input",
            ));
        };
        self.serving = Some(serving);
        self.rx = Some(rx);
        self.shutdown = Some(ctx.shutdown_rx.clone());
        Ok(())
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        let Some(rx) = self.rx.as_mut() else {
            return Err(CoreError::NotConnected);
        };
        let shutdown = self
            .shutdown
            .clone()
            .unwrap_or_else(|| ctx.shutdown_rx.clone());
        if *shutdown.borrow() {
            return Err(CoreError::EndOfInput);
        }
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(pending) => Ok(pending),
                None => Err(CoreError::EndOfInput),
            },
            _ = shutdown_signal(shutdown) => Err(CoreError::EndOfInput),
        }
    }

    fn is_connected(&self) -> bool {
        self.serving.is_some()
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        // Dropping the route guard or server stops new requests; dropping the receiver answers
        // the waiting ones with 503 "Server closing".
        self.serving = None;
        self.rx = None;
        Ok(())
    }
}

/// Resolves once the stream is told to stop; never when the stop sender is gone without a stop.
pub async fn shutdown_signal(mut rx: watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// The peer address axum records when served with `into_make_service_with_connect_info`.
pub(crate) fn remote_addr(req: &Request) -> Option<SocketAddr> {
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(a)| *a)
}

/// `http.Error`: the text, a newline, `text/plain; charset=utf-8` and `nosniff`.
pub(crate) fn text_response(status: StatusCode, msg: &str) -> Response {
    let mut resp = Response::new(Body::from(format!("{msg}\n")));
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    h.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    h.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    resp
}

fn status_only(status: StatusCode) -> Response {
    let mut resp = Response::new(Body::empty());
    *resp.status_mut() = status;
    resp
}

fn unauthorized() -> Response {
    let mut resp = text_response(StatusCode::UNAUTHORIZED, "Unauthorized");
    resp.headers_mut().insert(
        axum::http::header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer"),
    );
    resp
}

fn with_retry_after(mut resp: Response, secs: u64) -> Response {
    if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
        resp.headers_mut()
            .insert(axum::http::header::RETRY_AFTER, v);
    }
    resp
}

/// Decodes `%XX` escapes of a URL path (as Go's `URL.Path`); invalid escapes stay as written.
pub(crate) fn percent_decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes.get(i).copied().unwrap_or_default();
        if b == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok());
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Go's `textproto.CanonicalMIMEHeaderKey` (`x-custom-header` is `X-Custom-Header`).
pub fn canonical_header_key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = true;
    for c in name.chars() {
        if upper {
            out.extend(c.to_uppercase());
        } else {
            out.extend(c.to_lowercase());
        }
        upper = c == '-';
    }
    out
}

fn bearer_ok(headers: &HeaderMap, token: &SecretString) -> bool {
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.strip_prefix("Bearer ")
                .or_else(|| s.strip_prefix("bearer "))
        });
    let Some(provided) = provided else {
        return false;
    };
    // Digests of equal length, compared in constant time: neither the token nor its length leak.
    let a = Sha256::digest(provided.as_bytes());
    let b = Sha256::digest(token.expose().as_bytes());
    a.as_slice().ct_eq(b.as_slice()).into()
}

fn hmac_ok(headers: &HeaderMap, header: &str, secret: &SecretString, body: &[u8]) -> bool {
    let Some(sig) = headers.get(header).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let hex_sig = sig.trim();
    let hex_sig = hex_sig.strip_prefix("sha256=").unwrap_or(hex_sig);
    let Ok(sig_bytes) = hex::decode(hex_sig) else {
        return false;
    };
    let Ok(mut mac) = <Hmac<Sha256>>::new_from_slice(secret.expose().as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&sig_bytes).is_ok()
}

/// The hex HMAC-SHA256 signature of `body` that `auth.hmac` accepts.
pub fn hmac_signature(secret: &str, body: &[u8]) -> String {
    match <Hmac<Sha256>>::new_from_slice(secret.as_bytes()) {
        Ok(mut mac) => {
            mac.update(body);
            hex::encode(mac.finalize().into_bytes())
        }
        Err(_) => String::new(),
    }
}

/// `mime.ParseMediaType`, enough for the type and its `boundary`.
fn parse_media_type(ct: &str) -> Result<(String, HashMap<String, String>), ()> {
    let mut pieces = ct.split(';');
    let media = pieces
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if media.is_empty() || !media.contains('/') || media.contains(char::is_whitespace) {
        return Err(());
    }
    let mut params = HashMap::new();
    for p in pieces {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        let (k, v) = p.split_once('=').ok_or(())?;
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(v);
        params.insert(k.trim().to_ascii_lowercase(), v.to_string());
    }
    Ok((media, params))
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > hay.len() {
        return None;
    }
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// The bodies of a multipart body (RFC 2046), one per part.
fn parse_multipart(body: &[u8], boundary: &str) -> Result<Vec<Bytes>, ()> {
    if boundary.is_empty() || boundary.len() > 70 {
        return Err(());
    }
    let delim = format!("--{boundary}");
    let mut pos = find(body, delim.as_bytes(), 0).ok_or(())? + delim.len();
    let mut parts = Vec::new();
    loop {
        let rest = body.get(pos..).ok_or(())?;
        if rest.starts_with(b"--") {
            return Ok(parts);
        }
        // Skip transport padding and the line break after the delimiter.
        let line_end = find(body, b"\r\n", pos).ok_or(())?;
        let headers_start = line_end + 2;
        let body_start = if body.get(headers_start..headers_start + 2) == Some(b"\r\n") {
            headers_start + 2
        } else {
            find(body, b"\r\n\r\n", headers_start).ok_or(())? + 4
        };
        let next_delim = format!("\r\n{delim}");
        let end = find(body, next_delim.as_bytes(), body_start).ok_or(())?;
        let part = body.get(body_start..end).ok_or(())?;
        parts.push(Bytes::copy_from_slice(part));
        pos = end + next_delim.len();
    }
}

struct RequestMeta {
    user_agent: String,
    path: String,
    verb: String,
    remote_ip: Option<String>,
    headers: BTreeMap<String, String>,
    query: Vec<(String, String)>,
    params: Vec<(String, String)>,
    cookies: Vec<(String, String)>,
}

fn request_meta(
    conf: &HttpServerConf,
    req_headers: &HeaderMap,
    method: &Method,
    query: Option<&str>,
    path: String,
    params: Vec<(String, String)>,
    remote: Option<SocketAddr>,
) -> RequestMeta {
    let hidden: Option<String> = match &conf.auth {
        Some(IngestAuth::Bearer(_)) => Some("authorization".to_string()),
        Some(IngestAuth::Hmac { header, .. }) => Some(header.to_ascii_lowercase()),
        _ => None,
    };
    let mut headers = BTreeMap::new();
    for name in req_headers.keys() {
        let lower = name.as_str();
        if lower == "host" || hidden.as_deref() == Some(lower) {
            continue;
        }
        if let Some(v) = req_headers.get(name).and_then(|v| v.to_str().ok()) {
            headers.insert(canonical_header_key(lower), v.to_string());
        }
    }
    let mut seen = HashSet::new();
    let mut q = Vec::new();
    for (k, v) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if seen.insert(k.to_string()) {
            q.push((k.into_owned(), v.into_owned()));
        }
    }
    let mut cookies = Vec::new();
    for v in req_headers.get_all(axum::http::header::COOKIE) {
        if let Ok(s) = v.to_str() {
            for pair in s.split(';') {
                if let Some((k, val)) = pair.trim().split_once('=') {
                    if !k.is_empty() {
                        let val = val.trim_matches('"');
                        cookies.push((k.to_string(), val.to_string()));
                    }
                }
            }
        }
    }
    RequestMeta {
        user_agent: req_headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string(),
        path,
        verb: method.as_str().to_string(),
        remote_ip: remote.map(|a| a.ip().to_string()),
        headers,
        query: q,
        params,
        cookies,
    }
}

fn apply_meta(part: &mut Part, meta: &RequestMeta, prefix: bool) {
    part.meta_set_str("http_server_user_agent", meta.user_agent.clone());
    part.meta_set_str("http_server_request_path", meta.path.clone());
    part.meta_set_str("http_server_verb", meta.verb.clone());
    if let Some(ip) = &meta.remote_ip {
        part.meta_set_str("http_server_remote_ip", ip.clone());
    }
    let key = |p: &str, k: &str| {
        if prefix {
            format!("{p}{k}")
        } else {
            k.to_string()
        }
    };
    for (k, v) in &meta.headers {
        part.meta_set_str(key("header_", k), v.clone());
    }
    for (k, v) in &meta.query {
        part.meta_set_str(key("query_", k), v.clone());
    }
    for (k, v) in &meta.params {
        part.meta_set_str(key("path_", k), v.clone());
    }
    for (k, v) in &meta.cookies {
        part.meta_set_str(key("cookie_", k), v.clone());
    }
}

fn extract_batch(
    headers: &HeaderMap,
    body: Bytes,
    meta: &RequestMeta,
    prefix: bool,
) -> Result<Batch, ()> {
    let ct = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .unwrap_or("application/octet-stream");
    let (media, params) = parse_media_type(ct)?;
    let payloads = if media.starts_with("multipart/") {
        let boundary = params.get("boundary").ok_or(())?;
        parse_multipart(&body, boundary)?
    } else {
        vec![body]
    };
    Ok(payloads
        .into_iter()
        .map(|b| {
            let mut p = Part::new(b);
            apply_meta(&mut p, meta, prefix);
            p
        })
        .collect())
}

fn cors_headers(conf: &HttpServerConf, headers: &HeaderMap) -> Option<HeaderValue> {
    if !conf.cors_enabled {
        return None;
    }
    let origin = headers.get(axum::http::header::ORIGIN)?.to_str().ok()?;
    if conf.cors_origins.iter().any(|o| o == "*" || o == origin) {
        HeaderValue::from_str(origin).ok()
    } else {
        None
    }
}

/// Serves one request for `ep` (`path` decoded, relative to the stream for the ingest listener).
pub(crate) async fn serve_request(
    ep: Arc<Endpoint>,
    req: Request,
    path: String,
    params: Vec<(String, String)>,
    remote: Option<SocketAddr>,
) -> Response {
    let conf = ep.conf.clone();
    let allow_origin = cors_headers(&conf, req.headers());
    let mut resp = handle(ep, req, path, params, remote).await;
    if let Some(origin) = allow_origin {
        resp.headers_mut()
            .insert(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    }
    resp
}

async fn handle(
    ep: Arc<Endpoint>,
    req: Request,
    path: String,
    params: Vec<(String, String)>,
    remote: Option<SocketAddr>,
) -> Response {
    let conf = ep.conf.clone();
    if ep.closing.load(Ordering::SeqCst) {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "Server closing");
    }
    let (parts, body) = req.into_parts();

    if conf.cors_enabled && parts.method == Method::OPTIONS {
        let mut resp = status_only(StatusCode::OK);
        let mut verbs: Vec<&str> = conf.allowed_verbs.iter().map(String::as_str).collect();
        verbs.sort_unstable();
        if let Ok(v) = HeaderValue::from_str(&verbs.join(", ")) {
            resp.headers_mut()
                .insert(axum::http::header::ACCESS_CONTROL_ALLOW_METHODS, v);
        }
        if let Some(h) = parts
            .headers
            .get(axum::http::header::ACCESS_CONTROL_REQUEST_HEADERS)
        {
            resp.headers_mut()
                .insert(axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS, h.clone());
        }
        return resp;
    }

    if !conf.allowed_verbs.contains(parts.method.as_str()) {
        return text_response(StatusCode::METHOD_NOT_ALLOWED, "Incorrect method");
    }

    if let Some(IngestAuth::Bearer(token)) = &conf.auth {
        if !bearer_ok(&parts.headers, token) {
            return unauthorized();
        }
    }

    let Ok(_permit) = ep.in_flight.clone().try_acquire_owned() else {
        return with_retry_after(
            text_response(StatusCode::SERVICE_UNAVAILABLE, "Server busy"),
            1,
        );
    };

    let declared = parts
        .headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    if declared.is_some_and(|n| n > conf.max_body_bytes as u64) {
        return text_response(StatusCode::PAYLOAD_TOO_LARGE, "Request body too large");
    }
    let body = match axum::body::to_bytes(body, conf.max_body_bytes).await {
        Ok(b) => b,
        Err(_) if declared.is_some() => {
            return text_response(StatusCode::BAD_REQUEST, "Bad request")
        }
        Err(_) => return text_response(StatusCode::PAYLOAD_TOO_LARGE, "Request body too large"),
    };

    if let Some(IngestAuth::Hmac { header, secret }) = &conf.auth {
        if !hmac_ok(&parts.headers, header, secret, &body) {
            return unauthorized();
        }
    }

    if !conf.rate_limit.is_empty() {
        match ep.resources.get_rate_limit(&ep.org_id, &conf.rate_limit) {
            Err(e) => {
                tracing::warn!("http_server: failed to access rate limit: {e}");
                return text_response(StatusCode::BAD_GATEWAY, "Server error");
            }
            Ok(rl) => match rl.access(&[]).await {
                Err(e) => {
                    tracing::warn!("http_server: failed to access rate limit: {e}");
                    return text_response(StatusCode::BAD_GATEWAY, "Server error");
                }
                Ok(wait) if !wait.is_zero() => {
                    return with_retry_after(
                        text_response(StatusCode::TOO_MANY_REQUESTS, "Too Many Requests"),
                        wait.as_secs(),
                    );
                }
                Ok(_) => {}
            },
        }
    }

    let meta = request_meta(
        &conf,
        &parts.headers,
        &parts.method,
        parts.uri.query(),
        path,
        params,
        remote,
    );
    let mut batch = match extract_batch(&parts.headers, body, &meta, conf.prefix_metadata_keys) {
        Ok(b) => b,
        Err(()) => return text_response(StatusCode::BAD_REQUEST, "Bad request"),
    };

    let store = SyncResponseStore::new();
    for p in &mut batch {
        p.with_sync_response_store(store.clone());
    }
    let (ack, res_rx) = AckResponder::channel();
    match tokio::time::timeout(conf.timeout, ep.tx.send((batch, ack))).await {
        Err(_) => return text_response(StatusCode::REQUEST_TIMEOUT, "Request timed out"),
        Ok(Err(_)) => return text_response(StatusCode::SERVICE_UNAVAILABLE, "Server closing"),
        Ok(Ok(())) => {}
    }
    match tokio::time::timeout(conf.timeout, res_rx).await {
        Err(_) => return text_response(StatusCode::REQUEST_TIMEOUT, "Request timed out"),
        Ok(Err(_)) => return text_response(StatusCode::SERVICE_UNAVAILABLE, "Server closing"),
        Ok(Ok(Some(err))) => return text_response(StatusCode::BAD_GATEWAY, &err.message),
        Ok(Ok(None)) => {}
    }
    build_sync_response(&conf, store.read())
}

/// Go's `http.DetectContentType`, for the types a sync response produces.
pub fn detect_content_type(payload: &[u8]) -> &'static str {
    let head = payload.get(..payload.len().min(512)).unwrap_or_default();
    let trimmed: &[u8] = {
        let start = head
            .iter()
            .position(|b| !matches!(b, b'\t' | b'\n' | b'\x0c' | b'\r' | b' '))
            .unwrap_or(head.len());
        head.get(start..).unwrap_or_default()
    };
    let lower: Vec<u8> = trimmed
        .iter()
        .take(14)
        .map(u8::to_ascii_lowercase)
        .collect();
    if lower.starts_with(b"<!doctype html") || lower.starts_with(b"<html") {
        return "text/html; charset=utf-8";
    }
    let binary = head
        .iter()
        .any(|b| matches!(b, 0x00..=0x08 | 0x0b | 0x0e..=0x1a | 0x1c..=0x1f));
    if !binary && std::str::from_utf8(head).is_ok() {
        "text/plain; charset=utf-8"
    } else if !binary && head.len() == 512 {
        // A multibyte character cut at the sniffing window still reads as text.
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

fn set_header(headers: &mut HeaderMap, k: &str, v: &str) {
    if let (Ok(name), Ok(val)) = (
        HeaderName::from_bytes(k.as_bytes()),
        HeaderValue::from_str(v),
    ) {
        headers.insert(name, val);
    }
}

/// The answer from the sync response store (Bento's `postHandler` tail).
fn build_sync_response(conf: &HttpServerConf, batches: Vec<Batch>) -> Response {
    let parts: Vec<Part> = batches.into_iter().flatten().collect();
    if parts.is_empty() {
        return status_only(StatusCode::OK);
    }
    let total: usize = parts.iter().map(|p| p.as_bytes_ref().len()).sum();
    if total > MAX_SYNC_RESPONSE_BYTES {
        tracing::error!("http_server: sync response of {total} bytes is over the limit");
        return status_only(StatusCode::BAD_GATEWAY);
    }
    let view = PartsView(&parts);
    let mut headers = HeaderMap::new();
    for (k, expr) in &conf.response_headers {
        match expr.resolve_string(0, &view) {
            Ok(v) => set_header(&mut headers, k, &v),
            Err(e) => tracing::error!("Interpolation of response header {k} error: {e}"),
        }
    }
    let status_str = match conf.response_status.resolve_string(0, &view) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Interpolation of response status code error: {e}");
            return status_only(StatusCode::BAD_GATEWAY);
        }
    };
    let status = if status_str == "200" {
        StatusCode::OK
    } else {
        match status_str
            .parse::<u16>()
            .ok()
            .and_then(|c| StatusCode::from_u16(c).ok())
        {
            Some(s) => s,
            None => {
                tracing::error!("Failed to parse sync response status code expression");
                return status_only(StatusCode::BAD_GATEWAY);
            }
        }
    };

    if let [part] = parts.as_slice() {
        for (k, v) in part.meta_all() {
            if conf.metadata_headers.matches(k) {
                set_header(&mut headers, k, &v.to_string_coerced());
            }
        }
        let payload = part.as_bytes_ref();
        if !headers.contains_key(axum::http::header::CONTENT_TYPE) {
            headers.insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static(detect_content_type(&payload)),
            );
        }
        let mut resp = Response::new(Body::from(payload));
        *resp.status_mut() = status;
        *resp.headers_mut() = headers;
        return resp;
    }

    // Bento looks the per-part content type up under the exact key `content-type`.
    let custom_ct = conf
        .response_header_keys
        .iter()
        .position(|k| k == "content-type")
        .and_then(|i| conf.response_headers.get(i))
        .map(|(_, e)| e);
    let boundary = uuid::Uuid::new_v4().simple().to_string();
    let mut body: Vec<u8> = Vec::with_capacity(total + parts.len() * 96);
    for (i, part) in parts.iter().enumerate() {
        for (k, v) in part.meta_all() {
            if conf.metadata_headers.matches(k) {
                set_header(&mut headers, k, &v.to_string_coerced());
            }
        }
        let payload = part.as_bytes_ref();
        let ct = match custom_ct {
            Some(expr) => expr
                .resolve_string(i, &view)
                .unwrap_or_else(|_| detect_content_type(&payload).to_string()),
            None => detect_content_type(&payload).to_string(),
        };
        if i > 0 {
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}\r\nContent-Type: {ct}\r\n\r\n").as_bytes());
        body.extend_from_slice(&payload);
    }
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    headers.remove(axum::http::header::CONTENT_TYPE);
    set_header(
        &mut headers,
        "Content-Type",
        &format!("multipart/form-data; boundary={boundary}"),
    );
    let mut resp = Response::new(Body::from(body));
    *resp.status_mut() = status;
    *resp.headers_mut() = headers;
    resp
}
