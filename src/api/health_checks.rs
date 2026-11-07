//! The health log (T-5067): `GET /projects/{project}/health-checks` lists the checks of one
//! project's resources the caller may read, `GET /organization/health-checks` every project's
//! for an administrator of the organization. The rows are written by the leader's alert run.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::health_log::Check;
use crate::permissions::ORG_NAMESPACE;
use crate::resource::is_dns1123;
use crate::state::AppState;

/// The checks, errors first, then warnings, then ok.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = HealthCheckList)]
pub struct HealthChecks {
    pub items: Vec<Check>,
}

fn unavailable(error: sqlx::Error) -> ApiError {
    tracing::warn!(%error, "the health log could not be read");
    ApiError::Unavailable("the health log could not be read; try again in a minute".into())
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/health-checks",
    summary = "List Project Health Checks",
    description = "Every check of the project's pipelines, Context Spaces, Endpoints, models and data sources the caller may read (T-5067): its status, since when it holds, when it was last ok, what it found with credentials masked, a fix hint and the statuses of the last 7 days. A project the caller cannot read is 404.",
    tag = "system",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The project's checks, errors first", body = HealthChecks),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "Project not found", body = ProblemDetails),
        (status = 503, description = "The health log could not be read", body = ProblemDetails)
    )
)]
pub async fn list_project(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<HealthChecks>, ApiError> {
    let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
    if !is_dns1123(&project) || !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let items = state
        .health_log
        .list(Some(&project), Utc::now())
        .await
        .map_err(unavailable)?
        .into_iter()
        .filter(|check| {
            let kind = check.kind.manifest_kind();
            match state.mirror.get(&project, kind, &check.name) {
                Some(manifest) => serde_json::to_value(&manifest)
                    .is_ok_and(|value| effective.may_read_manifest(kind, &value)),
                None => effective.may_read(kind),
            }
        })
        .collect();
    Ok(Json(HealthChecks { items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/health-checks",
    summary = "List Organization Health Checks",
    description = "Every project's checks (T-5067), errors first. Only an administrator of the organization: approve and delete on RoleBinding at organization scope (PF-03).",
    tag = "system",
    responses(
        (status = 200, description = "Every project's checks, errors first", body = HealthChecks),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an administrator of the organization", body = ProblemDetails),
        (status = 503, description = "The health log could not be read", body = ProblemDetails)
    )
)]
pub async fn list_all(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<HealthChecks>, ApiError> {
    let effective = crate::permissions::for_request(&state, &user.0.identity, ORG_NAMESPACE);
    if !effective.administers_organization() {
        return Err(ApiError::Denied(
            "only an administrator of the organization reads every project's health: it needs \
             approve and delete on RoleBinding at organization scope (PF-03)"
                .into(),
        ));
    }
    let items = state
        .health_log
        .list(None, Utc::now())
        .await
        .map_err(unavailable)?;
    Ok(Json(HealthChecks { items }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/health-checks", get(list_project))
        .route("/organization/health-checks", get(list_all))
}
