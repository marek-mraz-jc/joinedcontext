//! What a project's stored assistant turns add up to, and the same turns as CSV (API/05 §3.1,
//! API/01 §34, T-4728): the assistant computes both; the Portal checks the caller administers the
//! assistant, counts the call toward the transcripts' rate and records who read what.

use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use jc_core::kinds::Verb;
use serde::Deserialize;
use serde_json::{json, Value};
use utoipa::IntoParams;

use super::knowledge::{administers, admitted, ask_text, ask_with, audited, LIST_TIME};
use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::state::AppState;

/// The summary and the export of the stored turns, beside [`super::knowledge::router`]: this
/// file calls into that one, so it mounts its own routes (no module cycle, T-4965).
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/knowledge/conversations/usage",
            get(conversation_usage),
        )
        .route(
            "/projects/{project}/knowledge/conversations/export",
            get(export_conversations),
        )
}

/// The longest an export may take: up to 20,000 turns.
const EXPORT_TIME: Duration = Duration::from_secs(60);

/// The turns a summary or an export reads (API/05 §3.1); `jc-assistant` holds every bound.
#[derive(Debug, Default, Deserialize, serde::Serialize, IntoParams)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UsageRange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment: Option<String>,
    /// RFC 3339, inclusive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// RFC 3339, exclusive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// The route the turns took (`rag`, `data`, `direct`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/conversations/usage",
    summary = "Summarize Stored Turns",
    description = "What the project's stored assistant turns in the range add up to: questions, timeouts, errors, the cost of the priced turns (total, mean, p50, p90) with the unpriced counted apart, tokens, the speed per phase, the cost per stage and the newest 14 days (API/05 §3.1). Needs `approve` on AssistantDeployment; every call is recorded (API/01 §34).",
    tag = "knowledge",
    params(("project" = String, Path, description = "Project name"), UsageRange),
    responses(
        (status = 200, description = "The summary", body = Object),
        (status = 400, description = "A range value out of its bounds", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not approve an AssistantDeployment", body = crate::error::ProblemDetails),
        (status = 429, description = "60 calls on the stored conversations in a minute already", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn conversation_usage(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(range): Query<UsageRange>,
) -> Result<Json<Value>, ApiError> {
    administers(&state, &user, &project, Verb::Approve)?;
    admitted(&user.0.identity.subject, Instant::now())?;
    let path = format!("{project}/knowledge/conversations/usage");
    let query = |r: reqwest::RequestBuilder| r.query(&range);
    let (_, body) = ask_with(&state, reqwest::Method::GET, &path, LIST_TIME, query).await?;
    let what = "summarized the stored conversations";
    audited(&state, &user, &project, what, json!(range)).await;
    Ok(Json(body))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/knowledge/conversations/export",
    summary = "Export Stored Turns",
    description = "The project's stored assistant turns in the range as CSV, one row per turn, newest first, the questions as stored (redacted) and an unpriced turn's cost empty (API/05 §3.1). More than 20,000 turns is 400. Needs `approve` on AssistantDeployment; every call is recorded (API/01 §34).",
    tag = "knowledge",
    params(("project" = String, Path, description = "Project name"), UsageRange),
    responses(
        (status = 200, description = "text/csv, one row per turn", content_type = "text/csv", body = String),
        (status = 400, description = "A range value out of its bounds, or more than 20,000 turns", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller may not approve an AssistantDeployment", body = crate::error::ProblemDetails),
        (status = 429, description = "60 calls on the stored conversations in a minute already", body = crate::error::ProblemDetails),
        (status = 503, description = "The knowledge assistant is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn export_conversations(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Query(range): Query<UsageRange>,
) -> Result<Response, ApiError> {
    administers(&state, &user, &project, Verb::Approve)?;
    admitted(&user.0.identity.subject, Instant::now())?;
    let path = format!("{project}/knowledge/conversations/export");
    let csv = ask_text(&state, &path, EXPORT_TIME, &range).await?;
    let what = "exported the stored conversations";
    audited(&state, &user, &project, what, json!(range)).await;
    Ok(csv_file(&project, csv))
}

/// `csv` as a download named after `project` (a DNS-1123 name), never kept by a cache.
fn csv_file(project: &str, csv: String) -> Response {
    let file = format!("attachment; filename=\"{project}-conversations.csv\"");
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_owned()),
            (header::CONTENT_DISPOSITION, file),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        csv,
    )
        .into_response()
}
