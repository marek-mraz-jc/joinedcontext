//! `jc-functions`: the runtime of application functions (SDK-22, SDK-23, Architecture/20 §3).
//!
//! One route, `POST /invoke`, for the Portal: its Keycloak token must name the audience
//! `jc-functions` and be issued to the Portal's client. The knowledge assistant may call it too,
//! with its own client's token and only with `via: "none"`: a script over data it hands in, with
//! no endpoint and no network (AG-112, T-3056). The runtime holds no credential and no
//! code of its own; every call brings the files, the request and the caller's token, and runs in
//! a fresh QuickJS runtime on a thread of its own, at most [`SLOTS`] at a time.

pub mod endpoint;
pub mod sandbox;

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use jc_core::auth::token::{bearer, Verifier};
use jc_core::ProblemDetails;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Semaphore;

use endpoint::Endpoint;
use sandbox::Invocation;

/// Invocations running at once per replica; the next one answers 429.
pub const SLOTS: usize = 16;
/// The whole invocation: the files, request and token.
pub const INVOCATION_LIMIT: usize = 8 * 1024 * 1024;
/// The function's request body, as JSON, for a call with an endpoint (`via` gateway or proxy).
pub const REQUEST_BODY_LIMIT: usize = 256 * 1024;
/// The request body of a script with no endpoint (`via: "none"`): the data the assistant read for
/// it. Measured (T-7774): 4 MiB of register entities parse and group inside the 64 MiB heap with
/// room to spare (`a_4_mib_body_parses_and_groups_inside_the_heap`).
pub const SCRIPT_BODY_LIMIT: usize = 4 * 1024 * 1024;

pub struct AppState {
    pub verifier: Arc<Verifier>,
    /// The audience a caller's token must name (`JC_FUNCTIONS_AUDIENCE`).
    pub audience: String,
    /// The Keycloak client a caller's token must be issued to (`JC_FUNCTIONS_CALLER`).
    pub caller: String,
    /// The knowledge assistant's client (`JC_FUNCTIONS_ASSISTANT_CALLER`), whose token may run a
    /// script with no network and nothing else; `None` admits no such caller.
    pub assistant_caller: Option<String>,
    /// Scheme and authority of the Context Gateway (`JC_GATEWAY_URL`).
    pub gateway: String,
    /// Scheme and authority of the agent proxy (`JC_AGENT_PROXY_URL`), for a call `via: "proxy"`;
    /// `None` refuses such a call (ADR-N-038 decision 6).
    pub proxy: Option<String>,
    pub http: reqwest::Client,
    pub slots: Arc<Semaphore>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InvokeRequest {
    files: BTreeMap<String, String>,
    entry: String,
    request: FnRequest,
    config: Value,
    #[serde(default)]
    token: Option<String>,
    /// `proxy` for an editing agent's own call: its token is the run's data credential and its
    /// requests go through this runtime's agent proxy, never an address the call names.
    #[serde(default)]
    via: Via,
}

#[derive(Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Via {
    #[default]
    Gateway,
    Proxy,
    /// No endpoint, no host function: the code reads what the request carries and nothing else.
    None,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct FnRequest {
    method: String,
    #[serde(default)]
    query: BTreeMap<String, String>,
    #[serde(default)]
    body: Value,
    #[serde(default)]
    user: Value,
}

/// The platform's own problem type for `status` (T-3243), never a slug made of its reason phrase.
fn problem(status: StatusCode, detail: impl Into<String>) -> Response {
    ProblemDetails::for_status(status.as_u16())
        .with_detail(detail)
        .into_response()
}

async fn invoke(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let azp = bearer(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok()),
    )
    .and_then(|token| {
        state
            .verifier
            .verify(token, std::slice::from_ref(&state.audience))
    })
    .ok()
    .and_then(|claims| claims.azp);
    let portal = azp.as_deref() == Some(state.caller.as_str());
    let assistant = azp.is_some() && azp.as_deref() == state.assistant_caller.as_deref();
    if !portal && !assistant {
        return problem(
            StatusCode::UNAUTHORIZED,
            "a token for jc-functions issued to the Portal",
        );
    }
    let Ok(permit) = state.slots.clone().try_acquire_owned() else {
        return problem(
            StatusCode::TOO_MANY_REQUESTS,
            format!("{SLOTS} invocations are running"),
        );
    };
    let invocation = match prepare(&state, &body, assistant) {
        Ok(invocation) => invocation,
        Err(refusal) => return *refusal,
    };
    // A QuickJS runtime is not `Send`: each call gets a thread and a single-threaded executor.
    let outcome = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(|executor| executor.block_on(sandbox::run(invocation)))
    })
    .await;
    match outcome {
        Ok(Ok(outcome)) => Json(outcome).into_response(),
        _ => problem(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the invocation could not be run",
        ),
    }
}

/// The most of a JWKS document the runtime reads, and how long one fetch may take.
const KEYS_LIMIT: usize = 1024 * 1024;
const KEYS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Fetches the realm's signing keys at `url` and installs them, answering how many were usable.
///
/// The runtime reads the realm's public `https://idm.{domain}` address through the edge, the
/// way it already reaches the gateway, instead of jc's in-cluster listener: that listener admits
/// the edge alone, because jc trusts `X-Forwarded-For` from the pod network on that condition
/// (T-7774). jc-core's own refresh speaks plain http only, so this one uses the runtime's client.
pub async fn refresh_keys(
    verifier: &Verifier,
    http: &reqwest::Client,
    url: &str,
) -> Result<usize, String> {
    let fetch = async {
        let mut answer = http
            .get(url)
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| format!("the realm did not answer: {}", e.without_url()))?;
        if !answer.status().is_success() {
            return Err(format!("the realm answered {}", answer.status()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = answer
            .chunk()
            .await
            .map_err(|e| format!("the realm's answer broke off: {}", e.without_url()))?
        {
            if body.len() + chunk.len() > KEYS_LIMIT {
                return Err("the realm's key set is larger than 1 MiB".to_owned());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    };
    let body = tokio::time::timeout(KEYS_TIMEOUT, fetch)
        .await
        .map_err(|_| format!("the realm did not answer within {KEYS_TIMEOUT:?}"))??;
    let keys: jsonwebtoken::jwk::JwkSet = serde_json::from_slice(&body)
        .map_err(|e| format!("the realm's answer is not a key set: {e}"))?;
    match verifier.replace_keys(&keys) {
        0 => Err("the realm's key set holds no usable signing key".to_owned()),
        count => Ok(count),
    }
}

/// Keeps the realm's keys current: now, then every [`jc_core::auth::jwks::REFRESH`]. A failed
/// refresh keeps the keys already loaded.
pub async fn keep_keys(verifier: Arc<Verifier>, http: reqwest::Client, url: String) {
    loop {
        match refresh_keys(&verifier, &http, &url).await {
            Ok(count) => tracing::info!(count, "realm signing keys loaded"),
            Err(error) => {
                tracing::error!(%error, %url, "cannot refresh the realm signing keys")
            }
        }
        tokio::time::sleep(jc_core::auth::jwks::REFRESH).await;
    }
}

/// Reads and checks an invocation's body before any code runs: its shape, the request body's
/// size for its `via`, the method, what the assistant may ask for and the endpoint's slug. The
/// refusal is the problem the caller gets. `assistant` says the token was the assistant's.
pub fn prepare(
    state: &AppState,
    body: &[u8],
    assistant: bool,
) -> Result<Invocation, Box<Response>> {
    let call: InvokeRequest = match serde_json::from_slice(body) {
        Ok(call) => call,
        Err(error) => {
            return Err(Box::new(problem(
                StatusCode::BAD_REQUEST,
                error.to_string(),
            )))
        }
    };
    let limit = if call.via == Via::None {
        SCRIPT_BODY_LIMIT
    } else {
        REQUEST_BODY_LIMIT
    };
    if serde_json::to_vec(&call.request.body).map_or(true, |b| b.len() > limit) {
        return Err(Box::new(problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("the request body is larger than {} KiB", limit / 1024),
        )));
    }
    if !matches!(call.request.method.as_str(), "GET" | "POST") {
        return Err(Box::new(problem(
            StatusCode::BAD_REQUEST,
            "a function is called with GET or POST",
        )));
    }
    if assistant && call.via != Via::None {
        return Err(Box::new(problem(
            StatusCode::FORBIDDEN,
            "the assistant's scripts run with via: \"none\", no endpoint and no network",
        )));
    }
    let endpoint = match call.via {
        Via::None => None,
        via => {
            let Some(slug) = call
                .config
                .get("slug")
                .and_then(Value::as_str)
                .filter(|s| endpoint::is_slug(s))
            else {
                return Err(Box::new(problem(
                    StatusCode::BAD_REQUEST,
                    "config.slug must be the endpoint's slug",
                )));
            };
            let proxy = if via == Via::Proxy {
                match &state.proxy {
                Some(proxy) => Some(proxy.clone()),
                None => return Err(Box::new(problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "this runtime has no agent proxy address (JC_AGENT_PROXY_URL) for a run's own call",
                ))),
            }
            } else {
                None
            };
            Some(Endpoint {
                http: state.http.clone(),
                gateway: state.gateway.clone(),
                slug: slug.to_owned(),
                token: call.token.filter(|t| !t.is_empty()),
                proxy,
            })
        }
    };
    Ok(Invocation {
        endpoint,
        files: call.files,
        entry: call.entry,
        request: serde_json::to_value(&call.request).unwrap_or_default(),
        config: call.config,
    })
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/invoke", post(invoke))
        .route("/healthz", get(|| async { "ok" }))
        .layer(DefaultBodyLimit::max(INVOCATION_LIMIT))
        .with_state(state)
}
