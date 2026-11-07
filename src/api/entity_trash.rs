//! The trash of a space's entities (API/01 §31, ADR-N-042 §3.3, T-3107): each person keeps a copy
//! of what they delete from a data view and restores it with their own create through the gateway.
//! The Portal holds the copy and nothing else: the delete and the restore are the person's writes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::entity_trash::{TrashItem, MAX_ENTITY_BYTES};
use crate::error::ApiError;
use crate::state::AppState;

/// The copy of one entity about to be deleted.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct KeepRequest {
    /// The NGSI-LD entity as the person read it, normalized.
    #[schema(value_type = Object)]
    pub entity: Value,
}

/// The space, when the caller may read it; `404` for one they may not, as every space route.
pub(crate) fn readable(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    space: &str,
) -> Result<(), ApiError> {
    crate::api::data_views::readable_space(state, &user.0.identity, project, space).map(|_| ())
}

fn stored(err: sqlx::Error) -> ApiError {
    tracing::warn!(error = %err, "entity trash not reachable");
    ApiError::Unavailable("the trash could not be read or written".into())
}

/// An NGSI-LD URN of its own type and an NGSI-LD type name (PF-43, ADR-N-041).
fn urn_and_type(entity: &Value) -> Result<(String, String), ApiError> {
    let id = entity.get("id").and_then(Value::as_str).unwrap_or_default();
    let kind = entity
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let type_ok = !kind.is_empty()
        && kind.len() <= 256
        && kind.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && kind
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !type_ok {
        return Err(ApiError::BadRequest(
            "entity.type must be an NGSI-LD type name".into(),
        ));
    }
    let prefix = format!("urn:ngsi-ld:{kind}:");
    let rest = id.strip_prefix(&prefix).unwrap_or_default();
    if rest.is_empty() || id.len() > 1024 || id.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(ApiError::BadRequest(format!(
            "entity.id must be an NGSI-LD URN of its type, {prefix}{{id}} (PF-43)"
        )));
    }
    Ok((id.to_owned(), kind.to_owned()))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/trash",
    summary = "List My Deleted Entities",
    description = "The caller's own copies of the entities they deleted from this space's data views, newest first, kept 30 days (API/01 §31).",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    responses(
        (status = 200, description = "The caller's copies", body = [TrashItem]),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "The trash is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_trash(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
) -> Result<Json<Vec<TrashItem>>, ApiError> {
    readable(&state, &user, &project, &space)?;
    let owner = user.0.identity.subject.clone();
    state
        .trash
        .list((&project, &space, &owner))
        .await
        .map(Json)
        .map_err(stored)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/spaces/{space}/trash",
    summary = "Keep A Copy Before Deleting",
    description = "Keeps the caller's own copy of one entity they are about to delete through the gateway (API/01 §31). The copy grants nothing: restoring it is the caller's own create.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    request_body(
        content = KeepRequest,
        description = "The entity as the person read it, normalized.",
        content_type = "application/json",
        example = json!({
            "entity": {
                "id": "urn:ngsi-ld:BikeHireDockingStation:hel.fi:bikes:7",
                "type": "BikeHireDockingStation",
                "name": { "type": "Property", "value": "Station 7" }
            }
        })
    ),
    responses(
        (status = 201, description = "The copy", body = TrashItem),
        (status = 400, description = "Not an NGSI-LD entity, or larger than 256 KiB", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = crate::error::ProblemDetails),
        (status = 503, description = "The trash is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn keep(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
    Json(request): Json<KeepRequest>,
) -> Result<(StatusCode, Json<TrashItem>), ApiError> {
    readable(&state, &user, &project, &space)?;
    let size = serde_json::to_vec(&request.entity)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    if size > MAX_ENTITY_BYTES {
        return Err(ApiError::BadRequest(format!(
            "the entity is {size} bytes; the trash keeps at most {MAX_ENTITY_BYTES}"
        )));
    }
    let (urn, entity_type) = urn_and_type(&request.entity)?;
    let owner = user.0.identity.subject.clone();
    let item = state
        .trash
        .keep(
            (&project, &space, &owner),
            &urn,
            &entity_type,
            &request.entity,
        )
        .await
        .map_err(stored)?;
    Ok((StatusCode::CREATED, Json(item)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/spaces/{space}/trash/{id}",
    summary = "Forget A Copy",
    description = "Forgets one of the caller's copies: the entity was restored, or its delete was refused (API/01 §31). Another caller's copy is 404.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
        ("id" = i64, Path, description = "The copy's id"),
    ),
    responses(
        (status = 204, description = "Forgotten"),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 404, description = "No such copy of the caller's, or no such space", body = crate::error::ProblemDetails),
        (status = 503, description = "The trash is not reachable", body = crate::error::ProblemDetails),
    )
)]
pub async fn forget(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space, id)): Path<(String, String, i64)>,
) -> Result<StatusCode, ApiError> {
    readable(&state, &user, &project, &space)?;
    let owner = user.0.identity.subject.clone();
    if state
        .trash
        .forget((&project, &space, &owner), id)
        .await
        .map_err(stored)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!(
            "no deleted entity {id} of yours here"
        )))
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/spaces/{space}/trash",
            get(list_trash).post(keep),
        )
        .route(
            "/projects/{project}/spaces/{space}/trash/{id}",
            delete(forget),
        )
}
