//! The attributes of an entity type in a space's data model (T-3223): what the pipeline editor's
//! output node maps a record's fields onto, read from the model the space pins (DM-61) through the
//! same compiled schema the runner's validation stage checks every record against (PL-59).

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::pipeline_validation::ClassAttributes;
use crate::state::AppState;

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/spaces/{space}/types/{type}/attributes",
    summary = "Read A Type's Attributes",
    description = "The attributes an entity type has in the data model the space pins: kind, value type, whether it is required, its unit, description, coded values and a relationship's target type. Required attributes come first. Reading the space is all it needs.",
    tag = "spaces",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "Context Space name"),
        ("type" = String, Path, description = "The entity type, a class of the space's model"),
    ),
    responses(
        (status = 200, description = "The type and its attributes", body = ClassAttributes),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such space the caller may read, a space without a data model, or a type the model does not declare", body = ProblemDetails),
    )
)]
pub async fn type_attributes(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, space, entity_type)): Path<(String, String, String)>,
) -> Result<Json<ClassAttributes>, ApiError> {
    crate::api::data_views::readable_space(&state, &user.0.identity, &project, &space)?;
    let model = state.model_schemas.get(&project, &space).ok_or_else(|| {
        ApiError::NotFound(format!(
            "context space '{space}' names no data model this Portal holds, so its types have no attributes to list (DM-61)"
        ))
    })?;
    model.attributes(&entity_type).map(Json).ok_or_else(|| {
        ApiError::NotFound(format!(
            "{entity_type} is not a class of the space's data model {} (DM-61)",
            model.model
        ))
    })
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/spaces/{space}/types/{type}/attributes",
        get(type_attributes),
    )
}
