//! Workspaces over REST (API/01 §22, T-1236): each route calls the one implementation in
//! `crate::ops::workspaces`, which the operations of the registry and MCP call too.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::auth::session::Front;
use crate::auth::CurrentUser;
use crate::change::Change;
use crate::error::{ApiError, ProblemDetails};
use crate::ops::previews::{self, Preview};
use crate::ops::workspaces::{
    self, Comparison, OpenRequest, UpdateReport, UpdateRequest, WorkspaceList, WorkspaceView,
};
use crate::ops::{Caller, OpError, Via};
use crate::state::AppState;

fn caller(user: CurrentUser, front: Front) -> Caller {
    Caller::new(
        user.0.identity,
        match front {
            Front::Portal | Front::Edge => Via::Session,
            Front::Bearer => Via::Bearer,
        },
    )
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/workspaces",
    summary = "Open A Workspace",
    description = "Opens a named branch of the project to change several resources in, brought back later as one Change.",
    tag = "workspaces",
    params(("project" = String, Path, description = "Project name")),
    request_body(
        content = OpenRequest,
        example = json!({ "name": "bike-lanes", "title": "Bike lanes", "ttlDays": 7, "scope": { "kind": "space", "name": "mobility" } })
    ),
    responses(
        (status = 201, description = "The workspace, opened", body = WorkspaceView),
        (status = 400, description = "A name, title, scope or TTL out of bounds", body = ProblemDetails),
        (status = 403, description = "No role with propose in the project", body = ProblemDetails),
        (status = 404, description = "No such project", body = ProblemDetails),
        (status = 409, description = "A workspace of that name exists", body = ProblemDetails),
    )
)]
pub async fn open_workspace(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    Json(request): Json<OpenRequest>,
) -> Result<Response, ApiError> {
    let view = workspaces::open(&user.0.identity, &state, &project, request).await?;
    Ok((StatusCode::CREATED, Json(view)).into_response())
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/workspaces",
    summary = "List Workspaces",
    description = "The open workspaces of the project, oldest first.",
    tag = "workspaces",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The open workspaces", body = WorkspaceList),
        (status = 404, description = "No such project", body = ProblemDetails),
    )
)]
pub async fn list_workspaces(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<WorkspaceList>, ApiError> {
    Ok(Json(
        workspaces::list(&user.0.identity, &state, &project).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/workspaces/{name}",
    summary = "Read A Workspace",
    description = "One workspace: whose it is, what it covers, when it expires and how many files it changes.",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 200, description = "The workspace", body = WorkspaceView),
        (status = 404, description = "No such project or workspace", body = ProblemDetails),
    )
)]
pub async fn get_workspace(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<WorkspaceView>, ApiError> {
    Ok(Json(
        workspaces::get(&user.0.identity, &state, &project, &name).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/workspaces/{name}/compare",
    summary = "Compare A Workspace",
    description = "Every file the workspace changes with its fields and lane, and every field main changed too.",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 200, description = "What it changes, and where main changed the same", body = Comparison),
        (status = 404, description = "No such project or workspace", body = ProblemDetails),
    )
)]
pub async fn compare_workspace(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<Comparison>, ApiError> {
    Ok(Json(
        workspaces::compare(&user.0.identity, &state, &project, &name).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/workspaces/{name}/update",
    summary = "Update A Workspace From Main",
    description = "Takes what main changed into the workspace; a field both changed needs a resolution, ours or theirs.",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    request_body(
        content = UpdateRequest,
        example = json!({ "resolutions": [{ "path": "projects/helsinki/endpoints/helsinki-air.yaml", "field": "spec.audience", "keep": "ours" }] })
    ),
    responses(
        (status = 200, description = "Main merged into the workspace", body = UpdateReport),
        (status = 403, description = "Not the owner", body = ProblemDetails),
        (status = 409, description = "A conflict without a resolution"),
    )
)]
pub async fn update_workspace(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Json(request): Json<UpdateRequest>,
) -> Result<Json<UpdateReport>, OpError> {
    Ok(Json(
        workspaces::update_from_main(&user.0.identity, &state, &project, &name, request).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/workspaces/{name}/propose",
    summary = "Bring A Workspace Back",
    description = "Proposes the workspace as one Change a person approves; never for an agent (AG-82).",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 202, description = "The Change that brings the workspace back", body = Change),
        (status = 403, description = "Not the owner, or a kind the owner may not propose", body = ProblemDetails),
        (status = 409, description = "A conflict, nothing to bring back, or already brought back"),
    )
)]
pub async fn propose_workspace(
    user: CurrentUser,
    front: Front,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Response, OpError> {
    let change = workspaces::propose(&caller(user, front), &state, &project, &name).await?;
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/workspaces/{name}",
    summary = "Discard A Workspace",
    description = "Removes the workspace and its branch; nothing in it reaches main.",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 204, description = "The workspace and its branch are gone"),
        (status = 403, description = "Not the owner", body = ProblemDetails),
        (status = 404, description = "No such project or workspace", body = ProblemDetails),
    )
)]
pub async fn discard_workspace(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    workspaces::discard(&user.0.identity, &state, &project, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/workspaces/{name}/preview",
    summary = "Start A Workspace Preview",
    description = "Renders the workspace with its prefix and serves its Endpoints on slugs of their own; every pipeline stays paused (CC-78, PF-83).",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 202, description = "The preview, running", body = Preview),
        (status = 403, description = "Not the workspace's owner", body = ProblemDetails),
        (status = 404, description = "No such workspace", body = ProblemDetails),
        (status = 409, description = "Running already, two run on the node, or the render is refused", body = ProblemDetails),
    )
)]
pub async fn start_workspace_preview(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let preview = previews::start(&user.0.identity, &state, &project, &name).await?;
    Ok((StatusCode::ACCEPTED, Json(preview)).into_response())
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/workspaces/{name}/preview",
    summary = "Read A Workspace Preview",
    description = "Whether the preview runs, the addresses of its Endpoints, its paused pipelines, and why it failed when it did.",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 200, description = "The preview", body = Preview),
        (status = 404, description = "No such workspace", body = ProblemDetails),
    )
)]
pub async fn get_workspace_preview(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<Preview>, ApiError> {
    Ok(Json(
        previews::get(&user.0.identity, &state, &project, &name).await?,
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/workspaces/{name}/preview",
    summary = "Stop A Workspace Preview",
    description = "Stops the preview; its Endpoints stop answering. Stopping one that does not run changes nothing.",
    tag = "workspaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Workspace name"),
    ),
    responses(
        (status = 204, description = "The preview stopped, or was not running"),
        (status = 403, description = "Not the workspace's owner", body = ProblemDetails),
        (status = 404, description = "No such workspace", body = ProblemDetails),
    )
)]
pub async fn stop_workspace_preview(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    previews::stop(&user.0.identity, &state, &project, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/workspaces",
            get(list_workspaces).post(open_workspace),
        )
        .route(
            "/projects/{project}/workspaces/{name}",
            get(get_workspace).delete(discard_workspace),
        )
        .route(
            "/projects/{project}/workspaces/{name}/compare",
            get(compare_workspace),
        )
        .route(
            "/projects/{project}/workspaces/{name}/update",
            post(update_workspace),
        )
        .route(
            "/projects/{project}/workspaces/{name}/preview",
            get(get_workspace_preview)
                .post(start_workspace_preview)
                .delete(stop_workspace_preview),
        )
        .route(
            "/projects/{project}/workspaces/{name}/propose",
            post(propose_workspace),
        )
}
