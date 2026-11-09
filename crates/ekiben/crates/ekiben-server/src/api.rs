use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use ekiben_core::{CoreError, OrgId, StreamConfig};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use subtle::ConstantTimeEq;

use crate::manager::{ResourceError, StreamManager};
use crate::oidc::OidcAuth;
use crate::tokens::OrgTokenFile;

pub const MAX_API_BODY_BYTES: usize = 16 * 1024 * 1024; // 16 MiB

/// Authentication & multi-tenant token configuration for the Streams HTTP API (`Section 5 & 7`).
///
/// - No token configured and `require_auth` off: one tenant. Every call serves the default
///   organization and `X-Jc-Org` is ignored (honouring it would let any caller pick any
///   organization).
/// - `require_auth` on: every route but `/ping` and `/ready` needs a valid token; with no token
///   configured at all, every such call is refused (fail closed).
/// - `guard_probes` on as well (ekiben hosted in a shared process such as `jc`, which has probes
///   of its own): `/ready` needs a token too, only `/ping` answers without one.
/// - The admin token may act for the organization named by `X-Jc-Org` (default otherwise); an
///   organization token acts for its own organization only.
/// - `oidc`: a JWT of one of its clients acts as the admin token does (the Portal's runner client
///   in `jc`, signed by kaisatsu for audience `ekiben`).
#[derive(Clone, Default)]
pub struct ServerAuthConfig {
    /// Optional single admin bearer token (compared in constant time).
    pub admin_bearer_token: Option<String>,
    /// Map of per-organization bearer tokens -> `OrgId` (each compared in constant time).
    pub org_tokens: Vec<(String, OrgId)>,
    /// Refuse every unauthenticated call but `/ping` and `/ready`, even with no token configured.
    pub require_auth: bool,
    /// `--org-tokens-file`: `<org> <sha256 hex of token>` lines, re-read when the file changes.
    pub org_token_file: Option<Arc<OrgTokenFile>>,
    /// With `require_auth`, guard `/ready` as well, leaving only `/ping` open. Set when ekiben is
    /// hosted in a shared process (`jc`), where an open API would hand any caller a stream runner
    /// inside that process and no orchestrator probes ekiben's own `/ready`.
    pub guard_probes: bool,
    /// Admits JWTs an OIDC provider signs for ekiben ([`OidcAuth`]) as the admin caller.
    pub oidc: Option<Arc<OidcAuth>>,
}

impl std::fmt::Debug for ServerAuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerAuthConfig")
            .field(
                "admin_bearer_token",
                &self.admin_bearer_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "org_tokens",
                &self
                    .org_tokens
                    .iter()
                    .map(|(_, org)| ("[REDACTED]", org.0.as_str()))
                    .collect::<Vec<_>>(),
            )
            .field("require_auth", &self.require_auth)
            .field("guard_probes", &self.guard_probes)
            .field("oidc", &self.oidc)
            .field(
                "org_token_file",
                &self.org_token_file.as_ref().map(|f| f.path().to_path_buf()),
            )
            .finish()
    }
}

/// Who is calling: the organization the call acts for, and whether it may see every
/// organization (`/metrics`, `/ready` stream names).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub org: OrgId,
    /// The admin token was presented.
    pub admin: bool,
    /// No auth is configured at all (one tenant, everything is the default organization's).
    pub open: bool,
}

impl Caller {
    /// Sees every organization's series and stream names.
    pub fn sees_all(&self) -> bool {
        self.admin || self.open
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&[u8]> {
    let auth_str = headers.get("authorization")?.to_str().ok()?;
    auth_str
        .strip_prefix("Bearer ")
        .or_else(|| auth_str.strip_prefix("bearer "))
        .map(str::as_bytes)
}

impl ServerAuthConfig {
    pub fn is_enabled(&self) -> bool {
        self.admin_bearer_token.is_some()
            || !self.org_tokens.is_empty()
            || self.org_token_file.is_some()
            || self.oidc.is_some()
    }

    /// Whether `provided` is a JWT the OIDC provider signed for an admitted client.
    fn oidc_admits(&self, provided: &[u8]) -> bool {
        self.oidc
            .as_ref()
            .is_some_and(|o| std::str::from_utf8(provided).is_ok_and(|token| o.verify(token)))
    }

    /// Whether the auth middleware guards the API.
    pub fn guards_routes(&self) -> bool {
        self.require_auth || self.is_enabled()
    }

    /// Verifies the request's `Authorization: Bearer <token>` header in constant time and
    /// returns the organization the call acts for (always `Some`): the default organization
    /// without auth, the `X-Jc-Org` organization (or the default) for the admin token, the
    /// token's organization for an organization token. `401` for a missing or wrong token,
    /// `403` for an organization token naming another organization in `X-Jc-Org`.
    pub fn authenticate(&self, headers: &HeaderMap) -> Result<Option<OrgId>, StatusCode> {
        self.identify(headers).map(|c| Some(c.org))
    }

    /// As [`Self::authenticate`], with whether the caller is the admin.
    pub fn identify(&self, headers: &HeaderMap) -> Result<Caller, StatusCode> {
        if !self.is_enabled() {
            if self.require_auth {
                return Err(StatusCode::UNAUTHORIZED);
            }
            return Ok(Caller {
                org: OrgId::default(),
                admin: false,
                open: true,
            });
        }

        let header_org = match headers.get("x-jc-org").map(|v| v.to_str()) {
            None => None,
            Some(Ok("")) => None,
            Some(Ok(s)) if validate_stream_id(s).is_ok() => Some(OrgId::new(s)),
            Some(_) => return Err(StatusCode::BAD_REQUEST),
        };

        let provided = bearer_token(headers).ok_or(StatusCode::UNAUTHORIZED)?;

        if let Some(ref admin_tok) = self.admin_bearer_token {
            if constant_time_eq_slices(provided, admin_tok.as_bytes()) {
                return Ok(Caller {
                    org: header_org.unwrap_or_default(),
                    admin: true,
                    open: false,
                });
            }
        }

        if let Some(org) = self.match_org_token(provided) {
            if let Some(ref requested_org) = header_org {
                if requested_org != &org {
                    return Err(StatusCode::FORBIDDEN);
                }
            }
            return Ok(Caller {
                org,
                admin: false,
                open: false,
            });
        }

        if self.oidc_admits(provided) {
            return Ok(Caller {
                org: header_org.unwrap_or_default(),
                admin: true,
                open: false,
            });
        }

        Err(StatusCode::UNAUTHORIZED)
    }

    /// The organization of an organization token (static list, then the tokens file); every
    /// entry is compared, in constant time, whatever matches first.
    fn match_org_token(&self, provided: &[u8]) -> Option<OrgId> {
        let mut matched_org: Option<OrgId> = None;
        for (tok, org_id) in &self.org_tokens {
            if constant_time_eq_slices(provided, tok.as_bytes()) && matched_org.is_none() {
                matched_org = Some(org_id.clone());
            }
        }
        if let Some(file) = &self.org_token_file {
            if let Some(org) = file.match_token(provided) {
                if matched_org.is_none() {
                    matched_org = Some(org);
                }
            }
        }
        matched_org
    }

    /// Admits only the admin bearer token: `401` without a valid token, `403` for an organization
    /// token or when no admin token is configured (a server without auth has no admin).
    pub fn authenticate_admin(&self, headers: &HeaderMap) -> Result<(), StatusCode> {
        if self.admin_bearer_token.is_none() && self.oidc.is_none() {
            return Err(StatusCode::FORBIDDEN);
        }
        let provided = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| {
                s.strip_prefix("Bearer ")
                    .or_else(|| s.strip_prefix("bearer "))
            })
            .ok_or(StatusCode::UNAUTHORIZED)?
            .as_bytes();
        if let Some(ref admin_tok) = self.admin_bearer_token {
            if constant_time_eq_slices(provided, admin_tok.as_bytes()) {
                return Ok(());
            }
        }
        if self.oidc_admits(provided) {
            return Ok(());
        }
        if self.match_org_token(provided).is_some() {
            return Err(StatusCode::FORBIDDEN);
        }
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Routes that answer without a token: the liveness probe, and the readiness probe unless
/// [`ServerAuthConfig::guard_probes`] closes it.
fn is_open_route(path: &str, guard_probes: bool) -> bool {
    match path {
        "/ping" | "/bento/ping" => true,
        "/ready" | "/bento/ready" => !guard_probes,
        _ => false,
    }
}

/// The `require_auth` middleware: every route but `/ping` and `/ready` (only `/ping` with
/// `guard_probes`) needs a valid token whenever auth is configured or required. Missing and
/// wrong tokens get the same 401 body.
async fn require_auth_layer(
    State(state): State<Arc<ServerState>>,
    req: Request,
    next: Next,
) -> Response {
    if state.auth.guards_routes() && !is_open_route(req.uri().path(), state.auth.guard_probes) {
        if let Some(oidc) = &state.auth.oidc {
            let token = bearer_token(req.headers())
                .and_then(|t| std::str::from_utf8(t).ok())
                .map(str::to_owned);
            if let Some(token) = token {
                oidc.prepare(&token).await;
            }
        }
        match state.auth.identify(req.headers()) {
            Ok(_) => {}
            Err(StatusCode::FORBIDDEN) => {
                return (StatusCode::FORBIDDEN, "Forbidden").into_response()
            }
            Err(StatusCode::BAD_REQUEST) => {
                return (StatusCode::BAD_REQUEST, "invalid X-Jc-Org header").into_response()
            }
            Err(_) => {
                return (
                    StatusCode::UNAUTHORIZED,
                    [("www-authenticate", "Bearer")],
                    "Unauthorized",
                )
                    .into_response()
            }
        }
    }
    next.run(req).await
}

fn constant_time_eq_slices(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

#[derive(Clone)]
pub struct ServerState {
    pub manager: StreamManager,
    pub auth: ServerAuthConfig,
    pub version: String,
    pub built: String,
}

impl ServerState {
    pub fn new(manager: StreamManager, auth: ServerAuthConfig) -> Arc<Self> {
        Arc::new(Self {
            manager,
            auth,
            version: env!("CARGO_PKG_VERSION").to_string(),
            built: "2026-11-07".to_string(),
        })
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct StreamsQueryParams {
    pub chilled: Option<String>,
    pub run: Option<String>,
}

impl StreamsQueryParams {
    fn is_chilled(&self) -> bool {
        self.chilled.as_deref() == Some("true")
    }

    fn is_run_once(&self) -> bool {
        self.run.as_deref() == Some("once")
    }
}

/// Builds the `axum::Router` for the Bento-compatible Streams HTTP API (`internal/api/api.go`, `internal/stream/manager/api.go`).
/// Registers all endpoints at both `/` and `/bento` (`root_path`).
pub fn build_router(state: Arc<ServerState>) -> Router {
    let api_routes = Router::new()
        .route("/ping", get(handle_ping))
        .route("/version", get(handle_version))
        .route("/ready", get(handle_ready))
        .route("/endpoints", get(handle_endpoints))
        .route("/metrics", get(handle_metrics))
        .route("/stats", get(handle_metrics))
        .route(
            "/streams",
            get(handle_list_streams).post(handle_set_streams),
        )
        .route(
            "/streams/{id}",
            get(handle_get_stream)
                .post(handle_create_stream)
                .put(handle_put_stream)
                .patch(handle_patch_stream)
                .delete(handle_delete_stream),
        )
        .route("/streams/{id}/stats", get(handle_stream_stats))
        .route("/resources/{type}/{id}", post(handle_post_resource))
        .route(
            "/orgs/{org}/quotas",
            get(handle_get_quotas).put(handle_put_quotas),
        );

    Router::new()
        .merge(api_routes.clone())
        .nest("/bento", api_routes)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_auth_layer,
        ))
        .layer(DefaultBodyLimit::max(MAX_API_BODY_BYTES))
        .with_state(state)
}

async fn handle_ping() -> &'static str {
    "pong"
}

async fn handle_version(State(state): State<Arc<ServerState>>) -> Response {
    Json(serde_json::json!({
        "version": state.version,
        "built": state.built,
    }))
    .into_response()
}

async fn handle_endpoints() -> Response {
    Json(serde_json::json!({
        "/ping": "Ping me.",
        "/version": "Returns the service version.",
        "/ready": "Returns 200 OK if all active streams are connected, otherwise 503.",
        "/endpoints": "Returns this map of endpoints.",
        "/metrics": "Exposes service-wide metrics in Prometheus format.",
        "/stats": "Exposes service-wide metrics.",
        "/streams": "GET: List all streams. POST: Replace all streams.",
        "/streams/{id}": "Perform CRUD operations on streams (POST, GET, PUT, PATCH, DELETE).",
        "/streams/{id}/stats": "GET a structured JSON object containing metrics for the stream.",
        "/resources/{type}/{id}": "POST: Create or replace a resource configuration."
    }))
    .into_response()
}

/// `GET /ready` stays open for probes; the names of streams that are not connected are shown
/// only to an authenticated caller, and only its own organization's (all for the admin).
async fn handle_ready(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    let not_ready = state.manager.not_ready_streams().await;
    if not_ready.is_empty() {
        return (StatusCode::OK, "OK").into_response();
    }
    let body = match state.auth.identify(&headers) {
        Ok(caller) => {
            let names: Vec<String> = not_ready
                .into_iter()
                .filter(|(org, _)| caller.sees_all() || org == &caller.org)
                .map(|(_, id)| id)
                .collect();
            if names.is_empty() {
                "streams are not connected\n".to_string()
            } else {
                format!("streams {} are not connected\n", names.join(", "))
            }
        }
        Err(_) => "streams are not connected\n".to_string(),
    };
    (StatusCode::SERVICE_UNAVAILABLE, body).into_response()
}

/// `GET /metrics`: Bento's Prometheus exposition. An organization token sees only its own
/// organization's series; the admin (or a server without auth) sees them all.
async fn handle_metrics(State(state): State<Arc<ServerState>>, headers: HeaderMap) -> Response {
    let caller = match state.auth.identify(&headers) {
        Ok(c) => c,
        Err(code) => return (code, "Unauthorized").into_response(),
    };
    let body = if caller.sees_all() {
        state.manager.metrics_registry.render_prometheus()
    } else {
        state
            .manager
            .metrics_registry
            .render_prometheus_for_org(&caller.org.0)
    };
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
        .into_response()
}

async fn handle_list_streams(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Response {
    let caller_org = match state.auth.authenticate(&headers) {
        Ok(org) => org,
        Err(code) => return (code, "Unauthorized").into_response(),
    };
    let list = state.manager.list(caller_org.as_ref()).await;
    Json(list).into_response()
}

async fn handle_set_streams(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(params): Query<StreamsQueryParams>,
    body: Bytes,
) -> Response {
    let caller_org = match state.auth.authenticate(&headers) {
        Ok(org) => org,
        Err(code) => return (code, "Unauthorized").into_response(),
    };
    let chilled = params.is_chilled();
    let body_str = String::from_utf8_lossy(&body);

    let raw_map: HashMap<String, JsonValue> = match serde_norway::from_str(&body_str) {
        Ok(m) => m,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("Error: {e}")).into_response(),
    };

    let mut parsed_streams = Vec::with_capacity(raw_map.len());
    let mut all_lints = Vec::new();

    for (id, val) in &raw_map {
        if let Err(e) = validate_stream_id(id) {
            return (StatusCode::BAD_REQUEST, e).into_response();
        }
        let val_str = serde_json::to_string(val).unwrap_or_default();
        match parse_and_lint_stream(&state, &val_str, chilled, caller_org.as_ref()) {
            Ok((conf, lints, _)) => {
                for l in lints {
                    all_lints.push(format!("stream '{id}': {l}"));
                }
                parsed_streams.push((id.clone(), conf, Some(val.clone())));
            }
            Err(resp) => return *resp,
        }
    }

    if !chilled && !all_lints.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "lint_errors": all_lints,
                "linting_errors": all_lints,
            })),
        )
            .into_response();
    }

    let existing = state.manager.list(caller_org.as_ref()).await;
    for old_id in existing.keys() {
        if !raw_map.contains_key(old_id) {
            let _ = state.manager.delete(old_id, caller_org.as_ref()).await;
        }
    }

    for (id, conf, posted) in parsed_streams {
        let res = if existing.contains_key(&id) {
            state
                .manager
                .update_posted(&id, conf, caller_org.as_ref(), posted)
                .await
        } else {
            state.manager.create_posted(id, conf, posted).await
        };
        if let Err(e) = res {
            return map_core_err_to_response(e);
        }
    }

    StatusCode::OK.into_response()
}

async fn handle_get_stream(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let caller_org = match state.auth.authenticate(&headers) {
        Ok(org) => org,
        Err(code) => return (code, "Unauthorized").into_response(),
    };
    match state.manager.read(&id, caller_org.as_ref()).await {
        Ok(info) => Json(info).into_response(),
        Err(e) => map_core_err_to_response(e),
    }
}

async fn handle_create_stream(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<StreamsQueryParams>,
    body: Bytes,
) -> Response {
    let caller_org = match state.auth.authenticate(&headers) {
        Ok(org) => org,
        Err(code) => return (code, "Unauthorized").into_response(),
    };
    if let Err(e) = validate_stream_id(&id) {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }
    let chilled = params.is_chilled();
    let body_str = String::from_utf8_lossy(&body);

    let (mut conf, lints, posted) =
        match parse_and_lint_stream(&state, &body_str, chilled, caller_org.as_ref()) {
            Ok(parsed) => parsed,
            Err(resp) => return *resp,
        };

    if !chilled && !lints.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "lint_errors": lints,
                "linting_errors": lints,
            })),
        )
            .into_response();
    }

    if params.is_run_once() {
        conf.run = ekiben_core::RunMode::Once;
    }

    match state.manager.create_posted(id, conf, posted).await {
        Ok(Some(summary)) => Json(summary).into_response(),
        Ok(None) => StatusCode::OK.into_response(),
        Err(e) => map_core_err_to_response(e),
    }
}

async fn handle_put_stream(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<StreamsQueryParams>,
    body: Bytes,
) -> Response {
    let caller_org = match state.auth.authenticate(&headers) {
        Ok(org) => org,
        Err(code) => return (code, "Unauthorized").into_response(),
    };
    if let Err(e) = validate_stream_id(&id) {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }
    let chilled = params.is_chilled();
    let body_str = String::from_utf8_lossy(&body);

    let (mut conf, lints, posted) =
        match parse_and_lint_stream(&state, &body_str, chilled, caller_org.as_ref()) {
            Ok(parsed) => parsed,
            Err(resp) => return *resp,
        };

    if !chilled && !lints.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "lint_errors": lints,
                "linting_errors": lints,
            })),
        )
            .into_response();
    }

    if params.is_run_once() {
        conf.run = ekiben_core::RunMode::Once;
    }

    match state
        .manager
        .update_posted(&id, conf, caller_org.as_ref(), posted)
        .await
    {
        Ok(Some(summary)) => Json(summary).into_response(),
        Ok(None) => StatusCode::OK.into_response(),
        Err(e) => map_core_err_to_response(e),
    }
}

async fn handle_patch_stream(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let caller_org = match state.auth.authenticate(&headers) {
        Ok(org) => org,
        Err(code) => return (code, "Unauthorized").into_response(),
    };
    let patch_json: JsonValue = match serde_norway::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("Error: {e}")).into_response(),
    };
    match state
        .manager
        .patch(&id, &patch_json, caller_org.as_ref())
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => map_core_err_to_response(e),
    }
}

async fn handle_delete_stream(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let caller_org = match state.auth.authenticate(&headers) {
        Ok(org) => org,
        Err(code) => return (code, "Unauthorized").into_response(),
    };
    match state.manager.delete(&id, caller_org.as_ref()).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => map_core_err_to_response(e),
    }
}

async fn handle_stream_stats(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let caller_org = match state.auth.authenticate(&headers) {
        Ok(org) => org,
        Err(code) => return (code, "Unauthorized").into_response(),
    };
    match state.manager.read_stats(&id, caller_org.as_ref()).await {
        Ok(stats) => Json(stats).into_response(),
        Err(e) => map_core_err_to_response(e),
    }
}

async fn handle_post_resource(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path((res_type, id)): Path<(String, String)>,
    Query(params): Query<StreamsQueryParams>,
    body: Bytes,
) -> Response {
    let caller_org = match state.auth.authenticate(&headers) {
        Ok(org) => org,
        Err(code) => return (code, "Unauthorized").into_response(),
    };
    let org_id = caller_org.unwrap_or_default();
    let chilled = params.is_chilled();
    let body_str = String::from_utf8_lossy(&body);

    let interpolated = match state.manager.interpolate(&org_id, &body_str) {
        Ok(s) => s,
        Err(err) => {
            if !chilled {
                let lints: Vec<String> = err
                    .missing
                    .into_iter()
                    .map(|m| format!("environment variable '{m}' was not defined"))
                    .collect();
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "lint_errors": lints,
                        "linting_errors": lints,
                    })),
                )
                    .into_response();
            }
            err.best_attempt
        }
    };

    match state
        .manager
        .put_resource(&org_id, &res_type, &id, &interpolated)
    {
        Ok(()) => match state
            .manager
            .save_resource(&org_id, &res_type, &id, &body_str)
            .await
        {
            Ok(()) => StatusCode::OK.into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("the resource was not saved: {e}"),
            )
                .into_response(),
        },
        Err(ResourceError::Parse(why)) => (StatusCode::BAD_REQUEST, why).into_response(),
        Err(ResourceError::Kind) => (
            StatusCode::BAD_REQUEST,
            "Var `type` must be set to one of `cache`, `input`, `output`, `processor` or `rate_limit`",
        )
            .into_response(),
        Err(ResourceError::Build(e)) => {
            if !chilled {
                let lints = vec![e.to_string()];
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "lint_errors": lints,
                        "linting_errors": lints,
                    })),
                )
                    .into_response()
            } else {
                (StatusCode::BAD_REQUEST, e.to_string()).into_response()
            }
        }
    }
}

/// A parsed stream config, its lint errors and the config as posted (before interpolation).
type ParsedStream = (StreamConfig, Vec<String>, Option<JsonValue>);

fn parse_and_lint_stream(
    state: &ServerState,
    body_str: &str,
    chilled: bool,
    caller_org: Option<&OrgId>,
) -> Result<ParsedStream, Box<Response>> {
    let mut env_lints = Vec::new();
    // `${VAR}` comes from the caller's organization's variables, then the process environment
    // only as the server's `EnvAccess` allows (E1).
    let vars_org = caller_org.cloned().unwrap_or_default();
    let interpolated = match state.manager.interpolate(&vars_org, body_str) {
        Ok(s) => s,
        Err(err) => {
            for m in err.missing {
                env_lints.push(format!("environment variable '{m}' was not defined"));
            }
            err.best_attempt
        }
    };

    let raw_json: JsonValue = serde_norway::from_str(&interpolated)
        .map_err(|e| Box::new((StatusCode::BAD_REQUEST, format!("Error: {e}")).into_response()))?;

    let mut conf = StreamConfig::from_json_value(&raw_json)
        .map_err(|e| Box::new((StatusCode::BAD_REQUEST, format!("Error: {e}")).into_response()))?;

    if let Some(c_org) = caller_org {
        if conf.org != OrgId::default() && &conf.org != c_org {
            return Err(Box::new(
                (
                    StatusCode::FORBIDDEN,
                    "Forbidden: stream config specifies another organization",
                )
                    .into_response(),
            ));
        }
        conf.org = c_org.clone();
    }

    let mut lint_report = state.manager.registry.lint_stream_config(&conf, env_lints);
    // Streams mode: an http_server input serves on the shared ingest listener (E4).
    for err in ekiben_io_http::http_server::lint_streams_mode(&conf.raw_source) {
        lint_report.add_error(err);
    }

    let _ = chilled;
    let posted = StreamConfig::posted_json(body_str);
    Ok((conf, lint_report.lint_errors, posted))
}

/// `GET /orgs/{org}/quotas` (admin only): the organization's current quotas.
async fn handle_get_quotas(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
) -> Response {
    if let Err(code) = state.auth.authenticate_admin(&headers) {
        return (code, "the quota API needs the admin token").into_response();
    }
    if let Err(msg) = validate_stream_id(&org) {
        return (
            StatusCode::BAD_REQUEST,
            msg.replace("stream id", "organization id"),
        )
            .into_response();
    }
    let ctx = state.manager.resources.get_or_create_org(&OrgId::new(org));
    Json(ctx.get_quotas()).into_response()
}

/// `PUT /orgs/{org}/quotas` (admin only): replaces the organization's quotas (a missing field
/// takes its default). Running streams of that organization pace by the new rates within one
/// second, without a restart; other organizations are untouched.
async fn handle_put_quotas(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(org): Path<String>,
    body: Bytes,
) -> Response {
    if let Err(code) = state.auth.authenticate_admin(&headers) {
        return (code, "the quota API needs the admin token").into_response();
    }
    if let Err(msg) = validate_stream_id(&org) {
        return (
            StatusCode::BAD_REQUEST,
            msg.replace("stream id", "organization id"),
        )
            .into_response();
    }
    let quotas = match parse_quotas(&body) {
        Ok(q) => q,
        Err(msg) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("invalid quotas: {msg} (e.g. {{\"max_messages_per_sec\": 50000}})"),
            )
                .into_response()
        }
    };
    if let Err(field) = quotas.validate() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("{field} must be at least 1"),
        )
            .into_response();
    }
    let ctx = state
        .manager
        .resources
        .set_org_quotas(&OrgId::new(org), quotas);
    Json(ctx.get_quotas()).into_response()
}

/// Parses a quotas body; an error names the offending field (serde alone omits it for a bad value).
fn parse_quotas(body: &[u8]) -> Result<ekiben_core::OrgQuotas, String> {
    let value: JsonValue = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    serde_json::from_value(value.clone()).map_err(|e| {
        let field = value.as_object().and_then(|map| {
            map.iter().find_map(|(k, v)| {
                let one = serde_json::json!({ k: v });
                serde_json::from_value::<ekiben_core::OrgQuotas>(one)
                    .is_err()
                    .then_some(k.clone())
            })
        });
        match field {
            Some(name) => format!("field `{name}`: {e}"),
            None => e.to_string(),
        }
    })
}

pub(crate) fn validate_stream_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 256 {
        return Err("invalid stream id length".to_string());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
    {
        return Err(format!("stream id '{id}' contains disallowed characters"));
    }
    Ok(())
}

fn map_core_err_to_response(err: CoreError) -> Response {
    match err {
        CoreError::KeyNotFound(_) => (StatusCode::NOT_FOUND, "Stream not found").into_response(),
        CoreError::KeyAlreadyExists(_) => {
            (StatusCode::BAD_REQUEST, "Stream already exists").into_response()
        }
        CoreError::AccessDenied { reason, .. } => (StatusCode::FORBIDDEN, reason).into_response(),
        CoreError::QuotaExceeded { reason, .. } => {
            (StatusCode::TOO_MANY_REQUESTS, reason).into_response()
        }
        CoreError::Config(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
        CoreError::MappingParse(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        other => (StatusCode::BAD_GATEWAY, format!("Error: {other}")).into_response(),
    }
}
