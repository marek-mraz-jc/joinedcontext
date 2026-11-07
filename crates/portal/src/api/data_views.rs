//! `/api/v1/projects/{project}/spaces/{space}/views`: saved data views of a space (API/01 §30,
//! ADR-N-042 §3.2, T-3104).
//!
//! Reading the space is all a view asks: it changes nothing in the space, and the rows it shows are
//! read with the person's own session. Who sees and changes one view is its mode's to say.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use jc_core::kinds::Verb;
use serde::de::DeserializeOwned;
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::ops::data_views::{
    access, is_id, Access, CreateView, DataView, DataViewError, Owner, UpdateView,
};
use crate::permissions::Effective;
use crate::resource::is_dns1123;
use crate::state::AppState;

#[derive(Debug, Serialize, ToSchema)]
pub struct ViewList {
    /// The views of the space this caller sees, oldest first.
    pub items: Vec<DataView>,
}

impl From<DataViewError> for ApiError {
    fn from(err: DataViewError) -> Self {
        match err {
            DataViewError::Invalid(detail) => ApiError::BadRequest(detail),
            DataViewError::NotFound => ApiError::NotFound("no such view in this space".into()),
            DataViewError::Conflict(_) | DataViewError::Limit => {
                ApiError::Conflict(err.to_string())
            }
            DataViewError::Db(detail) => {
                // The SQL and the driver's wording stay in the log (TS-09).
                tracing::error!(error = %detail, "data views database call failed");
                ApiError::Internal("the data views database did not answer".into())
            }
        }
    }
}

/// The caller's rights in a space they may read; `404` for a project or space they may not, as
/// the other space routes answer (API/01 §27).
pub(crate) fn readable_space(
    state: &AppState,
    identity: &Identity,
    project: &str,
    space: &str,
) -> Result<Effective, ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
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
    Ok(effective)
}

fn access_to(view: &DataView, identity: &Identity, effective: &Effective, space: &str) -> Access {
    let owner = view.owner_subject == identity.subject;
    let steward = effective.may_in("ContextSpace", Verb::Update, Some(space));
    access(&view.mode, owner, steward)
}

/// A view the caller sees; a personal view of someone else is as absent as no view.
async fn visible(
    state: &AppState,
    identity: &Identity,
    project: &str,
    space: &str,
    id: &str,
) -> Result<(DataView, Access), ApiError> {
    let effective = readable_space(state, identity, project, space)?;
    if !is_id(id) {
        return Err(DataViewError::NotFound.into());
    }
    let view = state.data_views.get(project, space, id).await?;
    let rights = access_to(&view, identity, &effective, space);
    if !rights.see {
        return Err(DataViewError::NotFound.into());
    }
    Ok((view, rights))
}

/// Parsed by hand, as the other write handlers here are: a bad body is problem+json `400`, never
/// axum's plain-text `422`.
fn body<T: DeserializeOwned>(bytes: &Bytes) -> Result<T, ApiError> {
    serde_json::from_slice(bytes)
        .map_err(|e| ApiError::BadRequest(format!("the view body is invalid: {e}")))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/views",
    summary = "List Data Views",
    description = "The saved views of one space this caller sees: their own personal views and every collaborative or locked one, oldest first.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    responses(
        (status = 200, description = "The views this caller sees", body = ViewList),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = ProblemDetails),
    )
)]
pub async fn list_views(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
) -> Result<Json<ViewList>, ApiError> {
    let identity = &user.0.identity;
    let effective = readable_space(&state, identity, &project, &space)?;
    let items = state
        .data_views
        .list(&project, &space)
        .await?
        .into_iter()
        .filter(|view| access_to(view, identity, &effective, &space).see)
        .collect();
    Ok(Json(ViewList { items }))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/spaces/{space}/views",
    summary = "Save A Data View",
    description = "Saves a new view of one entity type of the space, owned by the caller. Reading the space is all it needs.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
    ),
    request_body(
        content = CreateView,
        example = json!({
            "type": "BikeHireDockingStation",
            "kind": "grid",
            "mode": "collaborative",
            "title": "Stations short of bikes",
            "config": { "q": "availableBikeNumber<3", "hidden": ["dateLastReported"], "colour": [{ "when": "availableBikeNumber==0", "colour": "danger" }] }
        })
    ),
    responses(
        (status = 201, description = "The view, saved", body = DataView),
        (status = 400, description = "A field out of bounds or an unknown key", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Missing or mismatched CSRF token", body = ProblemDetails),
        (status = 404, description = "No such space the caller may read", body = ProblemDetails),
        (status = 409, description = "The caller keeps the most views a person may in this space", body = ProblemDetails),
    )
)]
pub async fn create_view(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space)): Path<(String, String)>,
    bytes: Bytes,
) -> Result<Response, ApiError> {
    let identity = &user.0.identity;
    readable_space(&state, identity, &project, &space)?;
    let request: CreateView = body(&bytes)?;
    let owner = Owner {
        subject: &identity.subject,
        name: &identity.username,
    };
    let view = state
        .data_views
        .create(&project, &space, owner, request)
        .await?;
    Ok((StatusCode::CREATED, Json(view)).into_response())
}

#[utoipa::path(
    put,
    path = "/api/v1/projects/{project}/spaces/{space}/views/{id}",
    summary = "Change A Data View",
    description = "Renames a view or changes its kind, mode or settings. A locked view is changed by its owner or a steward of the space, and so is any view's mode. With `expectedVersion`, a save against a newer view is refused.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
        ("id" = String, Path, description = "The view's id"),
    ),
    request_body(
        content = UpdateView,
        example = json!({ "kind": "grid", "mode": "collaborative", "title": "Stations short of bikes", "config": { "q": "availableBikeNumber<2" }, "expectedVersion": 3 })
    ),
    responses(
        (status = 200, description = "The view as now saved", body = DataView),
        (status = 400, description = "A field out of bounds or an unknown key", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A locked view, or its mode, is its owner's or a steward's to change; or a missing CSRF token", body = ProblemDetails),
        (status = 404, description = "No such view the caller sees", body = ProblemDetails),
        (status = 409, description = "The view was changed since `expectedVersion`", body = ProblemDetails),
    )
)]
pub async fn update_view(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space, id)): Path<(String, String, String)>,
    bytes: Bytes,
) -> Result<Json<DataView>, ApiError> {
    let (view, rights) = visible(&state, &user.0.identity, &project, &space, &id).await?;
    let request: UpdateView = body(&bytes)?;
    if !rights.change {
        return Err(ApiError::Denied(
            "this view is locked: only its owner or a steward of the space changes it".into(),
        ));
    }
    if request.mode != view.mode && !rights.govern {
        return Err(ApiError::Denied(
            "only the view's owner or a steward of the space changes who sees it".into(),
        ));
    }
    Ok(Json(
        state
            .data_views
            .update(&project, &space, &id, request)
            .await?,
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/spaces/{space}/views/{id}",
    summary = "Delete A Data View",
    description = "Deletes a saved view: its owner's, or for a shared one also a steward's of the space. The space's data is not touched.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
        ("id" = String, Path, description = "The view's id"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Neither its owner nor a steward of the space; or a missing CSRF token", body = ProblemDetails),
        (status = 404, description = "No such view the caller sees", body = ProblemDetails),
    )
)]
pub async fn delete_view(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space, id)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let (_, rights) = visible(&state, &user.0.identity, &project, &space, &id).await?;
    if !rights.govern {
        return Err(ApiError::Denied(
            "only the view's owner or a steward of the space deletes it".into(),
        ));
    }
    state.data_views.delete(&project, &space, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/spaces/{space}/views",
            get(list_views).post(create_view),
        )
        .route(
            "/projects/{project}/spaces/{space}/views/{id}",
            put(update_view).delete(delete_view),
        )
}
