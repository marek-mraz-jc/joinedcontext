//! `POST /apps/{name}/api/services/ai/complete`: a completion for an App from the model the
//! platform chooses, through `jc-agent-proxy`, within the App's `aiTokensPerDay` (AP-169, AP-165,
//! API/06 §3, T-3584).
//!
//! The App holds no key and names no model (AP-147): the Portal sends the call as its own service
//! account with `X-JC-App`, the proxy adds the organization's key. The model is the `app-builder`
//! profile's, or the first of the organization's `policies.agents.models` when that list leaves
//! the profile's out. Neither a prompt nor an answer is logged: an App's people write into them.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use jc_core::kinds::AppService;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

use super::services::{self, Quota};
use crate::agents::profile::Profile;
use crate::error::ApiError;
use crate::state::AppState;

pub const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_MESSAGES: usize = 50;
const MAX_CONTENT_BYTES: usize = 100 * 1024;
const MAX_SCHEMA_BYTES: usize = 16 * 1024;
/// The answer's ceiling unless the App asks for less, and the most it may ask for.
const DEFAULT_MAX_TOKENS: u32 = 1024;
const MAX_TOKENS: u32 = 4096;
/// The profile whose model an App's completion uses.
const PROFILE: &str = "app-builder";
const PROXY_TIMEOUT: Duration = Duration::from_secs(60);
const SERVICE: &str = "ai";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Complete {
    messages: Vec<Message>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    schema: Option<Value>,
}

fn bad(detail: impl Into<String>) -> Response {
    ApiError::BadRequest(detail.into()).into_response()
}

/// What is wrong with a request before anything is asked, `None` when nothing is.
fn refusal(request: &Complete) -> Option<String> {
    if request.messages.is_empty() || request.messages.len() > MAX_MESSAGES {
        return Some(format!("messages holds 1 to {MAX_MESSAGES} messages"));
    }
    if let Some(message) = request
        .messages
        .iter()
        .find(|message| !matches!(message.role.as_str(), "system" | "user" | "assistant"))
    {
        return Some(format!(
            "role '{}' is none of system, user and assistant",
            message.role
        ));
    }
    if !request
        .messages
        .iter()
        .any(|message| message.role == "user")
    {
        return Some("messages holds at least one user message".into());
    }
    let content: usize = request.messages.iter().map(|m| m.content.len()).sum();
    if content > MAX_CONTENT_BYTES {
        return Some(format!(
            "the messages hold at most {MAX_CONTENT_BYTES} bytes"
        ));
    }
    if request.max_tokens.is_some_and(|n| n == 0 || n > MAX_TOKENS) {
        return Some(format!("maxTokens is 1 to {MAX_TOKENS}"));
    }
    match &request.schema {
        Some(schema) if !schema.is_object() => Some("schema is a JSON Schema object".into()),
        Some(schema) if schema.to_string().len() > MAX_SCHEMA_BYTES => {
            Some(format!("schema is at most {MAX_SCHEMA_BYTES} bytes"))
        }
        Some(schema) if jsonschema::validator_for(schema).is_err() => {
            Some("schema is no JSON Schema".into())
        }
        _ => None,
    }
}

/// Why no model answers a call: no profile to take it from, or a profile whose model the
/// organization does not allow.
enum NoModel {
    NoProfile,
    NotAllowed(String),
}

/// The model and the body shape: the profile's, held to the organization's list (AG-53). A model
/// outside the list is refused, never swapped for the list's first one: that model may belong to
/// another provider than the profile's (F-POR2-014).
fn model(state: &AppState, project: &str) -> Result<(String, String), NoModel> {
    let profile = Profile::load(&state.mirror, PROFILE).map_err(|_| NoModel::NoProfile)?;
    // The models the App's own Organization allows (ADR-N-058).
    let allowed: Option<Vec<String>> =
        crate::permissions::organization_manifest_of(&state.mirror, project)
            .and_then(|org| org.spec.pointer("/policies/agents/models").cloned())
            .and_then(|models| serde_json::from_value(models).ok());
    if allowed.is_some_and(|list| !list.contains(&profile.model_name)) {
        return Err(NoModel::NotAllowed(profile.model_name));
    }
    Ok((profile.model_name, profile.model_provider))
}

/// The provider's body: Anthropic's messages with the system part apart, else chat completions.
fn upstream(
    (model, provider): (&str, &str),
    request: &Complete,
    max_tokens: u32,
) -> (&'static str, Value) {
    let mut system: Vec<&str> = request
        .messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| m.content.as_str())
        .collect();
    let instruction = request.schema.as_ref().map(|schema| {
        format!(
            "Answer with exactly one JSON value and nothing else, no prose and no code fence, \
             that conforms to this JSON Schema: {schema}"
        )
    });
    if let Some(instruction) = &instruction {
        system.push(instruction);
    }
    let turns = request.messages.iter().filter(|m| m.role != "system");
    if provider == "anthropic" {
        let messages: Vec<Value> = turns
            .map(|m| json!({ "role": m.role, "content": m.content }))
            .collect();
        let mut body = json!({ "model": model, "max_tokens": max_tokens, "messages": messages });
        if !system.is_empty() {
            body["system"] = json!(system.join("\n\n"));
        }
        ("/v1/llm/v1/messages", body)
    } else {
        let mut messages: Vec<Value> = Vec::new();
        if !system.is_empty() {
            messages.push(json!({ "role": "system", "content": system.join("\n\n") }));
        }
        messages.extend(turns.map(|m| json!({ "role": m.role, "content": m.content })));
        (
            "/v1/llm/chat/completions",
            json!({ "model": model, "max_tokens": max_tokens, "messages": messages }),
        )
    }
}

/// The answer's text and its tokens in and out, from either provider's shape.
fn answer(body: &Value) -> Option<(String, u32, u32)> {
    let count = |pointer: &str| {
        body.pointer(pointer)
            .and_then(Value::as_u64)
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    if let Some(text) = body
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
    {
        return Some((
            text.to_owned(),
            count("/usage/prompt_tokens").unwrap_or(0),
            count("/usage/completion_tokens").unwrap_or(0),
        ));
    }
    let parts = body.get("content")?.as_array()?;
    let text: String = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect();
    Some((
        text,
        count("/usage/input_tokens").unwrap_or(0),
        count("/usage/output_tokens").unwrap_or(0),
    ))
}

/// The prompt's tokens before the model counts them: one per four bytes, rounded up, of the
/// messages and system part as sent, the schema's instruction with them. Close enough to hold
/// the day to; the model's own count settles it after the answer.
fn prompt_tokens(body: &Value) -> u32 {
    let bytes: usize = ["messages", "system"]
        .iter()
        .filter_map(|member| body.get(member))
        .map(|part| part.to_string().len())
        .sum();
    u32::try_from(bytes.div_ceil(4)).unwrap_or(u32::MAX)
}

/// One call to the model service as the Portal's own account naming the App: the answer's
/// text and its tokens in and out, or the response the App gets instead.
async fn ask_model(
    oidc: &crate::auth::oidc::OidcClient,
    settings: &crate::config::AgentSettings,
    (project, name): (&str, &str),
    (path, body): (&str, &Value),
) -> Result<(String, u32, u32), Box<Response>> {
    let token = oidc.service_token().await.map_err(|_| {
        Box::new(unavailable(
            "the Portal has no token to ask the model service with",
        ))
    })?;
    // One client for every App's calls, so they share its connection pool (F-R2POR3-022).
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let http = match HTTP.get() {
        Some(http) => http,
        None => {
            let built = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(PROXY_TIMEOUT)
                .build()
                .map_err(|_| Box::new(unavailable("the model service cannot be reached")))?;
            HTTP.get_or_init(|| built)
        }
    };
    let response = http
        .post(format!(
            "{}{path}",
            settings.proxy_base.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .header("x-jc-app", format!("{project}/{name}"))
        .json(body)
        .send()
        .await
        .map_err(|err| {
            tracing::warn!(app = %name, %project, timeout = err.is_timeout(), "an App's ai call did not reach the model service");
            Box::new(unavailable("the model service did not answer; try again in a moment"))
        })?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if matches!(status, 401..=403) {
        // The provider's refusal of the key reaches the App as the installation's problem.
        tracing::warn!(app = %name, %project, status, "the model service refused an App's ai call");
        return Err(Box::new(no_model_key()));
    }
    if !(200..300).contains(&status) {
        tracing::warn!(app = %name, %project, status, "the model service failed an App's ai call");
        return Err(Box::new(unavailable(
            "the model service failed; try again in a moment",
        )));
    }
    serde_json::from_str::<Value>(&text)
        .ok()
        .as_ref()
        .and_then(answer)
        .ok_or_else(|| Box::new(unavailable("the model service's answer could not be read")))
}

fn no_model_key() -> Response {
    services::problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "no-model-key",
        "No Model Key",
        "the organization's model key is missing, refused or out of credit".into(),
        json!({ "service": SERVICE }),
    )
}

fn unavailable(detail: &str) -> Response {
    ApiError::Unavailable(detail.to_owned()).into_response()
}

pub(super) async fn complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(name): Path<String>,
    body: Bytes,
) -> Response {
    let (project, spec, _) = match services::gate(
        &state,
        &headers,
        &uri,
        &name,
        (AppService::Ai, &Method::POST),
    )
    .await
    {
        Ok(gated) => gated,
        Err(refused) => return *refused,
    };
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim_start().starts_with("application/json"));
    if !json {
        return ApiError::UnsupportedMediaType("send the request as application/json".into())
            .into_response();
    }
    let request: Complete = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(err) => return bad(format!("the body is not a completion request: {err}")),
    };
    if let Some(why) = refusal(&request) {
        return bad(why);
    }
    let (Some(settings), Some(oidc), Some(db)) = (
        state.config.agent_settings.as_ref(),
        state.oidc.as_deref(),
        state.db.as_ref(),
    ) else {
        return unavailable("this installation answers no App's ai calls");
    };
    let (model, provider) = match model(&state, &project) {
        Ok(model) => model,
        Err(NoModel::NoProfile) => {
            return unavailable(
                "this installation has no app-builder profile to take the model from",
            )
        }
        Err(NoModel::NotAllowed(name)) => {
            return ApiError::Unprocessable {
                detail: format!("the app-builder profile's model {name} is not allowed"),
                errors: vec![format!(
                    "model: {name} is not in the organization's policies.agents.models; an \
                     organization administrator adds it there or sets the profile to an allowed model"
                )],
            }
            .into_response()
        }
    };

    let limit = services::quota(&state.mirror, &project, &spec, Quota::AiTokensPerDay);
    let used = match services::used_today(db, (&project, &name), SERVICE, "").await {
        Ok(used) => used,
        Err(err) => return ApiError::Internal(err.to_string()).into_response(),
    };
    let spent = || {
        services::quota_used(
            AppService::Ai,
            "aiTokensPerDay",
            Some(time::OffsetDateTime::now_utc()),
        )
    };
    let (path, mut upstream_body) = upstream((&model, &provider), &request, 0);
    let prompt = prompt_tokens(&upstream_body);
    let left = limit.saturating_sub(used).saturating_sub(prompt);
    if left == 0 {
        return spent();
    }
    let max_tokens = request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS).min(left);
    upstream_body["max_tokens"] = json!(max_tokens);
    // The prompt and the answer's ceiling are held before the call, in the statement that checks
    // the day, so calls in flight together cannot pass it together (T-3661).
    let held = prompt.saturating_add(max_tokens);
    match services::take(db, (&project, &name), SERVICE, "", held, limit).await {
        Ok(true) => {}
        Ok(false) => return spent(),
        Err(err) => return ApiError::Internal(err.to_string()).into_response(),
    }
    let asked = ask_model(oidc, settings, (&project, &name), (path, &upstream_body)).await;
    let used = asked.as_ref().map_or(0, |(_, tokens_in, tokens_out)| {
        tokens_in.saturating_add(*tokens_out)
    });
    if let Err(err) = services::settle(
        db,
        (&project, &name),
        SERVICE,
        "",
        i64::from(used) - i64::from(held),
    )
    .await
    {
        tracing::error!(app = %name, %project, error = %err, "an App's ai tokens were not settled");
    }
    let (answer_text, tokens_in, tokens_out) = match asked {
        Ok(answer) => answer,
        Err(refused) => return *refused,
    };
    tracing::info!(app = %name, %project, tokens_in, tokens_out, "app ai completion");

    let mut out = json!({ "text": answer_text, "tokens": { "in": tokens_in, "out": tokens_out } });
    if let Some(schema) = &request.schema {
        let parsed = serde_json::from_str::<Value>(answer_text.trim())
            .ok()
            .filter(|value| {
                jsonschema::validator_for(schema).is_ok_and(|validator| validator.is_valid(value))
            });
        let Some(parsed) = parsed else {
            return services::problem(
                StatusCode::BAD_GATEWAY,
                "no-json",
                "No JSON",
                "the model's answer is no JSON of the schema; ask again".into(),
                json!({ "service": SERVICE }),
            );
        };
        out["json"] = parsed;
    }
    Json(out).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::ORG_NAMESPACE;

    /// F-POR2-014: a profile model outside the organization's list is refused, never replaced
    /// by the list's first model.
    #[test]
    fn a_model_the_organization_does_not_allow_is_refused() {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let put = |kind: &str, name: &str, spec: Value| {
            state.mirror.upsert(ResourceEnvelope {
                api_version: API_VERSION.into(),
                kind: kind.into(),
                metadata: ObjectMeta::new(name, ORG_NAMESPACE),
                spec,
                status: None,
            })
        };
        put(
            "AgentProfile",
            PROFILE,
            serde_json::json!({
                "role": "builder",
                "runtime": { "image": "ghcr.io/x/agent:v1", "digest": format!("sha256:{}", "1".repeat(64)) },
                "model": { "provider": "openai-compatible", "name": "deepseek/deepseek-v4.1-flash", "maxTokensPerRun": 400000 },
                "limits": { "stepsPerRun": 120, "wallClock": "PT20M", "concurrentRunsPerOrganization": 2, "requestsPerMinute": 60, "maxResponseBytes": 2097152 },
                "egress": { "allowedHosts": [] },
                "tools": ["shell"],
                "workspace": { "cpu": "1", "memory": "2Gi", "ephemeralStorage": "4Gi" },
            }),
        );
        put(
            "Organization",
            "helsinki",
            serde_json::json!({
                "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en",
                "policies": { "agents": { "models": ["claude-sonnet-5-5"] } },
            }),
        );
        assert!(
            matches!(model(&state, "helsinki"), Err(NoModel::NotAllowed(name)) if name == "deepseek/deepseek-v4.1-flash")
        );
        put(
            "Organization",
            "helsinki",
            serde_json::json!({
                "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en",
                "policies": { "agents": { "models": ["deepseek/deepseek-v4.1-flash"] } },
            }),
        );
        assert!(
            matches!(model(&state, "helsinki"), Ok((name, provider)) if name == "deepseek/deepseek-v4.1-flash" && provider == "openai-compatible")
        );
    }

    fn request(value: Value) -> Complete {
        serde_json::from_value(value).expect("a request")
    }

    #[test]
    fn a_request_is_refused_before_anything_is_asked() {
        let user = json!([{ "role": "user", "content": "Classify." }]);
        assert_eq!(refusal(&request(json!({ "messages": user }))), None);
        for (body, why) in [
            (json!({ "messages": [] }), "1 to 50"),
            (
                json!({ "messages": [{ "role": "tool", "content": "x" }] }),
                "role 'tool'",
            ),
            (
                json!({ "messages": [{ "role": "system", "content": "x" }] }),
                "one user",
            ),
            (json!({ "messages": user, "maxTokens": 0 }), "maxTokens"),
            (json!({ "messages": user, "maxTokens": 5000 }), "maxTokens"),
            (
                json!({ "messages": user, "schema": [1] }),
                "JSON Schema object",
            ),
            (
                json!({ "messages": user, "schema": { "type": 7 } }),
                "no JSON Schema",
            ),
            (
                json!({ "messages": [{ "role": "user", "content": "x".repeat(MAX_CONTENT_BYTES + 1) }] }),
                "at most",
            ),
        ] {
            let refused = refusal(&request(body.clone())).unwrap_or_default();
            assert!(refused.contains(why), "{body}: {refused}");
        }
    }

    #[test]
    fn each_provider_gets_its_own_shape_with_the_schema_as_an_instruction() {
        let ask = request(json!({
            "messages": [{ "role": "system", "content": "You sort defects." },
                         { "role": "user", "content": "A pothole." }],
            "schema": { "type": "object" },
        }));
        let (path, body) = upstream(("claude-sonnet-5", "anthropic"), &ask, 100);
        assert_eq!(path, "/v1/llm/v1/messages");
        assert_eq!(
            body["messages"],
            json!([{ "role": "user", "content": "A pothole." }])
        );
        assert!(body["system"]
            .as_str()
            .is_some_and(|s| s.starts_with("You sort defects.") && s.contains("JSON Schema")));
        let (path, body) = upstream(("m", "openai-compatible"), &ask, 100);
        assert_eq!(path, "/v1/llm/chat/completions");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["max_tokens"], 100);

        assert_eq!(
            answer(
                &json!({ "content": [{ "type": "text", "text": "ok" }], "usage": { "input_tokens": 3, "output_tokens": 1 } })
            ),
            Some(("ok".to_owned(), 3, 1))
        );
        assert_eq!(
            answer(
                &json!({ "choices": [{ "message": { "content": "ok" } }], "usage": { "prompt_tokens": 5, "completion_tokens": 2 } })
            ),
            Some(("ok".to_owned(), 5, 2))
        );
        assert_eq!(answer(&json!({ "error": "x" })), None);
    }
}
