//! An in-process fake of the gateway Endpoint in front of Antares, and of Keycloak's token
//! endpoint, for the `ngsi_ld` tests (no docker). The fake keeps Antares' refusals: 411 without
//! `Content-Length`, 413 over 4 MiB, 400 over 1000 items, 415 for a media type that is not JSON,
//! 400 for `application/ld+json` with a Link header, 401 without a live token (when asked),
//! 207 for the entities named in `refuse`, else 204.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use axum::Router;
use bytes::Bytes;
use ekiben_bloblang::Environment;
use ekiben_core::{
    OrgId, OrgQuotas, ProcContext, Registry, ResourceManager, StreamContext, StreamMetrics,
};
use serde_json::{json, Value as JsonValue};
use tokio::sync::watch;

pub const FOUR_MIB: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub uri: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    }

    pub fn json(&self) -> JsonValue {
        serde_json::from_slice(&self.body).unwrap_or(JsonValue::Null)
    }

    pub fn path(&self) -> &str {
        self.uri.split('?').next().unwrap_or("")
    }

    pub fn query(&self) -> &str {
        self.uri.split_once('?').map(|(_, q)| q).unwrap_or("")
    }
}

#[derive(Clone, Debug)]
pub struct Scripted {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Default)]
pub struct FakeState {
    pub requests: Vec<Recorded>,
    /// Answers given before any other rule, in order.
    pub script: VecDeque<Scripted>,
    /// Entity id → the problem `detail` of its 207 member error.
    pub refuse: HashMap<String, String>,
    pub require_token: bool,
    pub tokens: HashMap<String, Instant>,
    pub token_ttl: u64,
    pub token_requests: usize,
    pub token_auth: Vec<String>,
    pub token_bodies: Vec<String>,
    pub token_form_answer: bool,
    /// When set, the token endpoint answers this status (and `Retry-After: 0`) to every request.
    pub token_status: Option<u16>,
    /// The entities `GET /entities` pages through.
    pub entities: Vec<JsonValue>,
    pub send_count: bool,
}

pub struct FakeBroker {
    pub addr: SocketAddr,
    pub state: Arc<Mutex<FakeState>>,
}

fn lock(st: &Mutex<FakeState>) -> std::sync::MutexGuard<'_, FakeState> {
    st.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl FakeBroker {
    pub async fn start() -> Self {
        let state = Arc::new(Mutex::new(FakeState {
            token_ttl: 3600,
            send_count: true,
            ..Default::default()
        }));
        let app = Router::new()
            .route("/token", post(token))
            .route("/{*path}", any(broker))
            .layer(DefaultBodyLimit::disable())
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { addr, state }
    }

    /// The gateway Endpoint base of the fake (`/api/endpoint/{slug}/ngsi-ld/v1`).
    pub fn base(&self) -> String {
        format!("http://{}/api/endpoint/test/ngsi-ld/v1", self.addr)
    }

    /// The base of an outside broker (no gateway Endpoint in the path).
    pub fn outside_base(&self) -> String {
        format!("http://{}/ngsi-ld/v1", self.addr)
    }

    pub fn token_url(&self) -> String {
        format!("http://{}/token", self.addr)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        lock(&self.state).requests.clone()
    }

    pub fn posts(&self) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == "POST")
            .collect()
    }

    pub fn script(&self, status: u16, headers: &[(&str, &str)], body: &str) {
        lock(&self.state).script.push_back(Scripted {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.to_string(),
        });
    }

    pub fn refuse(&self, id: &str, detail: &str) {
        lock(&self.state)
            .refuse
            .insert(id.to_string(), detail.to_string());
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut FakeState) -> R) -> R {
        f(&mut lock(&self.state))
    }
}

fn answer(status: u16, headers: &[(String, String)], body: String) -> Response {
    let mut resp = (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        body,
    )
        .into_response();
    for (k, v) in headers {
        if let (Ok(name), Ok(value)) = (
            axum::http::HeaderName::from_bytes(k.as_bytes()),
            axum::http::HeaderValue::from_str(v),
        ) {
            resp.headers_mut().insert(name, value);
        }
    }
    resp
}

fn problem(status: u16, title: &str, detail: &str) -> Response {
    answer(
        status,
        &[("content-type".into(), "application/json".into())],
        json!({"type": "https://uri.etsi.org/ngsi-ld/errors/BadRequestData", "title": title, "detail": detail})
            .to_string(),
    )
}

async fn token(
    State(st): State<Arc<Mutex<FakeState>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut s = lock(&st);
    s.token_requests += 1;
    s.token_auth.push(
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string(),
    );
    s.token_bodies
        .push(String::from_utf8_lossy(&body).into_owned());
    if let Some(status) = s.token_status {
        return answer(status, &[("retry-after".into(), "0".into())], "{}".into());
    }
    let tok = format!("tok-{}", s.token_requests);
    let ttl = s.token_ttl;
    s.tokens
        .insert(tok.clone(), Instant::now() + Duration::from_secs(ttl));
    if s.token_form_answer {
        answer(
            200,
            &[(
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
            format!("access_token={tok}&token_type=bearer&expires_in={ttl}"),
        )
    } else {
        answer(
            200,
            &[("content-type".into(), "application/json".into())],
            json!({"access_token": tok, "token_type": "bearer", "expires_in": ttl}).to_string(),
        )
    }
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

async fn broker(
    State(st): State<Arc<Mutex<FakeState>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut s = lock(&st);
    s.requests.push(Recorded {
        method: method.to_string(),
        uri: uri
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default(),
        headers: headers.clone(),
        body: body.clone(),
    });
    if s.require_token {
        let live = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .and_then(|t| s.tokens.get(t))
            .is_some_and(|until| Instant::now() < *until);
        if !live {
            return problem(401, "Unauthorized", "the token is missing or expired");
        }
    }
    if let Some(next) = s.script.pop_front() {
        return answer(next.status, &next.headers, next.body);
    }
    let query = uri.query().unwrap_or("");
    if method == Method::GET {
        if !uri.path().ends_with("/entities") || uri.path().contains("/temporal/") {
            return answer(200, &[], "[]".into());
        }
        let limit: usize = query_param(query, "limit")
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);
        let offset: usize = query_param(query, "offset")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let page: Vec<JsonValue> = s
            .entities
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();
        let mut hs = vec![("content-type".to_string(), "application/json".to_string())];
        if query_param(query, "count") == Some("true") && s.send_count {
            hs.push(("NGSILD-Results-Count".into(), s.entities.len().to_string()));
        }
        return answer(200, &hs, JsonValue::Array(page).to_string());
    }
    if method != Method::POST {
        return answer(405, &[], String::new());
    }
    if headers.get("content-length").is_none() {
        return problem(411, "Length Required", "Content-Length is required");
    }
    if body.len() > FOUR_MIB {
        return problem(413, "Payload Too Large", "the body is over 4 MiB");
    }
    let media = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if media == "application/ld+json" && headers.get("link").is_some() {
        return problem(400, "Bad Request", "ld+json with a Link header");
    }
    if media != "application/json" && media != "application/ld+json" {
        return problem(415, "Unsupported Media Type", "not JSON");
    }
    let Ok(JsonValue::Array(items)) = serde_json::from_slice::<JsonValue>(&body) else {
        return problem(400, "Bad Request", "the body is no array");
    };
    if items.len() > 1000 {
        return problem(400, "Bad Request", "more than 1000 items");
    }
    let mut success = Vec::new();
    let mut errors = Vec::new();
    for item in &items {
        let id = item
            .get("id")
            .and_then(JsonValue::as_str)
            .unwrap_or("unknown");
        match s.refuse.get(id) {
            Some(detail) => errors.push(json!({
                "entityId": id,
                "error": {"type": "https://uri.etsi.org/ngsi-ld/errors/BadRequestData", "title": "Bad Request", "detail": detail}
            })),
            None => success.push(json!(id)),
        }
    }
    if errors.is_empty() {
        return answer(204, &[], String::new());
    }
    answer(
        207,
        &[("content-type".into(), "application/json".into())],
        json!({"success": success, "errors": errors}).to_string(),
    )
}

/// A stream context of organization `org`. `allow_private_ips` lets it call the fake on
/// 127.0.0.1; without it the outbound rule applies. The sender keeps the stop signal alive.
pub fn ctx(
    org: &str,
    allow_private_ips: bool,
) -> (StreamContext, ProcContext, watch::Sender<bool>) {
    let reg = Arc::new(Registry::new());
    ekiben_io_ngsild::register_all(&reg);
    let resources = ResourceManager::new();
    let oid = OrgId::new(org);
    resources.set_org_quotas(
        &oid,
        OrgQuotas {
            allow_private_ips,
            ..Default::default()
        },
    );
    let org_ctx = resources.get_or_create_org(&oid);
    let (tx, rx) = watch::channel(false);
    let sctx = StreamContext {
        stream_id: format!("{org}-stream"),
        org: org_ctx,
        resources,
        registry: reg,
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new(format!("{org}-stream"), org),
        label: "ngsi".to_string(),
        path: "root".to_string(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    };
    let pctx = sctx.to_proc_context("ngsi_ld", "write_0", "pipeline.processors.0");
    (sctx, pctx, tx)
}

/// An NGSI-LD entity with one Property.
pub fn entity(id: &str) -> JsonValue {
    json!({"id": id, "type": "Vehicle", "speed": {"type": "Property", "value": 1}})
}

/// `{"url": base}` with the fields of `extra`.
pub fn conf(base: &str, extra: JsonValue) -> JsonValue {
    let mut m = serde_json::Map::new();
    m.insert("url".into(), JsonValue::String(base.to_string()));
    m.insert("retry_period".into(), JsonValue::String("10ms".into()));
    if let JsonValue::Object(e) = extra {
        for (k, v) in e {
            m.insert(k, v);
        }
    }
    JsonValue::Object(m)
}

/// The log lines written while the guard lives (this thread).
#[derive(Clone, Default)]
pub struct LogBuf(pub Arc<Mutex<Vec<u8>>>);

impl LogBuf {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(
            &self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_owned()
    }
}

impl std::io::Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A second registered dispatcher that lives for the whole test binary.
///
/// With exactly one dispatcher registered, tracing-core rebuilds a new callsite's interest from
/// the default of whichever thread hits it first; a parallel test thread without a subscriber
/// then caches `Interest::never` and the capturing test misses the event. With two dispatchers
/// the interest is combined across both (`sometimes`), so every thread asks its own default.
static KEEP_TWO_DISPATCHERS: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();

pub fn capture_logs() -> (LogBuf, tracing::subscriber::DefaultGuard) {
    KEEP_TWO_DISPATCHERS
        .get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
    let buf = LogBuf::default();
    let writer = buf.clone();
    let sub = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    (buf, tracing::subscriber::set_default(sub))
}
