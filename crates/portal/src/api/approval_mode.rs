//! `GET /api/v1/organization/approval-mode`: whether an Organization lets its one person approve
//! their own access Changes after a fresh second factor (T-7388). Its Administration page asks by
//! `organization`, an approval by `project`; only an installation operator changes the mode,
//! through the operator door (`SinglePersonSet`).

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::auth::session::CurrentUser;
use crate::error::ApiError;
use crate::single_person::ApprovalMode;
use crate::state::AppState;

/// Exactly one of the two.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ModeQuery {
    /// The Organization, e.g. `marek-mraz`: for a person who belongs to it.
    pub organization: Option<String>,
    /// A project, e.g. `ovzdusie`: its Organization's mode, for a person who reads the project.
    pub project: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/approval-mode",
    operation_id = "approval_mode_get",
    summary = "Read The Approval Mode",
    description = "Whether an Organization is in single-person approval mode (T-7388): its one person approves their own access Changes after a fresh second factor. Off by default; an installation operator sets it, and a second person with a role turns it off.",
    tag = "changes",
    params(ModeQuery),
    responses(
        (status = 200, description = "The mode", body = ApprovalMode),
        (status = 400, description = "Neither or both of organization and project", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No Organization or project the caller belongs to or reads", body = crate::error::ProblemDetails),
        (status = 503, description = "The database did not answer", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_approval_mode(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<ModeQuery>,
) -> Result<Json<ApprovalMode>, ApiError> {
    let identity = &user.0.identity;
    let organization = match (query.organization, query.project) {
        (Some(organization), None) => Some(organization).filter(|organization| {
            crate::permissions::belongs_to_organization(
                &state.mirror,
                &state.config.bootstrap_admins,
                identity,
                organization,
            )
        }),
        (None, Some(project)) => crate::permissions::organization_of(&state.mirror, &project)
            .filter(|_| {
                crate::permissions::for_request(&state, identity, &project).may_read_project()
            }),
        _ => {
            return Err(ApiError::BadRequest(
                "name exactly one of organization (e.g. marek-mraz) and project (e.g. ovzdusie)"
                    .into(),
            ))
        }
    }
    .ok_or_else(|| ApiError::NotFound("no such Organization or project".into()))?;
    crate::single_person::mode(&state, &organization)
        .await
        .map(Json)
}

pub fn router() -> Router<AppState> {
    Router::new().route("/organization/approval-mode", get(get_approval_mode))
}
