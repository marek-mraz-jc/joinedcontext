//! LLM model provider mediation and token budget tracking (/v1/llm/*).

use crate::audit::{log_request, AuditEntry};
use crate::auth::authenticate;
use crate::ProxyState;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};
use std::time::Instant;

/// The body with the run's reasoning setting in the shape the endpoint reads (AG-72): OpenRouter's
/// `reasoning.effort` on chat completions, a thinking budget on Anthropic messages. Either way a
/// `max_tokens` the body names grows by the budget so the answer keeps the room it asked for:
/// Gemini behind OpenRouter counts its thinking inside `max_tokens` too (T-7758). A body that
/// already carries a setting, is not a JSON object, or an effort outside the three is sent as is.
fn with_reasoning(body: &Bytes, rest: &str, effort: &str) -> Bytes {
    let Some(&(_, budget)) = BUDGETS.iter().find(|(name, _)| *name == effort) else {
        return body.clone();
    };
    let Ok(Value::Object(mut map)) = serde_json::from_slice::<Value>(body) else {
        return body.clone();
    };
    if rest.ends_with("messages") {
        if map.contains_key("thinking") {
            return body.clone();
        }
        let max_tokens = map.get("max_tokens").and_then(Value::as_u64).unwrap_or(0);
        map.insert(
            "max_tokens".into(),
            json!(max_tokens.saturating_add(budget)),
        );
        map.insert(
            "thinking".into(),
            json!({ "type": "enabled", "budget_tokens": budget }),
        );
    } else {
        if map.contains_key("reasoning") || map.contains_key("reasoning_effort") {
            return body.clone();
        }
        map.insert("reasoning".into(), json!({ "effort": effort }));
        if let Some(max_tokens) = map.get("max_tokens").and_then(Value::as_u64) {
            map.insert(
                "max_tokens".into(),
                json!(max_tokens.saturating_add(budget)),
            );
        }
    }
    Bytes::from(Value::Object(map).to_string())
}

/// The body with `model` set to the run's profile model (F-PLT1-006): the profile's model and
/// its cost hold whatever model the workspace names, and a harness that names a side model of its
/// own (titles, summaries) keeps working on the run's. A body that is not a JSON object, or a
/// profile naming no model, is sent as is.
fn with_model(body: Bytes, model: &str) -> Bytes {
    if model.is_empty() {
        return body;
    }
    let Ok(Value::Object(mut map)) = serde_json::from_slice::<Value>(&body) else {
        return body;
    };
    if map.get("model").and_then(Value::as_str) == Some(model) {
        return body;
    }
    map.insert("model".into(), json!(model));
    Bytes::from(Value::Object(map).to_string())
}

/// The body with its output capped at `left`, what remains of the run's token budget
/// (F-R2PLT1-006): checking the budget before the call alone let one call asking a large
/// `max_tokens` spend far past it. Each output field the body names (`max_tokens`, OpenAI's
/// `max_completion_tokens`) is lowered to `left`; a body naming none gets `max_tokens: left` when
/// that is below the output a stream is otherwise counted for, the one size every model accepts.
/// A body that is not a JSON object is sent as is.
fn within_budget(body: Bytes, left: u64) -> Bytes {
    let Ok(Value::Object(mut map)) = serde_json::from_slice::<Value>(&body) else {
        return body;
    };
    let mut named = false;
    let mut lowered = false;
    for field in ["max_tokens", "max_completion_tokens"] {
        if let Some(asked) = map.get(field).and_then(Value::as_u64) {
            named = true;
            if asked > left {
                map.insert(field.into(), json!(left));
                lowered = true;
            }
        }
    }
    if !named && left < STREAM_OUTPUT_UNSAID {
        map.insert("max_tokens".into(), json!(left));
        lowered = true;
    }
    if !lowered {
        return body;
    }
    Bytes::from(Value::Object(map).to_string())
}

/// Why the body's OpenRouter `provider` field is refused, or `None` when it is absent or holds
/// only `order` (1 to 8 provider slugs), `sort` (`latency`, `throughput` or `price`; T-4360), at
/// least one of the two, and `allow_fallbacks` (a boolean); it is sent as is otherwise (T-4292,
/// MF-52). A body that is not a JSON object is left to the provider.
fn provider_refusal(body: &Bytes) -> Option<&'static str> {
    use jc_core::kinds::assistant::{is_provider_slug, MAX_PROVIDERS};
    let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(body) else {
        return None;
    };
    let provider = map.get("provider")?;
    let Some(fields) = provider.as_object() else {
        return Some("provider is an object of order, sort and allow_fallbacks");
    };
    if fields
        .keys()
        .any(|k| !["order", "sort", "allow_fallbacks"].contains(&k.as_str()))
    {
        return Some("provider takes only order, sort and allow_fallbacks");
    }
    if !fields.contains_key("order") && !fields.contains_key("sort") {
        return Some("provider names an order, a sort or both");
    }
    let slugs_ok = fields.get("order").is_none_or(|order| {
        order.as_array().is_some_and(|o| {
            (1..=MAX_PROVIDERS).contains(&o.len())
                && o.iter().all(|s| s.as_str().is_some_and(is_provider_slug))
        })
    });
    if !slugs_ok {
        return Some("provider.order names 1 to 8 provider slugs of a-z 0-9 . _ / -");
    }
    if fields.get("sort").is_some_and(|sort| {
        !sort
            .as_str()
            .is_some_and(|s| ["latency", "throughput", "price"].contains(&s))
    }) {
        return Some("provider.sort is latency, throughput or price");
    }
    if fields
        .get("allow_fallbacks")
        .is_some_and(|v| !v.is_boolean())
    {
        return Some("provider.allow_fallbacks is true or false");
    }
    None
}

/// The body for this upstream: OpenRouter reads `provider`, its routing, and another upstream may
/// refuse it as an unknown field (AG-72). A body naming none is sent as is.
fn routed(body: Bytes, routes: bool) -> Bytes {
    if routes {
        return body;
    }
    let Ok(Value::Object(mut map)) = serde_json::from_slice::<Value>(&body) else {
        return body;
    };
    if map.remove("provider").is_none() {
        return body;
    }
    Bytes::from(Value::Object(map).to_string())
}

/// Each reasoning effort and the thinking budget it is on Anthropic messages (AG-72).
const BUDGETS: [(&str, u64); 3] = [("low", 2048), ("medium", 8192), ("high", 24576)];

/// Whether the call asks for a stream (`"stream": true`, ADR-N-032).
fn wants_stream(body: &Bytes) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("stream").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// A streamed chat completion that asks for the usage chunk, so the proxy can count the call:
/// OpenAI-compatible providers send it only when `stream_options.include_usage` is set. A body
/// that already says, or Anthropic's messages (whose stream always carries usage), is left alone.
fn with_stream_usage(body: &Bytes, rest: &str) -> Bytes {
    if rest.ends_with("messages") {
        return body.clone();
    }
    let Ok(Value::Object(mut map)) = serde_json::from_slice::<Value>(body) else {
        return body.clone();
    };
    let options = map.entry("stream_options").or_insert_with(|| json!({}));
    let Value::Object(options) = options else {
        return body.clone();
    };
    options.entry("include_usage").or_insert(json!(true));
    Bytes::from(Value::Object(map).to_string())
}

/// What a stream that reports no usage is counted as: the output it was allowed and a quarter of
/// its body's bytes for the input. Never nothing: the budget is a cost control (AG-41).
fn uncounted_stream(body: &Bytes) -> u64 {
    let allowed = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("max_tokens").and_then(Value::as_u64))
        .unwrap_or(STREAM_OUTPUT_UNSAID);
    allowed.saturating_add(body.len() as u64 / 4)
}

/// The output a stream is counted for when its request names no `max_tokens`.
const STREAM_OUTPUT_UNSAID: u64 = 8192;

/// The longest line of a stream the usage reader keeps; a longer one is dropped unread.
const STREAM_LINE_MAX: usize = 1 << 20;

/// The usage a provider reports inside its stream, read line by line as the chunks pass.
#[derive(Default)]
struct StreamUsage {
    line: Vec<u8>,
    total: Option<u64>,
    input: u64,
    output: u64,
    cached: u64,
    /// The call's cost, when the provider reports one (OpenRouter's `usage.cost`, AG-97).
    cost: Option<f64>,
}

/// The counts of one `usage` object, OpenAI-compatible or Anthropic: the total when it is given,
/// the input, the output, and the input read from the provider's prompt cache
/// (`prompt_tokens_details.cached_tokens`, or Anthropic's `cache_read_input_tokens`).
fn counts(usage: &Value) -> (Option<u64>, u64, u64, u64) {
    let count = |keys: &[&str]| keys.iter().find_map(|key| usage.get(*key)?.as_u64());
    let cached = usage
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_u64)
        .or_else(|| count(&["cache_read_input_tokens"]))
        .unwrap_or(0);
    (
        count(&["total_tokens"]),
        count(&["prompt_tokens", "input_tokens"]).unwrap_or(0),
        count(&["completion_tokens", "output_tokens"]).unwrap_or(0),
        cached,
    )
}

/// One call as the Portal's `usage` frame carries it (API/04 §4, AG-72, T-2771).
#[derive(Debug, Default, PartialEq)]
struct CallUsage {
    tokens: u64,
    input: u64,
    output: u64,
    cached: u64,
    latency_ms: u64,
    /// The `model` the call named, cut to [`MODEL_NAME_MAX`] characters.
    model: Option<String>,
    /// The reasoning effort the call was sent with (T-2998), one of [`BUDGETS`].
    effort: Option<&'static str>,
    /// What the provider says the call cost (AG-97).
    cost: Option<f64>,
}

/// The most of a call's `model` a usage frame repeats; the body is the run's to write.
const MODEL_NAME_MAX: usize = 128;

impl CallUsage {
    fn payload(&self) -> Value {
        let mut payload = json!({
            "tokensThisStep": self.tokens,
            "inputTokens": self.input,
            "outputTokens": self.output,
            "cachedTokens": self.cached,
            "latencyMs": self.latency_ms,
        });
        if let Some(model) = &self.model {
            payload["model"] = json!(model);
        }
        if let Some(effort) = self.effort {
            payload["reasoningEffort"] = json!(effort);
        }
        if let Some(cost) = self.cost.filter(|cost| cost.is_finite() && *cost >= 0.0) {
            payload["costUsd"] = json!(cost);
        }
        payload
    }
}

/// The reasoning effort the body as sent asks for (T-2998): the run's setting the proxy wrote, or
/// the one the body carried itself, which the proxy leaves as it is. OpenRouter names it
/// (`reasoning.effort`, `reasoning_effort`), Anthropic's messages give its thinking budget. Only
/// one of [`BUDGETS`] is reported, so a frame never repeats what a body made up.
fn effort_of(body: &Bytes) -> Option<&'static str> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let named = value
        .pointer("/reasoning/effort")
        .or_else(|| value.get("reasoning_effort"))
        .and_then(Value::as_str);
    let budget = value
        .pointer("/thinking/budget_tokens")
        .and_then(Value::as_u64);
    BUDGETS
        .iter()
        .find(|(name, tokens)| named == Some(*name) || budget == Some(*tokens))
        .map(|(name, _)| *name)
}

/// The `model` a call's body names, for its usage frame.
fn model_of(body: &Bytes) -> Option<String> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let model = value.get("model")?.as_str()?;
    Some(model.chars().take(MODEL_NAME_MAX).collect())
}

fn millis(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

impl StreamUsage {
    fn feed(&mut self, chunk: &[u8]) {
        for &byte in chunk {
            if byte == b'\n' {
                let line = std::mem::take(&mut self.line);
                self.read_line(&line);
            } else if self.line.len() < STREAM_LINE_MAX {
                self.line.push(byte);
            }
        }
    }

    fn read_line(&mut self, line: &[u8]) {
        let Some(data) = std::str::from_utf8(line)
            .ok()
            .and_then(|line| line.trim().strip_prefix("data:"))
        else {
            return;
        };
        let Ok(value) = serde_json::from_str::<Value>(data.trim()) else {
            return;
        };
        // OpenAI-compatible: `usage` on the last chunk. Anthropic: `message.usage` on
        // `message_start` (the input), `usage` on `message_delta` (the output so far).
        let Some(usage) = value
            .get("usage")
            .or_else(|| value.pointer("/message/usage"))
        else {
            return;
        };
        let (total, input, output, cached) = counts(usage);
        if total.is_some() {
            self.total = total;
        }
        self.input = self.input.max(input);
        self.output = self.output.max(output);
        self.cached = self.cached.max(cached);
        if let Some(cost) = usage.get("cost").and_then(Value::as_f64) {
            self.cost = Some(cost);
        }
    }

    fn tokens(&self) -> Option<u64> {
        self.total
            .or_else(|| (self.input + self.output > 0).then_some(self.input + self.output))
    }
}

/// Whose model call it is: a run's, or the knowledge assistant's for one `AssistantDeployment`
/// (AG-109).
#[derive(Clone)]
enum Caller {
    Run(std::sync::Arc<crate::runs::RunContext>),
    Assistant {
        /// `{project}/{deployment}`, the key its day is counted under (AG-110).
        deployment: String,
    },
    /// An App's `ai` service, sent by the Portal, which counts the App's day itself (AP-169).
    App {
        /// `{project}/{app}`, what the audit line names.
        app: String,
    },
}

impl Caller {
    /// What the audit line names: the run, or the deployment (AG-111).
    fn audit_id(&self) -> &str {
        match self {
            Caller::Run(run) => &run.id,
            Caller::Assistant { deployment } => deployment,
            Caller::App { app } => app,
        }
    }

    fn audit_user(&self) -> &str {
        match self {
            Caller::Run(run) => &run.created_by,
            Caller::Assistant { .. } => "jc-assistant",
            Caller::App { .. } => "jc-portal-app",
        }
    }
}

/// The tokens of one call: a run's are counted against the run and reported to the Portal with
/// their halves, their cached input, their latency and their model (AG-41, AG-72); an
/// assistant's against its deployment's day (AG-110), and reported nowhere else.
fn record_usage(
    state: &ProxyState,
    caller: &Caller,
    usage: CallUsage,
) -> impl std::future::Future<Output = ()> {
    let state = state.clone();
    let caller = caller.clone();
    async move {
        if usage.tokens == 0 {
            return;
        }
        let run = match caller {
            Caller::Assistant { deployment } => {
                state
                    .limits
                    .record_deployment_day(&deployment, usage.tokens)
                    .await;
                return;
            }
            // The Portal reads the usage from the answer and counts it against the App's day.
            Caller::App { .. } => return,
            Caller::Run(run) => run,
        };
        let run_id = run.id.clone();
        state.limits.record_tokens(&run_id, usage.tokens).await;
        state
            .limits
            .record_daily(&run.kind, &run.created_by, usage.tokens)
            .await;
        let payload = usage.payload();
        let portal_base = state.config.portal_base.clone();
        let http = state.http.clone();
        let credentials = state.credentials.clone();
        tokio::spawn(async move {
            // The report travels on the same identity as every other callback, minted in the
            // spawned task so the answer to the run is never held up for the realm. A realm
            // that cannot be reached costs this report and nothing else.
            let Ok(bearer) = credentials.get_portal_token().await else {
                tracing::warn!(run = %run_id, "no token to report usage with");
                return;
            };
            let mut url = portal_base;
            url.set_path("internal/agent-runs/events");
            let _ = http
                .post(url)
                .bearer_auth(bearer)
                .json(&serde_json::json!({
                    "runId": run_id,
                    "kind": "usage",
                    "payload": payload
                }))
                .send()
                .await;
        });
    }
}

/// The header naming the `AssistantDeployment` an assistant call is for, `{project}/{name}`.
pub const ASSISTANT_DEPLOYMENT: &str = "x-jc-assistant-deployment";
/// The header carrying that deployment's `budget.tokensPerDay` (AG-110).
pub const ASSISTANT_TOKENS_PER_DAY: &str = "x-jc-assistant-tokens-per-day";
/// The header naming the App an `ai` service call is for, `{project}/{app}` (AP-169).
pub const APP: &str = "x-jc-app";

/// A DNS label, which a project's and a deployment's `metadata.name` are.
fn is_label(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// A refusal answered before the provider is asked; boxed, it is a pointer on the happy path.
type Refusal = Box<Response>;

/// The knowledge assistant's call: its service account, the deployment it names and that
/// deployment's day, all checked before the provider is asked (AG-109, AG-110).
async fn assistant_caller(state: &ProxyState, headers: &HeaderMap) -> Result<Caller, Refusal> {
    let refused = || {
        Box::new(
            jc_core::ProblemDetails::unauthorized()
                .with_detail(
                    "only the knowledge assistant's service account calls the model for a deployment",
                )
                .into_response(),
        )
    };
    let Some(bearer) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| jc_core::http::bearer(v.to_str().ok()))
    else {
        return Err(refused());
    };
    match state.credentials.grants().is_assistant(bearer).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!("an assistant model call not from the assistant's service account");
            return Err(refused());
        }
        Err(error) => {
            tracing::error!(%error, "the realm could not judge an assistant model call");
            return Err(Box::new(
                jc_core::ProblemDetails::new(503, "service-unavailable", "Service Unavailable")
                    .with_detail("the realm could not be reached; try again shortly")
                    .into_response(),
            ));
        }
    }
    let bad = |detail: &str| {
        Box::new(
            jc_core::ProblemDetails::new(400, "bad-request", "Bad Request")
                .with_detail(detail.to_owned())
                .into_response(),
        )
    };
    let deployment = headers
        .get(ASSISTANT_DEPLOYMENT)
        .and_then(|v| v.to_str().ok())
        .filter(|v| {
            v.split_once('/')
                .is_some_and(|(project, name)| is_label(project) && is_label(name))
        })
        .ok_or_else(|| bad("X-JC-Assistant-Deployment names the deployment as {project}/{name}"))?
        .to_owned();
    let cap = headers
        .get(ASSISTANT_TOKENS_PER_DAY)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|cap| *cap > 0)
        .ok_or_else(|| {
            bad("X-JC-Assistant-Tokens-Per-Day carries the deployment's budget.tokensPerDay, a whole number above 0")
        })?;
    if let Err(detail) = state.limits.check_deployment_day(&deployment, cap).await {
        return Err(Box::new(
            (
                StatusCode::TOO_MANY_REQUESTS,
                jc_core::ProblemDetails::new(429, "daily-budget", "Daily Budget Spent")
                    .with_detail(detail),
            )
                .into_response(),
        ));
    }
    Ok(Caller::Assistant { deployment })
}

/// An App's `ai` call (AP-169): the Portal's own service account, never an App's or a person's
/// token, naming the App it calls for. The Portal has checked the App's layers and its day before.
async fn app_caller(state: &ProxyState, headers: &HeaderMap) -> Result<Caller, Refusal> {
    let refused = || {
        Box::new(
            jc_core::ProblemDetails::unauthorized()
                .with_detail("only the Portal's service account calls the model for an App")
                .into_response(),
        )
    };
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| jc_core::http::bearer(v.to_str().ok()))
        .ok_or_else(refused)?;
    match state.credentials.grants().is_portal(bearer).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!("an App's model call not from the Portal's service account");
            return Err(refused());
        }
        Err(error) => {
            tracing::error!(%error, "the realm could not judge an App's model call");
            return Err(Box::new(
                jc_core::ProblemDetails::new(503, "service-unavailable", "Service Unavailable")
                    .with_detail("the realm could not be reached; try again shortly")
                    .into_response(),
            ));
        }
    }
    let app = headers
        .get(APP)
        .and_then(|v| v.to_str().ok())
        .filter(|v| {
            v.split_once('/')
                .is_some_and(|(project, name)| is_label(project) && is_label(name))
        })
        .ok_or_else(|| {
            Box::new(
                jc_core::ProblemDetails::new(400, "bad-request", "Bad Request")
                    .with_detail("X-JC-App names the App as {project}/{name}")
                    .into_response(),
            )
        })?
        .to_owned();
    Ok(Caller::App { app })
}

/// A run's call, authenticated before the path is judged, as it always was: a caller without a
/// ticket learns nothing about which paths exist.
async fn run_caller_or_path(
    state: &ProxyState,
    headers: &HeaderMap,
    completion: bool,
) -> Result<Caller, Refusal> {
    let run = authenticate(headers, &state.runs, &state.config)
        .await
        .map_err(|p| Box::new((*p).into_response()))?;
    if !completion {
        return Err(Box::new(
            jc_core::ProblemDetails::forbidden()
                .with_detail("only chat/messages completion endpoints permitted")
                .into_response(),
        ));
    }
    run_limits(state, run).await
}

/// A run's steps, its tokens and the day's budgets of its consumer and its person, before the
/// provider is asked (AG-41, AG-51, AG-97).
async fn run_limits(
    state: &ProxyState,
    run: std::sync::Arc<crate::runs::RunContext>,
) -> Result<Caller, Refusal> {
    let too_many = |msg: &str| {
        Box::new(
            (
                StatusCode::TOO_MANY_REQUESTS,
                jc_core::ProblemDetails::new(429, "too-many-requests", msg),
            )
                .into_response(),
        )
    };
    // One model call is one step (AG-51): the run stops here however its driver loops.
    state
        .limits
        .check_steps(&run.id, run.steps_per_run)
        .await
        .map_err(too_many)?;
    state
        .limits
        .check_tokens(&run.id, run.max_tokens)
        .await
        .map_err(too_many)?;
    // The day's budgets of the run's consumer and its person (AG-97), before the provider is
    // asked: a capped call costs nothing.
    if let Err(detail) = state
        .limits
        .check_daily(&state.config.daily_caps, &run.kind, &run.created_by)
        .await
    {
        return Err(Box::new(
            (
                StatusCode::TOO_MANY_REQUESTS,
                jc_core::ProblemDetails::new(429, "daily-budget", "Daily Budget Spent")
                    .with_detail(detail),
            )
                .into_response(),
        ));
    }
    Ok(Caller::Run(run))
}

/// A run's call capped at what is left of its token budget; an assistant or App call as sent.
async fn within_runs_budget(state: &ProxyState, caller: &Caller, body: Bytes) -> Bytes {
    match caller {
        Caller::Run(run) => within_budget(
            body,
            state.limits.tokens_left(&run.id, run.max_tokens).await,
        ),
        Caller::Assistant { .. } | Caller::App { .. } => body,
    }
}

/// Who calls: the assistant or an App (each at the completion endpoints alone, refused before the
/// realm is asked about its token, AG-111), else a run, authenticated first so a refused path
/// tells nobody without a ticket anything.
async fn caller_of(
    state: &ProxyState,
    headers: &HeaderMap,
    completion: bool,
) -> Result<Caller, Refusal> {
    let door = headers.contains_key(ASSISTANT_DEPLOYMENT) || headers.contains_key(APP);
    if door && !completion {
        return Err(Box::new(
            jc_core::ProblemDetails::forbidden()
                .with_detail("only chat/messages completion endpoints permitted")
                .into_response(),
        ));
    }
    if headers.contains_key(ASSISTANT_DEPLOYMENT) {
        assistant_caller(state, headers).await
    } else if headers.contains_key(APP) {
        app_caller(state, headers).await
    } else {
        run_caller_or_path(state, headers, completion).await
    }
}

pub async fn handler(
    State(state): State<ProxyState>,
    method: Method,
    headers: HeaderMap,
    Path(rest): Path<String>,
    req: Request<Body>,
) -> Response {
    let start = Instant::now();
    let completion = matches!(
        rest.as_str(),
        "v1/chat/completions" | "v1/messages" | "chat/completions" | "messages"
    );
    let caller = match caller_of(&state, &headers, completion).await {
        Ok(caller) => caller,
        Err(refusal) => return *refusal,
    };

    let body_bytes = match super::body::bounded(req.into_body()).await {
        Ok(bytes) => bytes,
        Err(refusal) => return *refusal,
    };
    if let Some(why) = provider_refusal(&body_bytes) {
        return jc_core::ProblemDetails::bad_request()
            .with_detail(why)
            .into_response();
    }
    let body_bytes = routed(body_bytes, state.config.model_routes);

    let body_bytes = match &caller {
        Caller::Run(run) => with_model(body_bytes, &run.model_name),
        Caller::Assistant { .. } | Caller::App { .. } => body_bytes,
    };
    let effort_setting = match &caller {
        Caller::Run(run) => run.reasoning_effort.as_deref(),
        Caller::Assistant { .. } | Caller::App { .. } => None,
    };
    let body_bytes = match effort_setting {
        Some(effort) => with_reasoning(&body_bytes, &rest, effort),
        None => body_bytes,
    };
    let body_bytes = within_runs_budget(&state, &caller, body_bytes).await;
    let streamed = wants_stream(&body_bytes);
    let body_bytes = if streamed {
        with_stream_usage(&body_bytes, &rest)
    } else {
        body_bytes
    };
    let uncounted = uncounted_stream(&body_bytes);
    let model = model_of(&body_bytes);
    let effort = effort_of(&body_bytes);

    let target_url = format!(
        "{}/{}",
        state.config.model_base.as_str().trim_end_matches('/'),
        rest.trim_start_matches('/')
    );

    let key = state.credentials.get_model_key();
    let mut client_req = state.http.request(method.clone(), &target_url);

    if state.config.model_provider == "anthropic" {
        client_req = client_req
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");
    } else {
        client_req = client_req
            .bearer_auth(key)
            .header("content-type", "application/json");
    }

    client_req = client_req.body(body_bytes);

    let sent = Instant::now();
    let upstream_resp = match client_req.send().await {
        Ok(r) => r,
        Err(e) => return super::upstream_unavailable(super::MODEL, &e),
    };

    let status =
        StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if let Some(refusal) =
        super::refused_redirect(status, super::location_of(&upstream_resp), "model provider")
    {
        return refusal;
    }
    // A key the provider does not take, or one without credit, is the installation's problem and
    // not the run's: the Portal hears of it now, not at the next probe (AG-96).
    crate::model_key::refused(&state, status.as_u16());
    if streamed && status.is_success() {
        let call = Streamed {
            caller,
            method,
            rest,
            uncounted,
            start,
            sent,
            model,
            effort,
        };
        return stream_through(state, call, status, upstream_resp);
    }
    // Past the cap the run gets the refusal, and the audit line says 502 like any other outage.
    let (resp_bytes, refusal) =
        match super::upstream_body(super::MODEL, upstream_resp, super::MODEL_ANSWER_BYTES).await {
            Ok(bytes) => (bytes, None),
            Err(refusal) => (Vec::new(), Some(*refusal)),
        };

    let latency_ms = millis(sent);
    if let Some(usage) = serde_json::from_slice::<Value>(&resp_bytes)
        .ok()
        .and_then(|answer| answer.get("usage").cloned())
    {
        let (total, input, output, cached) = counts(&usage);
        let usage = CallUsage {
            tokens: total.unwrap_or(input + output),
            input,
            output,
            cached,
            latency_ms,
            model,
            effort,
            cost: usage.get("cost").and_then(Value::as_f64),
        };
        record_usage(&state, &caller, usage).await;
    }

    log_request(&AuditEntry {
        run_id: caller.audit_id(),
        user: caller.audit_user(),
        upstream: "model-provider",
        method: method.as_str(),
        path: &rest,
        status: if refusal.is_some() {
            502
        } else {
            status.as_u16()
        },
        bytes: resp_bytes.len(),
        duration_ms: start.elapsed().as_millis(),
    });
    if let Some(refusal) = refusal {
        return refusal;
    }

    (
        status,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        Body::from(resp_bytes),
    )
        .into_response()
}

/// One streamed call: whose it is, what it asked and what it counts as without a usage.
struct Streamed {
    caller: Caller,
    method: Method,
    rest: String,
    uncounted: u64,
    start: Instant,
    /// When the call left for the provider: a usage frame's latency is counted from here.
    sent: Instant,
    model: Option<String>,
    effort: Option<&'static str>,
}

/// A stream passed through chunk by chunk (ADR-N-032). Its usage is read as it passes and
/// counted before the stream closes, so a caller that read to the end finds the call charged; a
/// stream without usage, or one the caller leaves, is counted as `uncounted` and the provider's
/// request is dropped with it.
/// How long a streamed answer may stay silent: providers send a keep-alive within seconds
/// (OpenRouter's comment lines, Anthropic's `ping`), so this is a provider that hung.
const STREAM_IDLE: std::time::Duration = std::time::Duration::from_secs(120);

fn stream_through(
    state: ProxyState,
    call: Streamed,
    status: StatusCode,
    upstream: reqwest::Response,
) -> Response {
    let Streamed {
        caller,
        method,
        rest,
        uncounted,
        start,
        sent,
        model,
        effort,
    } = call;
    use futures::StreamExt;
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);
    tokio::spawn(async move {
        let mut chunks = upstream.bytes_stream();
        let mut usage = StreamUsage::default();
        let mut bytes = 0usize;
        let mut whole = true;
        loop {
            // A caller gone or a provider silent past STREAM_IDLE ends the relay: neither the
            // task nor the upstream socket outlives the call (the client has no total timeout).
            // The caller is asked first, so a send that found it gone ends the relay here.
            let next = tokio::select! {
                biased;
                () = tx.closed() => {
                    whole = false;
                    break;
                }
                next = tokio::time::timeout(STREAM_IDLE, chunks.next()) => next,
            };
            let Ok(next) = next else {
                whole = false;
                let _ = tx
                    .send(Err(std::io::Error::other(
                        "the model provider sent nothing for 120 s",
                    )))
                    .await;
                break;
            };
            let Some(chunk) = next else {
                break;
            };
            match chunk {
                Ok(chunk) => {
                    bytes += chunk.len();
                    usage.feed(&chunk);
                    let _ = tx.send(Ok(chunk)).await;
                }
                Err(err) => {
                    whole = false;
                    let _ = tx.send(Err(std::io::Error::other(err))).await;
                    break;
                }
            }
        }
        let tokens = match usage.tokens() {
            Some(tokens) if whole => tokens,
            seen => seen.unwrap_or(0).max(uncounted),
        };
        let call = CallUsage {
            tokens,
            input: usage.input,
            output: usage.output,
            cached: usage.cached,
            latency_ms: millis(sent),
            model,
            effort,
            cost: usage.cost,
        };
        record_usage(&state, &caller, call).await;
        log_request(&AuditEntry {
            run_id: caller.audit_id(),
            user: caller.audit_user(),
            upstream: "model-provider",
            method: method.as_str(),
            path: &rest,
            status: status.as_u16(),
            bytes,
            duration_ms: start.elapsed().as_millis(),
        });
        drop(tx);
    });
    let body = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|chunk| (chunk, rx))
    });
    (
        status,
        [
            (axum::http::header::CONTENT_TYPE, "text/event-stream"),
            (axum::http::header::CACHE_CONTROL, "no-cache"),
        ],
        Body::from_stream(body),
    )
        .into_response()
}

/// The body rewrites and the stream reader over any bytes, for the fuzz target `llm_body`
/// (T-7795). Compiled only with the `fuzz` feature, which no deployment turns on.
#[cfg(feature = "fuzz")]
#[doc(hidden)]
pub mod fuzzing {
    use super::*;

    /// Every rewrite of one body, each checked for what it promises.
    pub fn body(body: &[u8], rest: &str, effort: &str, model: &str, left: u64, routes: bool) {
        let bytes = Bytes::copy_from_slice(body);
        let object = matches!(serde_json::from_slice::<Value>(body), Ok(Value::Object(_)));
        let _ = provider_refusal(&bytes);
        let sent = routed(bytes.clone(), routes);
        if !object || routes {
            assert_eq!(
                sent, bytes,
                "only an object loses its provider, and only off OpenRouter"
            );
        }
        let sent = with_model(bytes.clone(), model);
        if object && !model.is_empty() {
            let named: Value = serde_json::from_slice(&sent).expect("an object stays JSON");
            assert_eq!(
                named["model"],
                json!(model),
                "the run's model is the one sent"
            );
        } else {
            assert_eq!(sent, bytes);
        }
        let _ = with_reasoning(&bytes, rest, effort);
        let capped = within_budget(bytes.clone(), left);
        if let (true, Ok(Value::Object(map))) = (object, serde_json::from_slice::<Value>(&capped)) {
            for field in ["max_tokens", "max_completion_tokens"] {
                if let Some(asked) = map.get(field).and_then(Value::as_u64) {
                    assert!(asked <= left, "{field} {asked} past the {left} left");
                }
            }
        }
        let _ = wants_stream(&bytes);
        let _ = with_stream_usage(&bytes, rest);
        assert!(uncounted_stream(&bytes) >= bytes.len() as u64 / 4);
        let _ = effort_of(&bytes);
        if let Some(model) = model_of(&bytes) {
            assert!(model.chars().count() <= MODEL_NAME_MAX);
        }
    }

    /// A provider's stream fed in chunks of `split` bytes: what it counts never shrinks, and
    /// no line past the cap is held.
    pub fn stream(chunks: &[u8], split: usize) {
        let mut usage = StreamUsage::default();
        let mut last = (0, 0, 0);
        for chunk in chunks.chunks(split.max(1)) {
            usage.feed(chunk);
            assert!(usage.line.len() <= STREAM_LINE_MAX);
            let now = (usage.input, usage.output, usage.cached);
            assert!(now.0 >= last.0 && now.1 >= last.1 && now.2 >= last.2);
            last = now;
        }
        let _ = CallUsage {
            tokens: usage.tokens().unwrap_or(0),
            input: usage.input,
            output: usage.output,
            cached: usage.cached,
            latency_ms: 0,
            model: None,
            effort: None,
            cost: usage.cost,
        }
        .payload();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runs_model_replaces_the_one_the_body_names() {
        let sent = |body: &str, model: &str| {
            String::from_utf8(with_model(Bytes::from(body.to_owned()), model).to_vec())
                .unwrap_or_default()
        };
        let replaced: Value =
            serde_json::from_str(&sent(r#"{"model":"openai/o3-pro","messages":[]}"#, "m/run"))
                .unwrap_or_default();
        assert_eq!(replaced["model"], "m/run");
        assert_eq!(replaced["messages"], json!([]));
        let added: Value = serde_json::from_str(&sent("{}", "m/run")).unwrap_or_default();
        assert_eq!(added["model"], "m/run");
        assert_eq!(sent("not json", "m/run"), "not json");
        assert_eq!(sent(r#"{"model":"x"}"#, ""), r#"{"model":"x"}"#);
    }

    /// F-R2PLT1-006: a cap inside the budget is kept byte for byte, one past it is lowered.
    #[test]
    fn an_output_cap_inside_the_budget_is_kept_and_one_past_it_is_lowered() {
        let inside = Bytes::from_static(br#"{"model":"m","max_tokens":5}"#);
        assert_eq!(within_budget(inside.clone(), 10), inside);
        let large = Bytes::from_static(br#"{"model":"m"}"#);
        assert_eq!(within_budget(large.clone(), 100_000), large);
        let past: Value = serde_json::from_slice(&within_budget(
            Bytes::from_static(br#"{"max_tokens":50}"#),
            7,
        ))
        .expect("JSON");
        assert_eq!(past["max_tokens"], 7);
        // A cap equal to what is left is inside it; a body naming none at exactly the unsaid
        // output is sent as it came (T-7795).
        let equal = Bytes::from_static(br#"{ "model": "m", "max_tokens": 10 }"#);
        assert_eq!(within_budget(equal.clone(), 10), equal);
        let unsaid = Bytes::from_static(br#"{ "model": "m" }"#);
        assert_eq!(within_budget(unsaid.clone(), STREAM_OUTPUT_UNSAID), unsaid);
    }

    fn sent(body: &str, rest: &str, effort: &str) -> Value {
        serde_json::from_slice(&with_reasoning(&Bytes::from(body.to_owned()), rest, effort))
            .unwrap()
    }

    /// T-4292: OpenRouter's `provider` passes as written; a field or slug out of bounds is refused
    /// before the provider is asked, and a body without one is not looked at. A sort ranks the
    /// providers, with or without an order, and only OpenRouter's three are passed (T-4360).
    #[test]
    fn a_provider_field_passes_only_in_its_bounds() {
        let refusal = |body: &str| provider_refusal(&Bytes::from(body.to_owned()));
        for ok in [
            r#"{"model":"a/b","messages":[]}"#,
            r#"{"provider":{"order":["cerebras"]}}"#,
            r#"{"provider":{"order":["groq","deepinfra/fp8"],"allow_fallbacks":false}}"#,
            r#"{"provider":{"sort":"latency","allow_fallbacks":true}}"#,
            r#"{"provider":{"order":["groq"],"sort":"price"}}"#,
            "not json",
        ] {
            assert_eq!(refusal(ok), None, "{ok}");
        }
        for bad in [
            r#"{"provider":"cerebras"}"#,
            r#"{"provider":{"sort":"fastest"}}"#,
            r#"{"provider":{"sort":["latency"]}}"#,
            r#"{"provider":{"order":["groq"],"data_collection":"allow"}}"#,
            r#"{"provider":{"order":[]}}"#,
            r#"{"provider":{"allow_fallbacks":true}}"#,
            r#"{"provider":{"order":["Groq"]}}"#,
            r#"{"provider":{"order":[1]}}"#,
            r#"{"provider":{"order":["a","b","c","d","e","f","g","h","i"]}}"#,
            r#"{"provider":{"order":["groq"],"allow_fallbacks":"no"}}"#,
        ] {
            assert!(refusal(bad).is_some(), "{bad}");
        }
    }

    /// T-7758: per effort and provider, the thinking gets its budget on top of the answer's
    /// `max_tokens`; a chat body naming no `max_tokens` keeps the provider's own.
    #[test]
    fn chat_completions_carry_the_effort_and_messages_a_thinking_budget() {
        for (effort, budget) in BUDGETS {
            let chat = sent(
                r#"{"model":"m","max_tokens":100}"#,
                "v1/chat/completions",
                effort,
            );
            assert_eq!(chat["reasoning"], json!({ "effort": effort }));
            assert_eq!(chat["max_tokens"], 100 + budget, "{effort}");

            let messages = sent(r#"{"model":"m","max_tokens":100}"#, "v1/messages", effort);
            assert_eq!(
                messages["thinking"],
                json!({ "type": "enabled", "budget_tokens": budget })
            );
            assert_eq!(messages["max_tokens"], 100 + budget, "{effort}");
        }
        let unsaid = sent(r#"{"model":"m"}"#, "chat/completions", "low");
        assert_eq!(unsaid.get("max_tokens"), None, "{unsaid}");
    }

    #[test]
    fn a_setting_already_in_the_body_or_an_unknown_effort_is_left_alone() {
        let own = r#"{"reasoning":{"effort":"high"}}"#;
        assert_eq!(
            sent(own, "chat/completions", "low"),
            serde_json::from_str::<Value>(own).unwrap()
        );
        assert!(sent("{}", "chat/completions", "extreme")
            .get("reasoning")
            .is_none());
        let not_json = Bytes::from_static(b"not json");
        assert_eq!(with_reasoning(&not_json, "messages", "high"), not_json);
    }

    #[test]
    fn a_stream_is_counted_from_its_own_usage_frames_split_anywhere() {
        let anthropic = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":90,\"output_tokens\":1}}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":30}}\n\n";
        let mut usage = StreamUsage::default();
        for piece in anthropic.as_bytes().chunks(7) {
            usage.feed(piece);
        }
        assert_eq!(usage.tokens(), Some(120));

        let mut openai = StreamUsage::default();
        openai.feed(b"data: {\"usage\":{\"total_tokens\":42}}\n\ndata: [DONE]\n\n");
        assert_eq!(openai.tokens(), Some(42));
        let mut none = StreamUsage::default();
        none.feed(b"data: {\"choices\":[]}\n\n: keep-alive\n");
        assert_eq!(none.tokens(), None);
        // Output alone, before any input is told, still counts (T-7795).
        let mut output = StreamUsage::default();
        output.feed(b"data: {\"usage\":{\"output_tokens\":5}}\n");
        assert_eq!(output.tokens(), Some(5));
    }

    /// A line exactly at the cap is read whole; one byte past it is cut and counts nothing
    /// (T-7795).
    #[test]
    fn a_line_at_the_cap_is_read_and_one_byte_past_it_is_not() {
        let line = |length: usize| {
            let mut line = b"data: {\"usage\":{\"total_tokens\":9},\"pad\":\"".to_vec();
            line.resize(length - 3, b'x');
            line.extend_from_slice(b"\"}\n");
            line
        };
        let mut at = StreamUsage::default();
        at.feed(&line(STREAM_LINE_MAX + 1));
        assert_eq!(at.tokens(), Some(9));
        let mut past = StreamUsage::default();
        past.feed(&line(STREAM_LINE_MAX + 2));
        assert_eq!(past.tokens(), None);
    }

    /// T-2771: the counts are read in both shapes, the cached input included, and the frame the
    /// Portal gets carries them with the latency and the model named in the call.
    #[test]
    fn a_call_usage_carries_its_halves_its_cache_its_latency_and_its_model() {
        let openai = json!({ "prompt_tokens": 3980, "completion_tokens": 140, "total_tokens": 4120, "prompt_tokens_details": { "cached_tokens": 3200 } });
        assert_eq!(counts(&openai), (Some(4120), 3980, 140, 3200));
        let anthropic =
            json!({ "input_tokens": 90, "output_tokens": 30, "cache_read_input_tokens": 800 });
        assert_eq!(counts(&anthropic), (None, 90, 30, 800));
        assert_eq!(counts(&json!({})), (None, 0, 0, 0));

        let mut streamed = StreamUsage::default();
        streamed.feed(b"data: {\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2,\"prompt_tokens_details\":{\"cached_tokens\":8}}}\n");
        assert_eq!(
            (streamed.input, streamed.output, streamed.cached),
            (10, 2, 8)
        );

        let long = "m".repeat(400);
        let body = Bytes::from(json!({ "model": long, "messages": [] }).to_string());
        assert_eq!(model_of(&body).map(|m| m.len()), Some(MODEL_NAME_MAX));
        assert_eq!(model_of(&Bytes::from_static(b"not json")), None);

        let call = CallUsage {
            tokens: 4120,
            input: 3980,
            output: 140,
            cached: 3200,
            latency_ms: 1830,
            model: Some("google/gemini-3.8-flash".into()),
            effort: None,
            cost: None,
        };
        assert_eq!(
            call.payload(),
            json!({ "tokensThisStep": 4120, "inputTokens": 3980, "outputTokens": 140, "cachedTokens": 3200, "latencyMs": 1830, "model": "google/gemini-3.8-flash" })
        );
        assert!(CallUsage::default().payload().get("model").is_none());
        assert!(CallUsage::default()
            .payload()
            .get("reasoningEffort")
            .is_none());
        let at = CallUsage {
            effort: Some("medium"),
            ..CallUsage::default()
        };
        assert_eq!(at.payload()["reasoningEffort"], "medium");
        // AG-97: the provider's cost travels with the call; a nonsense figure does not.
        let paid = CallUsage {
            cost: Some(0.0042),
            ..CallUsage::default()
        };
        assert_eq!(paid.payload()["costUsd"], 0.0042);
        let nonsense = CallUsage {
            cost: Some(-1.0),
            ..CallUsage::default()
        };
        assert!(nonsense.payload().get("costUsd").is_none());
    }

    /// T-2998: the effort the body as sent asks for, in either provider's shape, and only one of
    /// the three.
    #[test]
    fn the_effort_is_read_from_the_body_as_sent() {
        let sent = |json: Value| effort_of(&Bytes::from(json.to_string()));
        let plain = Bytes::from(json!({ "model": "m", "max_tokens": 100 }).to_string());
        // The run's setting as the proxy wrote it, on both endpoints.
        assert_eq!(
            effort_of(&with_reasoning(&plain, "v1/chat/completions", "medium")),
            Some("medium")
        );
        assert_eq!(
            effort_of(&with_reasoning(&plain, "v1/messages", "high")),
            Some("high")
        );
        // A body that carried its own is sent as is, and reported as what it said.
        assert_eq!(
            sent(json!({ "reasoning": { "effort": "low" } })),
            Some("low")
        );
        assert_eq!(sent(json!({ "reasoning_effort": "high" })), Some("high"));
        // Nothing, a made-up effort or a budget of its own: nothing is reported.
        assert_eq!(effort_of(&plain), None);
        assert_eq!(sent(json!({ "reasoning": { "effort": "<script>" } })), None);
        assert_eq!(
            sent(json!({ "thinking": { "type": "enabled", "budget_tokens": 1234 } })),
            None
        );
        assert_eq!(effort_of(&Bytes::from_static(b"not json")), None);
    }

    #[test]
    fn a_streamed_chat_asks_for_its_usage_and_messages_are_left_alone() {
        let asked = |body: &str, rest: &str| -> Value {
            serde_json::from_slice(&with_stream_usage(&Bytes::from(body.to_owned()), rest)).unwrap()
        };
        let chat = asked(r#"{"stream":true}"#, "v1/chat/completions");
        assert_eq!(chat["stream_options"], json!({ "include_usage": true }));
        let own = asked(
            r#"{"stream":true,"stream_options":{"include_usage":false}}"#,
            "chat/completions",
        );
        assert_eq!(own["stream_options"], json!({ "include_usage": false }));
        assert!(asked(r#"{"stream":true}"#, "v1/messages")
            .get("stream_options")
            .is_none());
        assert!(wants_stream(&Bytes::from_static(br#"{"stream":true}"#)));
        assert!(!wants_stream(&Bytes::from_static(b"not json")));
        assert_eq!(
            uncounted_stream(&Bytes::from_static(br#"{"max_tokens":60}"#)),
            60 + 4
        );
    }

    /// T-7795: a stream whose body asks for every token there is is counted as the most there
    /// is, never wrapped round to a handful: an assistant's or an App's body reaches this
    /// uncapped, and its day is counted from it.
    #[test]
    fn a_stream_asking_for_every_token_is_counted_as_the_most_there_is() {
        let body = Bytes::from(format!(r#"{{"stream":true,"max_tokens":{}}}"#, u64::MAX));
        assert_eq!(uncounted_stream(&body), u64::MAX);
    }

    /// A messages body that names its own thinking keeps it, whatever the run's effort says.
    #[test]
    fn a_messages_body_with_its_own_thinking_is_sent_as_it_came() {
        let body = Bytes::from_static(
            br#"{"max_tokens":10,"thinking":{"type":"enabled","budget_tokens":1024}}"#,
        );
        assert_eq!(with_reasoning(&body, "v1/messages", "high"), body);
    }

    /// Off OpenRouter, a body that is not an object, or names no provider, goes byte for byte;
    /// an object's provider is dropped and the rest kept.
    #[test]
    fn only_an_objects_provider_is_dropped_off_openrouter() {
        for body in ["[1,2]", "not json", r#"{"model":"m"}"#] {
            let sent = Bytes::from(body.to_owned());
            assert_eq!(routed(sent.clone(), false), sent, "{body}");
        }
        let routed: Value = serde_json::from_slice(&routed(
            Bytes::from_static(br#"{"model":"m","provider":{"sort":"price"}}"#),
            false,
        ))
        .expect("JSON");
        assert_eq!(routed, json!({ "model": "m" }));
    }

    /// A stream body that is not an object, or whose `stream_options` is not one, is sent as it
    /// came: the proxy asks for usage only where it can add the flag without changing a value.
    #[test]
    fn a_stream_body_the_usage_flag_cannot_join_is_sent_as_it_came() {
        for body in ["[true]", r#"{"stream":true,"stream_options":"yes"}"#] {
            let sent = Bytes::from(body.to_owned());
            assert_eq!(
                with_stream_usage(&sent, "v1/chat/completions"),
                sent,
                "{body}"
            );
        }
        let asked: Value = serde_json::from_slice(&with_stream_usage(
            &Bytes::from_static(br#"{"stream":true,"stream_options":{"include_usage":false}}"#),
            "chat/completions",
        ))
        .expect("JSON");
        assert_eq!(asked["stream_options"]["include_usage"], json!(false));
    }

    /// A stream line past the cap is dropped unread, and the reader goes on with the next one;
    /// a provider's `usage.cost` is the call's cost.
    #[test]
    fn a_line_past_the_cap_is_dropped_and_the_next_one_is_read() {
        let mut usage = StreamUsage::default();
        let mut huge = b"data: {\"usage\":{\"total_tokens\":999999,\"pad\":\"".to_vec();
        huge.resize(STREAM_LINE_MAX + 64, b'x');
        huge.extend_from_slice(b"\"}}\n");
        usage.feed(&huge);
        assert_eq!(usage.tokens(), None, "the cut line counted nothing");
        usage.feed(
            b"data: {\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":4,\"cost\":0.25}}\n",
        );
        assert_eq!(usage.tokens(), Some(7));
        assert_eq!(usage.cost, Some(0.25));
        assert!(usage.line.is_empty());
    }
}
