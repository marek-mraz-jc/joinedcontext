//! `POST|GET /api/v1/projects/{project}/spaces/{space}/move`: moving a space to another broker
//! (API/01 §45, PF-115, ADR-N-047 §2.6), for organization administrators.

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::auth::session::CurrentUser;
use crate::error::ApiError;
use crate::space_move::{Move, Plan};
use crate::state::AppState;

/// Where the space goes.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveRequest {
    /// A `ContextBroker` of the organization other than the one the space lives on.
    pub to: String,
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/spaces/{space}/move",
    summary = "Move a Space to Another Broker",
    description = "Holds the space read-only, copies it with its history to the broker `to`, compares both digests and switches its brokerRef (API/01 §45, PF-115). Organization administrators only.",
    tag = "spaces",
    params(("project" = String, Path), ("space" = String, Path)),
    request_body(
        content = MoveRequest,
        description = "The broker the space goes to.",
        content_type = "application/json",
        example = json!({ "to": "canary" })
    ),
    responses(
        (status = 202, description = "The move started", body = Move),
        (status = 400, description = "No such broker, or the one the space lives on", body = crate::error::ProblemDetails),
        (status = 403, description = "Not an organization administrator", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space", body = crate::error::ProblemDetails),
        (status = 409, description = "The space is already moving", body = crate::error::ProblemDetails),
        (status = 503, description = "A broker, the gateway or the forge is not configured", body = crate::error::ProblemDetails),
    )
)]
pub async fn start_move(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
    body: Result<Json<MoveRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Move>), ApiError> {
    crate::permissions::require_organization_admin(&state, &user.0.identity, "moving a space")?;
    // Read after the right: a caller who may not move is told so, whatever they sent.
    let Json(request) = body.map_err(|err| {
        ApiError::BadRequest(format!(
            "the body is not a move ({err}); send {{\"to\": \"<broker>\"}}"
        ))
    })?;
    let envelope = state
        .mirror
        .get(&project, "ContextSpace", &space)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "context space '{space}' not found in project '{project}'"
            ))
        })?;
    let from = crate::spaces::broker(&state.mirror, &project, &space);
    let declared = state
        .mirror
        .find(|env| env.kind == "ContextBroker" && env.metadata.name == request.to)
        .is_some();
    if !declared {
        return Err(ApiError::BadRequest(format!(
            "no ContextBroker is named '{}'; pick one from Administration → Brokers",
            request.to
        )));
    }
    if from == request.to {
        return Err(ApiError::BadRequest(format!(
            "space '{space}' already lives on broker '{}'",
            request.to
        )));
    }
    let unavailable = |what: &str| ApiError::Unavailable(format!("{what}, so no space can move"));
    let counter = &state.space_usage;
    let source = counter
        .address(&from)
        .ok_or_else(|| unavailable(&format!("the Portal has no address for broker '{from}'")))?;
    let target = counter.address(&request.to).ok_or_else(|| {
        unavailable(&format!(
            "the Portal has no address for broker '{}'",
            request.to
        ))
    })?;
    let token = counter.token().map_err(|reason| unavailable(&reason))?;
    let gateway =
        state.config.gateway_url.clone().ok_or_else(|| {
            unavailable("the Portal has no gateway address (JC_PORTAL_GATEWAY_URL)")
        })?;
    let forge = state
        .forge_for(&project)
        .ok_or_else(|| unavailable("the Portal has no forge"))?;
    let kind = crate::resource::by_kind("ContextSpace")
        .ok_or_else(|| ApiError::Internal("ContextSpace is not in the catalogue".into()))?;
    let path = crate::api::mutate::resolve_repo_path(&envelope, kind, &project)?;
    let plan = Plan {
        tenant: crate::spaces::segment(&state.mirror, &project, &space),
        project,
        space,
        from,
        to: request.to,
        source: source.trim_end_matches('/').to_owned(),
        target: target.trim_end_matches('/').to_owned(),
        token,
        gateways: vec![state
            .config
            .gateway_pods_url
            .clone()
            .unwrap_or(gateway)
            .trim_end_matches('/')
            .to_owned()],
        forge,
        path,
        by: user.0.identity.username.clone(),
    };
    Arc::clone(&state.space_moves)
        .start(plan)
        .map(|job| (StatusCode::ACCEPTED, Json(job)))
        .map_err(ApiError::Conflict)
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/move",
    summary = "Read a Space's Move",
    description = "The last move of the space since the Portal started (API/01 §45). Organization administrators only.",
    tag = "spaces",
    params(("project" = String, Path), ("space" = String, Path)),
    responses(
        (status = 200, description = "The move", body = Move),
        (status = 403, description = "Not an organization administrator", body = crate::error::ProblemDetails),
        (status = 404, description = "No move of this space", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_move(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
) -> Result<Json<Move>, ApiError> {
    crate::permissions::require_organization_admin(&state, &user.0.identity, "a space's move")?;
    state
        .space_moves
        .get(&format!("{project}/{space}"))
        .map(Json)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "space '{space}' of project '{project}' has not moved"
            ))
        })
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/spaces/{space}/move",
        post(start_move).get(get_move),
    )
}
