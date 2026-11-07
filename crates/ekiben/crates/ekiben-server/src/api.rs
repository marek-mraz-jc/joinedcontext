use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use ekiben_bloblang::Environment;
use ekiben_core::{ComponentConfig, CoreError, OrgId, StreamConfig, StreamContext, StreamMetrics};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use subtle::ConstantTimeEq;
use tokio::sync::watch;

use crate::manager::StreamManager;

pub const MAX_API_BODY_BYTES: usize = 16 * 1024 * 1024; // 16 MiB

/// Authentication & multi-tenant token configuration for the Streams HTTP API (`Section 5 & 7`).
#[derive(Clone, Default)]
pub struct ServerAuthConfig {
    /// Optional single admin bearer token (compared in constant time).
    pub admin_bearer_token: Option<String>,
    /// Map of per-organization bearer tokens -> `OrgId` (each compared in constant time).
    pub org_tokens: Vec<(String, OrgId)>,
}

impl ServerAuthConfig {
    pub fn is_enabled(&self) -> bool {
        self.admin_bearer_token.is_some() || !self.org_tokens.is_empty()
    }

    /// Verifies the request's `Authorization: Bearer <token>` header in constant time.
    /// Returns:
    /// - `Ok(None)` if auth is disabled (or admin token matched and no `X-Jc-Org` header was provided).
    /// - `Ok(Some(OrgId))` if an organization token matched (or `X-Jc-Org` was provided with admin/local auth).
    /// - `Err(StatusCode)` (`401 Unauthorized` or `403 Forbidden`) if token is missing/invalid.
    pub fn authenticate(&self, headers: &HeaderMap) -> Result<Option<OrgId>, StatusCode> {
        let header_org = headers
            .get("x-jc-org")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
            .map(OrgId::new);

        if !self.is_enabled() {
            return Ok(header_org);
        }

        let auth_str = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::UNAUTHORIZED)?;

        let provided = auth_str
            .strip_prefix("Bearer ")
            .or_else(|| auth_str.strip_prefix("bearer "))
            .ok_or(StatusCode::UNAUTHORIZED)?
            .as_bytes();

        if let Some(ref admin_tok) = self.admin_bearer_token {
            if constant_time_eq_slices(provided, admin_tok.as_bytes()) {
                return Ok(header_org);
            }
        }

        let mut matched_org: Option<OrgId> = None;
        for (tok, org_id) in &self.org_tokens {
            if constant_time_eq_slices(provided, tok.as_bytes()) {
                matched_org = Some(org_id.clone());
            }
        }

        if let Some(org) = matched_org {
            if let Some(ref requested_org) = header_org {
                if requested_org != &org {
                    return Err(StatusCode::FORBIDDEN);
                }
            }
            return Ok(Some(org));
        }

        Err(StatusCode::UNAUTHORIZED)
    }
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
        .route("/resources/{type}/{id}", post(handle_post_resource));

    Router::new()
        .merge(api_routes.clone())
        .nest("/bento", api_routes)
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

async fn handle_ready(State(state): State<Arc<ServerState>>) -> Response {
    let not_ready = state.manager.not_ready_streams().await;
    if not_ready.is_empty() {
        (StatusCode::OK, "OK").into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("streams {} are not connected\n", not_ready.join(", ")),
        )
            .into_response()
    }
}

async fn handle_metrics(State(state): State<Arc<ServerState>>) -> Response {
    let body = state.manager.metrics_registry.render_prometheus();
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
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
            Ok((conf, lints)) => {
                for l in lints {
                    all_lints.push(format!("stream '{id}': {l}"));
                }
                parsed_streams.push((id.clone(), conf));
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

    for (id, conf) in parsed_streams {
        let res = if existing.contains_key(&id) {
            state.manager.update(&id, conf, caller_org.as_ref()).await
        } else {
            state.manager.create(id, conf).await
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

    let (mut conf, lints) =
        match parse_and_lint_stream(&state, &body_str, chilled, caller_org.as_ref()) {
            Ok(pair) => pair,
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

    match state.manager.create(id, conf).await {
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

    let (mut conf, lints) =
        match parse_and_lint_stream(&state, &body_str, chilled, caller_org.as_ref()) {
            Ok(pair) => pair,
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

    match state.manager.update(&id, conf, caller_org.as_ref()).await {
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

    let interpolated = match ekiben_core::interpolate_env_str(&body_str, |k| std::env::var(k).ok())
    {
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

    let raw_val: JsonValue = match serde_norway::from_str(&interpolated) {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("Error: {e}")).into_response(),
    };
    let comp_conf = match ComponentConfig::from_json_value(&raw_val, "noop", None) {
        Ok(mut c) => {
            c.label = id.clone();
            c
        }
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };

    let (_tx, rx) = watch::channel(false);
    let org_ctx = state.manager.resources.get_or_create_org(&org_id);
    let sctx = StreamContext {
        stream_id: format!("resource_{id}"),
        org: org_ctx,
        resources: state.manager.resources.clone(),
        registry: state.manager.registry.clone(),
        blobl_env: Environment::new().with_host_env(state.manager.allow_host_env),
        metrics: StreamMetrics::new(&id, &org_id.0),
        label: id.clone(),
        path: format!("resources.{res_type}.{id}"),
        file_root: state.manager.file_root.clone(),
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    };

    let res: Result<(), CoreError> = match res_type.as_str() {
        "cache" => state
            .manager
            .registry
            .build_cache(&comp_conf, &sctx)
            .map(|c| state.manager.resources.insert_cache(&org_id, id, c)),
        "rate_limit" => state
            .manager
            .registry
            .build_rate_limit(&comp_conf, &sctx)
            .map(|rl| state.manager.resources.insert_rate_limit(&org_id, id, rl)),
        "processor" => {
            let pctx = sctx.to_proc_context(comp_conf.component_type.clone(), id.clone(), "resource");
            state
                .manager
                .registry
                .build_processor(&comp_conf, &pctx, &id)
                .map(|p| state.manager.resources.insert_processor(&org_id, id, p))
        }
        "input" => state
            .manager
            .registry
            .build_input(&comp_conf, &sctx)
            .map(|inp| state.manager.resources.insert_input(&org_id, id, inp)),
        "output" => state
            .manager
            .registry
            .build_output(&comp_conf, &sctx)
            .map(|out| state.manager.resources.insert_output(&org_id, id, out)),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "Var `type` must be set to one of `cache`, `input`, `output`, `processor` or `rate_limit`",
            )
                .into_response()
        }
    };

    match res {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => {
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

fn parse_and_lint_stream(
    state: &ServerState,
    body_str: &str,
    chilled: bool,
    caller_org: Option<&OrgId>,
) -> Result<(StreamConfig, Vec<String>), Box<Response>> {
    let mut env_lints = Vec::new();
    let interpolated = match ekiben_core::interpolate_env_str(body_str, |k| std::env::var(k).ok()) {
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

    let lint_report = state.manager.registry.lint_stream_config(&conf, env_lints);

    let _ = chilled;
    Ok((conf, lint_report.lint_errors))
}

fn validate_stream_id(id: &str) -> Result<(), String> {
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
