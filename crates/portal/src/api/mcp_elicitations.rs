//! The page behind an MCP call's question (AG-63, T-5699): the person who asked reads what the
//! call would change and decides there, with their own Portal session. The decision is recorded
//! here and the client's second call reads it; a client's word never accepts.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::mcp::elicitation::{ElicitationDecision, Question};
use crate::state::AppState;

/// The person's answer to one question.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequest {
    /// `accept` lets the call run once; `decline` refuses it.
    pub decision: ElicitationDecision,
}

/// A question is decided by the person at the keyboard, never with a token a client holds:
/// that client is the one asking (AG-63, AG-133).
pub(crate) fn refuse_bearer(user: &CurrentUser) -> Result<(), ApiError> {
    if user.0.identity.client.is_some() {
        return Err(ApiError::Denied(
            "an MCP call's question is answered by the person in the Portal, never with a \
             client's token (AG-63)"
                .into(),
        ));
    }
    Ok(())
}

fn not_open(id: &str) -> ApiError {
    ApiError::NotFound(format!(
        "no open question '{id}' of yours: it was answered, it expired after ten minutes, or \
         another person's client asked it. Ask the client to send its call again for a new one"
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/mcp-elicitations/{id}",
    summary = "Read an MCP Call's Question",
    description = "The question a Yellow, Red or destructive MCP call asked, for the person whose client asked it: what would run, the page of what it changes, when it expires and the decision so far (AG-63). Another person's question, an answered or an expired one is 404; a bearer token is refused.",
    tag = "ops",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "The question's id, `eli-` and 16 hex digits"),
    ),
    responses(
        (status = 200, description = "The open question", body = Question),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "A bearer token, not the person's session", body = crate::error::ProblemDetails),
        (status = 404, description = "No open question of this person by that id", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn get_question(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Json<Question>, ApiError> {
    refuse_bearer(&user)?;
    state
        .mcp_elicitations
        .question(&user.0.identity.subject, &project, &id)
        .map(Json)
        .ok_or_else(|| not_open(&id))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/mcp-elicitations/{id}/decision",
    summary = "Decide an MCP Call's Question",
    description = "The person whose client asked records `accept` or `decline`; the client's next call with the question's id runs or is refused on it (AG-63). A decision may be changed until that call. Another person's question, an answered or an expired one is 404; a bearer token is refused.",
    tag = "ops",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "The question's id, `eli-` and 16 hex digits"),
    ),
    request_body(content = DecisionRequest, example = json!({"decision": "accept"})),
    responses(
        (status = 200, description = "The question with the decision recorded", body = Question),
        (status = 400, description = "A body that is not `{\"decision\": \"accept\"}` or `decline`", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "A bearer token, not the person's session", body = crate::error::ProblemDetails),
        (status = 404, description = "No open question of this person by that id", body = crate::error::ProblemDetails),
    )
)]
pub(crate) async fn decide_question(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
    body: Result<Json<DecisionRequest>, JsonRejection>,
) -> Result<Json<Question>, ApiError> {
    refuse_bearer(&user)?;
    let Json(request) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    state
        .mcp_elicitations
        .decide(&user.0.identity.subject, &project, &id, request.decision)
        .map(Json)
        .ok_or_else(|| not_open(&id))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/mcp-elicitations/{id}",
            get(get_question),
        )
        .route(
            "/projects/{project}/mcp-elicitations/{id}/decision",
            post(decide_question),
        )
}
