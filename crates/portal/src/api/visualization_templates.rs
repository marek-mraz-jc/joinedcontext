//! `GET /api/v1/visualization-templates/{name}`: a chart template of the Endpoint editor filled
//! with the columns a person picked (T-4642, EP-130). jc-core holds the templates, so the editor
//! and the check a proposal meets read the same file; the result is a Vega-Lite spec to save in
//! `spec.catalog.visualizations[].spec`, checked like any other on propose.

use std::collections::{BTreeMap, HashMap};

use axum::extract::{Path, Query};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::Value;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::state::AppState;

/// The roles a template places a column in.
const ROLES: [&str; 4] = ["x", "y", "color", "facet"];

#[utoipa::path(
    get,
    path = "/api/v1/visualization-templates/{name}",
    summary = "Fill a Visualization Template",
    description = "The Vega-Lite spec of the template `bar`, `line`, `heatmap`, `small-multiples` or `linked-brush` with each role's column filled in; a role left out drops the encodings that use it. 404 when the template does not exist or a role it needs is missing (T-4642).",
    tag = "endpoints",
    params(
        ("name" = String, Path, description = "The template's name"),
        ("x" = Option<String>, Query, description = "The column on the first axis"),
        ("y" = Option<String>, Query, description = "The measured column"),
        ("color" = Option<String>, Query, description = "The column that colours the marks"),
        ("facet" = Option<String>, Query, description = "The column that splits the chart into panels"),
    ),
    responses(
        (status = 200, description = "The filled spec", body = Object),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such template, or a role it needs is missing", body = ProblemDetails),
    )
)]
pub async fn fill_template(
    _user: CurrentUser,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let columns: BTreeMap<String, String> = query
        .into_iter()
        .filter(|(role, column)| ROLES.contains(&role.as_str()) && !column.is_empty())
        .collect();
    jc_core::kinds::visualization::template(&name, &columns)
        .map(Json)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "no visualization template '{name}' with the roles given"
            ))
        })
}

pub fn router() -> Router<AppState> {
    Router::new().route("/visualization-templates/{name}", get(fill_template))
}
