//! `GET /api/v1/projects/{project}/spaces/{space}/usage`: how much a space holds, as the broker
//! counts it (API/01 §29, T-2889), and `POST .../usage/recalculate`: the same reading measured
//! again for a reader who asked (T-6208); `GET .../tenant`: the broker Tenant the space's data is
//! filed under, read-only (T-7216, ARCH/TEN-10).

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::resource::is_dns1123;
use crate::space_usage::SpaceUsage;
use crate::state::AppState;

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/usage",
    summary = "Read Space Usage",
    description = "The entity count and the storage in bytes of one space, read from the broker that holds it and kept: five minutes for a space under 10 000 rows, an hour for a larger one, which the Portal measures once an hour. `measuredAt` says when the size was measured. `0` entities and no `bytes` for a space the broker has no tenant for yet. The same count for everyone who reads the space.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    responses(
        (status = 200, description = "The space's usage", body = crate::space_usage::SpaceUsage),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "No broker or admin token is configured, or the broker did not answer or refused", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_usage(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
) -> Result<Json<SpaceUsage>, ApiError> {
    let (broker, tenant) = readable(&state, &user, &project, &space)?;
    state
        .space_usage
        .usage(&broker, &tenant)
        .await
        .map(Json)
        .map_err(ApiError::Unavailable)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/spaces/{space}/usage/recalculate",
    summary = "Recalculate Space Usage",
    description = "Measures one space's entity count and size again on its broker, for anyone who may read the space. Concurrent asks share one broker read, and an answer under 30 s old stands. The broker keeps a size it measured for five minutes, so `measuredAt` can be older than the ask.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    responses(
        (status = 200, description = "The space's usage, measured now", body = crate::space_usage::SpaceUsage),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "No CSRF token", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "No broker or admin token is configured, or the broker did not answer or refused", body = crate::error::ProblemDetails),
    )
)]
pub async fn recalculate_usage(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
) -> Result<Json<SpaceUsage>, ApiError> {
    let (broker, tenant) = readable(&state, &user, &project, &space)?;
    state
        .space_usage
        .recalculate(&broker, &tenant)
        .await
        .map(Json)
        .map_err(ApiError::Unavailable)
}

/// The broker Tenant of one space: the name the broker files its data under, or why it is not
/// known yet (a project the registry has no id for yet).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpaceTenant {
    /// The Tenant, e.g. `helsinki-air`, or `{OrgId}__{ProjectId}__air` under project-key names.
    pub tenant: Option<String>,
    /// Why there is no Tenant yet; `None` when `tenant` is set.
    pub reason: Option<String>,
}

impl From<Result<String, String>> for SpaceTenant {
    fn from(tenant: Result<String, String>) -> Self {
        match tenant {
            Ok(tenant) => Self {
                tenant: Some(tenant),
                reason: None,
            },
            Err(reason) => Self {
                tenant: None,
                reason: Some(reason),
            },
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/tenant",
    summary = "Read Space Tenant",
    description = "The broker Tenant the space's data is filed under: its URN segment, or `{OrgId}__{ProjectId}__{space}` once the broker's Tenants were renamed to project-key names. A Tenant that is not known yet answers 200 with `reason` and no `tenant`. Read-only, for anyone who may read the space.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    responses(
        (status = 200, description = "The space's Tenant, or why it is not known yet", body = SpaceTenant),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_tenant(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
) -> Result<Json<SpaceTenant>, ApiError> {
    may_read(&state, &user, &project, &space)?;
    Ok(Json(
        crate::spaces::tenant(&state.mirror, &project, &space).into(),
    ))
}

/// `Ok` when the caller may read the space; 404 otherwise, the same for a space that does not
/// exist, so a refusal says nothing about what is there.
fn may_read(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    space: &str,
) -> Result<(), ApiError> {
    let effective = crate::permissions::for_request(state, &user.0.identity, project);
    let readable = is_dns1123(project)
        && is_dns1123(space)
        && effective.may_read_project()
        && effective.may_read_in("ContextSpace", Some(space))
        && state.mirror.get(project, "ContextSpace", space).is_some();
    if !readable {
        return Err(ApiError::NotFound(format!(
            "context space '{space}' not found in project '{project}'"
        )));
    }
    Ok(())
}

/// The space's broker and tenant when the caller may read the space ([`may_read`]).
fn readable(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    space: &str,
) -> Result<(String, String), ApiError> {
    may_read(state, user, project, space)?;
    Ok((
        crate::spaces::broker(&state.mirror, project, space),
        crate::spaces::tenant(&state.mirror, project, space).map_err(ApiError::Unavailable)?,
    ))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/spaces/{space}/usage", get(get_usage))
        .route("/projects/{project}/spaces/{space}/tenant", get(get_tenant))
        .route(
            "/projects/{project}/spaces/{space}/usage/recalculate",
            post(recalculate_usage),
        )
}

#[cfg(test)]
mod tests {
    use super::SpaceTenant;

    #[test]
    fn a_tenant_not_known_yet_is_its_reason_and_no_name() {
        assert_eq!(
            SpaceTenant::from(Err("no id yet".to_owned())),
            SpaceTenant {
                tenant: None,
                reason: Some("no id yet".to_owned())
            }
        );
        assert_eq!(
            SpaceTenant::from(Ok("helsinki-air".to_owned()))
                .tenant
                .as_deref(),
            Some("helsinki-air")
        );
    }
}
