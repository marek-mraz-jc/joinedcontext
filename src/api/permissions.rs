//! `GET /api/v1/projects/{project}/permissions/me` (T-0526, PF-50): what the caller may do in
//! one project, so the UI renders only the controls the API would honour.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use crate::auth::session::CurrentUser;
use crate::error::ApiError;
use crate::permissions::{self, Effective};
use crate::state::AppState;

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/permissions/me",
    summary = "Read My Permissions",
    description = "The caller's effective rules in the project: which verbs on which kinds, from which bindings. 404 when no binding covers the project.",
    tag = "permissions",
    params(("project" = String, Path, description = "Project slug")),
    responses(
        (status = 200, description = "The caller's effective rules in the project", body = Effective),
        (status = 401, description = "Unauthorized", body = crate::error::ProblemDetails),
        (status = 404, description = "No binding of the caller covers the project", body = crate::error::ProblemDetails),
    )
)]
pub async fn permissions_me(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<Effective>, ApiError> {
    let mut effective = permissions::for_request(&state, &user.0.identity, &project);
    // A project no binding of the caller covers reads like a project that is not there, here
    // as on every other read (PF-59, R20); the UI then renders the controls disabled.
    if !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    // Opening a project is the organization's own setting, not a binding, so the rules alone
    // cannot answer it and the UI would have to guess (PF-65, UI-44, T-0870).
    effective.projects = Some(permissions::ProjectAffordances {
        creation: crate::api::projects::creation_affordance(&state, &user.0.identity),
    });
    Ok(Json(effective))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/projects/{project}/permissions/me", get(permissions_me))
}
