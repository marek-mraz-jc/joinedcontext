//! `GET /api/v1/projects/{project}/spaces/{space}/usage`: how much a space holds, as the broker
//! counts it (API/01 §29, T-2889), and `POST .../usage/recalculate`: the same reading measured
//! again for a reader who asked (T-6208).

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};

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

/// The space's broker and tenant when the caller may read the space; 404 otherwise, the same for
/// a space that does not exist, so a refusal says nothing about what is there.
fn readable(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    space: &str,
) -> Result<(String, String), ApiError> {
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
    Ok((
        crate::spaces::broker(&state.mirror, project, space),
        crate::spaces::segment(&state.mirror, project, space),
    ))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/spaces/{space}/usage", get(get_usage))
        .route(
            "/projects/{project}/spaces/{space}/usage/recalculate",
            post(recalculate_usage),
        )
}
