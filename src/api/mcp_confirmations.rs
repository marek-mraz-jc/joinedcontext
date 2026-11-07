//! The decision page of a broker's MCP write (T-5829, Antares T-5816): an Antares started with
//! `ANTARES_MCP_CONFIRM_URL` sends the person here instead of trusting the AI client's own
//! accept. The Portal reads the question from the broker with its admin token and records the
//! decision there in the name of the person signed in, so only the person who asked decides it.
//!
//! The id names no space, so every broker the Portal reaches is asked. The broker answers its
//! question to anyone holding the admin token; the Portal shows it only to the subject whose call
//! asked, and every other person gets the same 404 as an unknown id.

use std::collections::BTreeSet;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use super::mcp_elicitations::{refuse_bearer, DecisionRequest};
use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::mcp::elicitation::ElicitationDecision;
use crate::state::AppState;

/// The subject header the gateway sets for a person's call (the token's `sub`), the one the
/// broker records with the question and the one the Portal vouches for when it decides.
const SUBJECT: &str = "x-jc-subject";

/// One broker question as its page shows it to the person who asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Confirmation {
    /// The question's id, 32 hex digits.
    pub id: String,
    /// The broker view the write goes through, `bb-air`.
    pub view: String,
    /// The MCP tool that would run, `create_entity`.
    pub tool: String,
    /// The words the client was handed, shown again on the page.
    pub message: String,
    /// Unix seconds after which the broker no longer takes a decision.
    pub expires_at: i64,
    /// `None` until the person decided.
    pub decision: Option<ElicitationDecision>,
}

/// The question as the broker answers it (Antares admin API, "MCP write decisions").
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Asked {
    id: String,
    view: String,
    tool: String,
    message: String,
    #[serde(default)]
    subject: std::collections::BTreeMap<String, Vec<String>>,
    expires_in_seconds: i64,
    decision: Option<ElicitationDecision>,
}

impl Asked {
    fn asked_by(&self, subject: &str) -> bool {
        !subject.is_empty()
            && self
                .subject
                .get(SUBJECT)
                .is_some_and(|values| values.iter().any(|v| v == subject))
    }

    fn shown(self) -> Confirmation {
        Confirmation {
            id: self.id,
            view: self.view,
            tool: self.tool,
            message: self.message,
            expires_at: crate::auth::session::now_unix().saturating_add(self.expires_in_seconds),
            decision: self.decision,
        }
    }
}

/// A broker question id is a UUID's 32 lowercase hex digits; anything else is asked nowhere.
fn well_formed(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn not_open(id: &str) -> ApiError {
    ApiError::NotFound(format!(
        "no open question '{id}' of yours: it was answered, it expired after ten minutes, or \
         another person's client asked it. Ask the client to send its write again for a new one"
    ))
}

fn unreachable() -> ApiError {
    ApiError::Unavailable(
        "a context broker did not answer, so the question may be on it; try again in a minute"
            .into(),
    )
}

/// The address of every broker the Portal reaches: the implicit `main` and each declared one.
fn brokers(state: &AppState) -> Vec<String> {
    let mut names = BTreeSet::from([jc_core::kinds::IMPLICIT_BROKER.to_owned()]);
    names.extend(
        state
            .mirror
            .matching(|env| env.kind == "ContextBroker")
            .into_iter()
            .map(|env| env.metadata.name),
    );
    names
        .iter()
        .filter_map(|name| state.space_usage.address(name))
        .collect()
}

/// The most a broker's answer about one question may hold; the words it carries are far shorter.
const MAX_ANSWER: usize = 256 * 1024;

/// Sends `method` for question `id` to `broker` and reads its answer, at most [`MAX_ANSWER`]
/// bytes: `None` when the broker does not hold the question, `Err` (logged) when it did not
/// answer, refused, or answered something that is no question.
async fn ask(
    state: &AppState,
    token: &str,
    method: Method,
    broker: &str,
    id: &str,
    body: Option<&serde_json::Value>,
) -> Result<Option<Asked>, ()> {
    let mut request = state
        .space_usage
        .http()
        .request(method, format!("{broker}/q/mcp-confirmations/{id}"))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(body);
    }
    let mut response = request.send().await.map_err(|err| {
        tracing::warn!(error = %err.without_url(), "a broker did not answer an MCP question call");
    })?;
    match response.status() {
        reqwest::StatusCode::OK => {}
        reqwest::StatusCode::NOT_FOUND => return Ok(None),
        status => {
            tracing::warn!(%status, "a broker refused an MCP question call");
            return Err(());
        }
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|err| {
        tracing::warn!(error = %err.without_url(), "a broker's MCP question broke off");
    })? {
        if bytes.len().saturating_add(chunk.len()) > MAX_ANSWER {
            tracing::warn!("a broker's MCP question passed {MAX_ANSWER} bytes");
            return Err(());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map(Some).map_err(|err| {
        tracing::warn!(error = %err, "a broker's MCP question did not parse");
    })
}

/// The broker holding question `id` and the question, when `subject` asked it.
async fn find(state: &AppState, id: &str, subject: &str) -> Result<(String, Asked), ApiError> {
    if !well_formed(id) {
        return Err(not_open(id));
    }
    let brokers = brokers(state);
    if brokers.is_empty() {
        return Err(not_open(id));
    }
    let token = state.space_usage.token().map_err(|why| {
        tracing::warn!("{why}");
        unreachable()
    })?;
    let mut missed = false;
    for broker in brokers {
        match ask(state, &token, Method::GET, &broker, id, None).await {
            Ok(Some(asked)) => {
                return match asked.id == id && asked.asked_by(subject) {
                    true => Ok((broker, asked)),
                    false => Err(not_open(id)),
                };
            }
            Ok(None) => {}
            Err(()) => missed = true,
        }
    }
    Err(if missed { unreachable() } else { not_open(id) })
}

#[utoipa::path(
    get,
    path = "/api/v1/mcp-confirmations/{id}",
    summary = "Read a Broker MCP Write's Question",
    description = "The question a context broker's MCP write tool asked, for the person whose client asked it: the view, the tool, the words the client showed, when it expires and the decision so far (T-5829). Another person's question, an answered or an expired one is 404; a bearer token is refused.",
    tag = "ops",
    params(("id" = String, Path, description = "The question's id, 32 hex digits")),
    responses(
        (status = 200, description = "The open question", body = Confirmation),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "A bearer token, not the person's session", body = crate::error::ProblemDetails),
        (status = 404, description = "No open question of this person by that id", body = crate::error::ProblemDetails),
        (status = 503, description = "A broker did not answer", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn get_confirmation(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Confirmation>, ApiError> {
    refuse_bearer(&user)?;
    let (_, asked) = find(&state, &id, &user.0.identity.subject).await?;
    Ok(Json(asked.shown()))
}

#[utoipa::path(
    post,
    path = "/api/v1/mcp-confirmations/{id}/decision",
    summary = "Decide a Broker MCP Write's Question",
    description = "The person whose client asked records `accept` or `decline` on the broker, in the name of the Portal session's subject; the client's retry runs the write or ends it on that decision (T-5829). A decision may be changed until the retry. Another person's question, an answered or an expired one is 404; a bearer token is refused.",
    tag = "ops",
    params(("id" = String, Path, description = "The question's id, 32 hex digits")),
    request_body(content = DecisionRequest, example = json!({"decision": "accept"})),
    responses(
        (status = 200, description = "The question with the decision recorded", body = Confirmation),
        (status = 400, description = "A body that is not `{\"decision\": \"accept\"}` or `decline`", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "A bearer token, not the person's session", body = crate::error::ProblemDetails),
        (status = 404, description = "No open question of this person by that id", body = crate::error::ProblemDetails),
        (status = 503, description = "A broker did not answer", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn decide_confirmation(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<DecisionRequest>, JsonRejection>,
) -> Result<Json<Confirmation>, ApiError> {
    refuse_bearer(&user)?;
    let Json(request) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let subject = &user.0.identity.subject;
    let (broker, _) = find(&state, &id, subject).await?;
    let token = state.space_usage.token().map_err(|_| unreachable())?;
    let body = json!({ "decision": request.decision, "subject": { SUBJECT: subject } });
    match ask(&state, &token, Method::PUT, &broker, &id, Some(&body)).await {
        Ok(Some(asked)) => Ok(Json(asked.shown())),
        Ok(None) => Err(not_open(&id)),
        Err(()) => Err(unreachable()),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/mcp-confirmations/{id}", get(get_confirmation))
        .route(
            "/mcp-confirmations/{id}/decision",
            post(decide_confirmation),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_uuid_hex_id_is_asked() {
        assert!(well_formed("0a1b2c3d4e5f60718293a4b5c6d7e8f9"));
        assert!(!well_formed("0A1B2C3D4E5F60718293A4B5C6D7E8F9"));
        assert!(!well_formed("../../q/views"));
        assert!(!well_formed("0a1b2c3d4e5f60718293a4b5c6d7e8f"));
    }

    #[test]
    fn a_question_is_shown_only_to_the_subject_who_asked() {
        let asked: Asked = serde_json::from_value(json!({
            "id": "0a1b2c3d4e5f60718293a4b5c6d7e8f9", "view": "air", "tool": "create_entity",
            "message": "m", "subject": {"x-jc-subject": ["sub-ana"], "x-jc-groups": ["sub-bo"]},
            "expiresInSeconds": 60, "decision": null
        }))
        .expect("question");
        assert!(asked.asked_by("sub-ana"));
        assert!(!asked.asked_by("sub-bo"));
        assert!(!asked.asked_by(""));
    }

    #[test]
    fn a_token_a_client_holds_never_decides() {
        let now = crate::auth::session::now_unix();
        let session = |client: Option<&str>| {
            CurrentUser(crate::auth::session::Session {
                identity: crate::auth::session::Identity {
                    client: client.map(str::to_owned),
                    subject: "sub-ana".into(),
                    username: "ana".into(),
                    email: None,
                    name: None,
                    roles: Vec::new(),
                    groups: Vec::new(),
                    second_factor: true,
                },
                expires_at: now + 60,
                issued_at: now,
                signed_in_at: None,
                id_token: "id".into(),
                access_expires_at: now + 60,
                refresh_token: None,
            })
        };
        assert!(matches!(
            refuse_bearer(&session(Some("mcp-hub"))),
            Err(ApiError::Denied(_))
        ));
        assert!(refuse_bearer(&session(None)).is_ok());
    }
}
