//! `/api/v1/projects/{project}/pipelinetemplates`: the pipelines a project saved to start the
//! next one from (API/01 §42, T-3976). A record of the Portal, not a kind: saving one is no
//! Change; using one fills the pipeline form, which is checked and proposed like any other.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use jc_core::kinds::Verb;
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::ops::data_views::is_id;
use crate::ops::pipeline_templates::{CreateTemplate, PipelineTemplate, TemplateError};
use crate::resource::is_dns1123;
use crate::state::AppState;

#[derive(Debug, Serialize, ToSchema)]
pub struct TemplateList {
    /// The project's templates, newest first.
    pub items: Vec<PipelineTemplate>,
}

impl From<TemplateError> for ApiError {
    fn from(err: TemplateError) -> Self {
        match err {
            TemplateError::Invalid(_) | TemplateError::TooLarge => {
                ApiError::BadRequest(err.to_string())
            }
            TemplateError::Limit => ApiError::Conflict(err.to_string()),
            TemplateError::NotFound => ApiError::NotFound(err.to_string()),
            TemplateError::Db(detail) => {
                tracing::error!(error = %detail, "pipeline templates database call failed");
                ApiError::Internal("the pipeline templates database did not answer".into())
            }
        }
    }
}

/// `404` unless the caller reads the project's pipelines; with `verb`, `403` unless they hold it.
fn allowed(
    state: &AppState,
    identity: &Identity,
    project: &str,
    verb: Option<Verb>,
) -> Result<(), ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    if !is_dns1123(project) || !effective.may_read_project() || !effective.may_read("Pipeline") {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    if let Some(verb) = verb {
        if !effective.may("Pipeline", verb) {
            return Err(ApiError::Denied(format!(
                "pipeline templates need `{verb:?}` on Pipeline in project '{project}'"
            )));
        }
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/pipelinetemplates",
    summary = "List Pipeline Templates",
    description = "The pipelines this project saved to start from, newest first (T-3976).",
    tag = "pipelines",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "The project's templates", body = TemplateList),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No project whose pipelines the caller reads", body = ProblemDetails),
    )
)]
pub async fn list_pipeline_templates(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Json<TemplateList>, ApiError> {
    allowed(&state, &user.0.identity, &project, None)?;
    let items = state.pipeline_templates.list(&project).await?;
    Ok(Json(TemplateList { items }))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/pipelinetemplates",
    summary = "Save a Pipeline Template",
    description = "Keeps a Pipeline manifest's spec without its secretRefs, its source and target references and its space, as a template of the project. A credential written out is refused (T-3976).",
    tag = "pipelines",
    params(("project" = String, Path, description = "Project name")),
    request_body(
        content = CreateTemplate,
        example = json!({
            "title": "MQTT sensor feed",
            "manifest": {
                "apiVersion": "joinedcontext.com/v1alpha2",
                "kind": "Pipeline",
                "metadata": { "name": "aq-mqtt-ingest", "namespace": "banskabystrica" },
                "spec": {
                    "class": "resident",
                    "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "aq-mqtt" } }],
                    "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:air", "type": "AirQualityObserved", "mode": "upsert" }],
                    "secretRefs": [{ "name": "mqtt-credentials", "key": "password", "envVar": "MQTT_PASSWORD" }]
                }
            }
        }),
    ),
    responses(
        (status = 201, description = "The saved template", body = PipelineTemplate),
        (status = 400, description = "Not a Pipeline, a credential written out, no title or too large", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "No `propose` on Pipeline", body = ProblemDetails),
        (status = 404, description = "No project whose pipelines the caller reads", body = ProblemDetails),
        (status = 409, description = "The project holds the most templates it may", body = ProblemDetails),
    )
)]
pub async fn create_pipeline_template(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    bytes: Bytes,
) -> Result<(StatusCode, Json<PipelineTemplate>), ApiError> {
    let identity = &user.0.identity;
    allowed(&state, identity, &project, Some(Verb::Propose))?;
    let request: CreateTemplate = serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::BadRequest(format!("the template body is invalid: {e}")))?;
    let by = identity.name.as_deref().unwrap_or(&identity.username);
    let template = state
        .pipeline_templates
        .create(&project, by, request)
        .await?;
    Ok((StatusCode::CREATED, Json(template)))
}

#[utoipa::path(
    delete,
    path = "/api/v1/projects/{project}/pipelinetemplates/{id}",
    summary = "Delete a Pipeline Template",
    description = "Removes one template of the project; pipelines made from it are not touched (T-3976).",
    tag = "pipelines",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "The template's id"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "No `propose` on Pipeline", body = ProblemDetails),
        (status = 404, description = "No such template in a project the caller reads", body = ProblemDetails),
    )
)]
pub async fn delete_pipeline_template(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    allowed(&state, &user.0.identity, &project, Some(Verb::Propose))?;
    if !is_id(&id) {
        return Err(TemplateError::NotFound.into());
    }
    state.pipeline_templates.delete(&project, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/pipelinetemplates",
            get(list_pipeline_templates).post(create_pipeline_template),
        )
        .route(
            "/projects/{project}/pipelinetemplates/{id}",
            delete(delete_pipeline_template),
        )
}
