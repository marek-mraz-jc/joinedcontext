//! The embedder service: `POST /v1/embeddings` for the deployments its config names, and
//! `GET /healthz` for probes.
//!
//! - **Who calls**: each deployment signs in with a client-credentials token its own kaisatsu
//!   signed for audience [`AUDIENCE`] (the jc door's check, [`jc_door::Verifier`]); the token's
//!   issuer picks the deployment. No other credential is taken.
//! - **How much**: per request at most `max_texts` texts of `max_text_bytes` each; per deployment
//!   `max_in_flight` requests at once (refused at once past it, never queued) and
//!   `max_texts_per_minute` texts. Every count is per deployment, so one deployment never
//!   spends another's.
//! - **How long**: reading the body and embedding it share `timeout_secs`. A caller that goes
//!   away or runs out of time drops its permits with its request; the model finishes the one text
//!   it is on and takes the next.
//! - **What stays**: nothing. No cache, no store; a text lives as long as its request, and no
//!   log line or error body carries one (a refusal names a text by its index).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use serde::Deserialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{AUDIENCE, EmbedAnswer, EmbedRequest, Embedder, Embedding, MODEL, MODEL_SHA256};

/// The service's file (`jc-embedder --config <file>` or `EMBEDDER_CONFIG`). Unknown keys are
/// refused, so a typo never keeps a default.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_listen")]
    pub listen_addr: SocketAddr,
    /// Prometheus `/metrics`, on a listener of its own; none when unset.
    pub metrics_addr: Option<SocketAddr>,
    /// The directory `scripts/ci/e5-small.sh` fills.
    pub model_path: PathBuf,
    /// ONNX Runtime threads, as `JC_ASSISTANT_EMBED_THREADS`: 1 to 16.
    #[serde(default = "default_threads")]
    pub threads: usize,
    #[serde(default)]
    pub limits: Limits,
    pub deployments: Vec<DeploymentConfig>,
}

fn default_listen() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 8089))
}

fn default_threads() -> usize {
    2
}

/// `[limits]`, the same for every deployment.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    /// Texts in one request.
    pub max_texts: usize,
    /// Bytes of one text.
    pub max_text_bytes: usize,
    /// Reading a request and embedding it.
    pub timeout_secs: u64,
    /// Requests of one deployment at once.
    pub max_in_flight: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_texts: 64,
            max_text_bytes: 8 * 1024,
            timeout_secs: 20,
            max_in_flight: 4,
        }
    }
}

/// `[[deployments]]`: one calling deployment.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentConfig {
    /// Its name in metrics and logs.
    pub name: String,
    /// Its kaisatsu's issuer (`iss`) and signing keys.
    pub issuer: String,
    pub jwks_url: String,
    /// The client ids (`azp`) it calls with.
    pub clients: Vec<String>,
    /// Texts it may embed per minute.
    pub max_texts_per_minute: u32,
}

impl Config {
    /// The file at `path`, checked.
    pub fn read(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("{} could not be read: {e}", path.display()))?;
        let cfg: Self =
            toml::from_str(&text).map_err(|e| format!("{} is not valid: {e}", path.display()))?;
        cfg.check()?;
        Ok(cfg)
    }

    pub fn check(&self) -> Result<(), String> {
        let l = &self.limits;
        if l.max_texts == 0 || l.max_text_bytes == 0 || l.timeout_secs == 0 || l.max_in_flight == 0
        {
            return Err("every [limits] value is at least 1".into());
        }
        if !(1..=16).contains(&self.threads) {
            return Err("threads is a number from 1 to 16".into());
        }
        if self.deployments.is_empty() {
            return Err("name at least one [[deployments]]: nobody else may call".into());
        }
        for (i, d) in self.deployments.iter().enumerate() {
            if d.name.trim().is_empty() || self.deployments[..i].iter().any(|o| o.name == d.name) {
                return Err(format!(
                    "deployment {i}: a name, unique among the deployments"
                ));
            }
            if d.clients.iter().all(|c| c.trim().is_empty()) {
                return Err(format!("deployment {}: name its clients", d.name));
            }
            if d.max_texts_per_minute == 0 {
                return Err(format!(
                    "deployment {}: max_texts_per_minute is at least 1",
                    d.name
                ));
            }
            if !(d.jwks_url.starts_with("https://") || d.jwks_url.starts_with("http://")) {
                return Err(format!("deployment {}: jwks_url is an http(s) URL", d.name));
            }
        }
        Ok(())
    }
}

/// One calling deployment and what it has used.
struct Deployment {
    name: String,
    verifier: jc_door::Verifier,
    per_minute: u32,
    /// The current minute's start and the texts charged in it.
    used: Mutex<(Instant, u32)>,
    in_flight: Arc<Semaphore>,
}

impl Deployment {
    /// Charges `texts` to the current minute, or the seconds until the next one.
    fn charge(&self, texts: u32) -> Result<(), u64> {
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if now.duration_since(used.0) >= Duration::from_secs(60) {
            *used = (now, 0);
        }
        if used.1.saturating_add(texts) > self.per_minute {
            let left = Duration::from_secs(60).saturating_sub(now.duration_since(used.0));
            return Err(left.as_secs().max(1));
        }
        used.1 += texts;
        Ok(())
    }
}

/// The service: the model and the deployments that may call it.
pub struct Service {
    embedder: Embedder,
    limits: Limits,
    deployments: Vec<Deployment>,
}

impl Service {
    pub fn new(embedder: Embedder, cfg: &Config) -> Result<Self, String> {
        cfg.check()?;
        let deployments = cfg
            .deployments
            .iter()
            .map(|d| {
                Ok(Deployment {
                    name: d.name.clone(),
                    verifier: jc_door::Verifier::new(&d.issuer, &d.jwks_url, &d.clients)?,
                    per_minute: d.max_texts_per_minute,
                    used: Mutex::new((Instant::now(), 0)),
                    in_flight: Arc::new(Semaphore::new(cfg.limits.max_in_flight)),
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            embedder,
            limits: cfg.limits.clone(),
            deployments,
        })
    }

    /// The deployment whose kaisatsu signed `token` for an admitted client and this audience.
    async fn caller(&self, token: &str) -> Option<&Deployment> {
        let issuer = unverified_issuer(token)?;
        for d in self
            .deployments
            .iter()
            .filter(|d| d.verifier.issuer() == issuer)
        {
            d.verifier.prepare(token).await;
            if d.verifier.client(token, AUDIENCE).is_some() {
                return Some(d);
            }
        }
        None
    }
}

/// The `iss` a token claims, read without checking it: it only picks whose keys check it.
fn unverified_issuer(token: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Iss {
        iss: String,
    }
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice::<Iss>(&bytes).ok().map(|c| c.iss)
}

/// The routes: `POST /v1/embeddings`, `GET /healthz`.
pub fn router(service: Arc<Service>) -> Router {
    Router::new()
        .route("/v1/embeddings", post(embeddings))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(service)
}

/// A refusal: a status and a fixed sentence, never a text of the request.
fn refuse(status: StatusCode, why: impl Into<String>) -> Response {
    (
        status,
        axum::Json(serde_json::json!({ "error": why.into() })),
    )
        .into_response()
}

fn refused(deployment: &str, reason: &'static str, status: StatusCode, why: String) -> Response {
    metrics::counter!("embedder_refusals_total", "deployment" => deployment.to_owned(), "reason" => reason)
        .increment(1);
    tracing::info!(deployment, reason, status = status.as_u16(), "refused");
    refuse(status, why)
}

async fn embeddings(State(service): State<Arc<Service>>, req: Request) -> Response {
    let Some(token) = jc_door::bearer(req.headers()).map(str::to_owned) else {
        let mut resp = refused(
            "unknown",
            "token",
            StatusCode::UNAUTHORIZED,
            "a bearer token of audience embedder is required".into(),
        );
        resp.headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return resp;
    };
    let Some(deployment) = service.caller(&token).await else {
        let mut resp = refused(
            "unknown",
            "token",
            StatusCode::UNAUTHORIZED,
            "the token is not one of a deployment this embedder serves".into(),
        );
        resp.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer error=\"invalid_token\""),
        );
        return resp;
    };
    let Ok(permit) = Arc::clone(&deployment.in_flight).try_acquire_owned() else {
        let mut resp = refused(
            &deployment.name,
            "busy",
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "this deployment has {} requests running already",
                service.limits.max_in_flight
            ),
        );
        resp.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        return resp;
    };
    let began = Instant::now();
    let timeout = Duration::from_secs(service.limits.timeout_secs);
    let resp = match tokio::time::timeout(timeout, serve(&service, deployment, permit, req)).await {
        Ok(resp) => resp,
        Err(_) => refused(
            &deployment.name,
            "timeout",
            StatusCode::REQUEST_TIMEOUT,
            format!("not done within {} s", service.limits.timeout_secs),
        ),
    };
    metrics::histogram!("embedder_request_seconds", "deployment" => deployment.name.clone())
        .record(began.elapsed().as_secs_f64());
    resp
}

/// One admitted request: holds `_permit` (its deployment's slot) until it is answered or
/// dropped.
async fn serve(
    service: &Service,
    deployment: &Deployment,
    _permit: OwnedSemaphorePermit,
    req: Request,
) -> Response {
    let limits = &service.limits;
    let name = deployment.name.as_str();
    // Room for every text escaped as JSON (at most 6 bytes per byte) and the envelope.
    let cap = limits
        .max_texts
        .saturating_mul(limits.max_text_bytes.saturating_mul(6).saturating_add(8))
        .saturating_add(1024);
    let Ok(body) = axum::body::to_bytes(req.into_body(), cap).await else {
        return refused(
            name,
            "size",
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("the body is larger than {cap} bytes or was cut off"),
        );
    };
    // serde_json quotes the offending value in its messages: none of it goes out.
    let Ok(request) = serde_json::from_slice::<EmbedRequest>(&body) else {
        return refused(
            name,
            "body",
            StatusCode::BAD_REQUEST,
            "the body is not {model, kind: query|passage, input: [text, ...]}".into(),
        );
    };
    drop(body);
    if request.model != MODEL {
        return refused(
            name,
            "model",
            StatusCode::BAD_REQUEST,
            format!("this embedder runs {MODEL} ({MODEL_SHA256}) only"),
        );
    }
    if request.input.is_empty() || request.input.len() > limits.max_texts {
        return refused(
            name,
            "size",
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("input holds 1 to {} texts", limits.max_texts),
        );
    }
    if let Some(at) = request
        .input
        .iter()
        .position(|t| t.len() > limits.max_text_bytes)
    {
        return refused(
            name,
            "size",
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("text {at} is longer than {} bytes", limits.max_text_bytes),
        );
    }
    let count = u32::try_from(request.input.len()).unwrap_or(u32::MAX);
    if let Err(wait) = deployment.charge(count) {
        let mut resp = refused(
            name,
            "quota",
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "this deployment may embed {} texts a minute",
                deployment.per_minute
            ),
        );
        if let Ok(value) = HeaderValue::from_str(&wait.to_string()) {
            resp.headers_mut().insert(header::RETRY_AFTER, value);
        }
        return resp;
    }
    let kind = request.kind;
    match service.embedder.embed(kind, &request.input).await {
        Ok(vectors) => {
            metrics::counter!("embedder_texts_total", "deployment" => name.to_owned(), "kind" => kind.as_str())
                .increment(u64::from(count));
            tracing::info!(
                deployment = name,
                kind = kind.as_str(),
                texts = count,
                "embedded"
            );
            axum::Json(EmbedAnswer {
                model: MODEL.to_owned(),
                model_sha256: MODEL_SHA256.to_owned(),
                data: vectors
                    .into_iter()
                    .enumerate()
                    .map(|(index, embedding)| Embedding { index, embedding })
                    .collect(),
            })
            .into_response()
        }
        Err(e) => {
            tracing::warn!(deployment = name, error = %e, "the model failed");
            refused(
                name,
                "model",
                StatusCode::SERVICE_UNAVAILABLE,
                "the model failed on this request".into(),
            )
        }
    }
}

#[cfg(test)]
mod tests;
