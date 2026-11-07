//! A space's retention as the broker runs it (DM-85, ADR-N-046): what a draft of
//! `spec.retention` would delete now, before an owner saves it, and what the sweeps deleted.
//!
//! The rules themselves are the space manifest's, saved through the manifest routes, where only
//! an owner changes them (`spaces::check_retention`). These two routes read the broker through
//! the reconciler's writer, which knows the type IRIs it writes the rules under.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use jc_core::kinds::{ContextSpaceSpec, RetentionRule, Verb};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::reconciler::space_rules::RetentionCount;
use crate::resource::is_dns1123;
use crate::state::AppState;

/// A draft of a space's retention rules.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RetentionDraft {
    /// The rules as `spec.retention` would hold them.
    #[schema(value_type = Vec<std::collections::HashMap<String, serde_json::Value>>)]
    pub retention: Vec<RetentionRule>,
}

/// Rows per class and kind.
#[derive(Debug, Serialize, ToSchema)]
pub struct RetentionCounts {
    pub counts: Vec<RetentionCount>,
}

/// The space's held rules when the caller may read it, else the 404 every unreadable space gets.
fn readable(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    space: &str,
) -> Result<(Vec<RetentionRule>, bool), ApiError> {
    let effective = crate::permissions::for_request(state, &user.0.identity, project);
    let held = (is_dns1123(project) && is_dns1123(space))
        .then(|| state.mirror.get(project, "ContextSpace", space))
        .flatten()
        .filter(|_| {
            effective.may_read_project() && effective.may_read_in("ContextSpace", Some(space))
        })
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "context space '{space}' not found in project '{project}'"
            ))
        })?;
    let rules = serde_json::from_value::<ContextSpaceSpec>(held.spec)
        .map(|spec| spec.retention)
        .unwrap_or_default();
    Ok((
        rules,
        effective.may_in("ContextSpace", Verb::Approve, Some(space)),
    ))
}

async fn counts(
    state: &AppState,
    project: &str,
    space: &str,
    draft: Option<&[RetentionRule]>,
    held: &[RetentionRule],
) -> Result<Json<RetentionCounts>, ApiError> {
    let views = state.views.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no broker address or admin token".into())
    })?;
    let tenant = crate::spaces::segment(&state.mirror, project, space);
    views
        .retention_counts(&tenant, draft, held)
        .await
        .map(|counts| Json(RetentionCounts { counts }))
        .map_err(|reason| {
            tracing::warn!(%reason, "the broker did not count a space's retention");
            ApiError::Unavailable("the broker did not count; try again shortly".into())
        })
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/spaces/{space}/retention/preview",
    summary = "Preview Space Retention",
    description = "What a draft of the space's `spec.retention` would delete now, per class and kind (`entity`, `history`), counted by the broker without deleting anything and at most 100 000 each. For the space's owners, who alone save retention (DM-85).",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    request_body(content = RetentionDraft, example = json!({"retention": [{"entityType": "Vehicle", "current": "PT5M", "history": "PT8H"}]})),
    responses(
        (status = 200, description = "What the draft would delete", body = RetentionCounts),
        (status = 400, description = "A rule the space manifest would refuse", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller reads the space but does not own it", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "No broker, or the broker did not answer", body = crate::error::ProblemDetails),
    )
)]
pub async fn preview_retention(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
    body: Result<Json<RetentionDraft>, JsonRejection>,
) -> Result<Json<RetentionCounts>, ApiError> {
    let (held, owner) = readable(&state, &user, &project, &space)?;
    if !owner {
        return Err(ApiError::Denied(format!(
            "only an owner of context space '{space}' sets its retention (DM-85)"
        )));
    }
    let Json(draft) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    ContextSpaceSpec {
        retention: draft.retention.clone(),
        ..ContextSpaceSpec::default()
    }
    .validate()
    .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    counts(&state, &project, &space, Some(&draft.retention), &held).await
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/retention/deleted",
    summary = "Read Space Retention Deletions",
    description = "What the broker's retention sweeps deleted of the space since the broker started, per class and kind (`entity`, `history`). The same numbers for everyone who reads the space.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    responses(
        (status = 200, description = "What retention deleted", body = RetentionCounts),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "No broker, or the broker did not answer", body = crate::error::ProblemDetails),
    )
)]
pub async fn retention_deleted(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
) -> Result<Json<RetentionCounts>, ApiError> {
    let (held, _) = readable(&state, &user, &project, &space)?;
    counts(&state, &project, &space, None, &held).await
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/spaces/{space}/retention/preview",
            post(preview_retention),
        )
        .route(
            "/projects/{project}/spaces/{space}/retention/deleted",
            get(retention_deleted),
        )
}
