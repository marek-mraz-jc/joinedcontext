//! `GET /api/v1/app-templates`: the gallery of App templates a person starts from (T-3263,
//! AP-141). The same samples the build run adapts (SDK-24), each with its purpose and the data it
//! needs; a run started from one names it in its prompt as `(template: {name})`.

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use rust_embed::RustEmbed;
use serde::Serialize;
use utoipa::ToSchema;

use crate::agents::samples::{self, Template};
use crate::auth::CurrentUser;
use crate::error::ProblemDetails;
use crate::state::AppState;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppTemplates {
    /// Every template, by name.
    pub templates: Vec<Template>,
    /// Whether "Generate" can answer now: an agent runner is configured and the model key was
    /// not last reported refused or out of credit (AG-96). When it is `false`, a template is
    /// still used as is (T-5617).
    pub model_available: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/app-templates",
    summary = "List App Templates",
    description = "The App templates of this Portal, each with what it is for, for whom and the data it needs. Starting a run whose prompt names `(template: {name})` builds from that template.",
    tag = "apps",
    responses(
        (status = 200, description = "Every template", body = AppTemplates),
        (status = 401, description = "Unauthorized", body = ProblemDetails)
    )
)]
pub async fn list_templates(
    _user: CurrentUser,
    State(state): State<AppState>,
) -> Json<AppTemplates> {
    Json(AppTemplates {
        templates: samples::templates(),
        model_available: state.config.agent_settings.is_some()
            && crate::api::internal::model_key::builder_answers(),
    })
}

/// The screenshots the SDK's demo journey took of each template (T-3306): committed beside its
/// `sample.json`, checked against its sources by `sdk/tests/template_screenshots.test.ts`.
#[derive(RustEmbed)]
#[folder = "sdk/samples"]
#[include = "*/screenshot-*.png"]
struct Screenshots;

#[utoipa::path(
    get,
    path = "/api/v1/app-templates/{name}/screenshot/{width}",
    summary = "Read an App Template's Screenshot",
    description = "The template as its live demo shows it, at 1440 or 375 pixels wide (T-3306).",
    tag = "apps",
    params(
        ("name" = String, Path, description = "A template's name"),
        ("width" = u32, Path, description = "1440 or 375")
    ),
    responses(
        (status = 200, description = "A PNG", content_type = "image/png", body = Vec<u8>),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such template or width", body = ProblemDetails)
    )
)]
pub async fn screenshot(_user: CurrentUser, Path((name, width)): Path<(String, u32)>) -> Response {
    let known = samples::templates()
        .iter()
        .any(|template| template.name == name);
    let Some(png) = (known && matches!(width, 1440 | 375))
        .then(|| Screenshots::get(&format!("{name}/screenshot-{width}.png")))
        .flatten()
    else {
        return crate::error::ApiError::NotFound(format!("no screenshot of '{name}' at {width}"))
            .into_response();
    };
    let mut response = (StatusCode::OK, png.data.into_owned()).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    response
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/app-templates", get(list_templates))
        .route("/app-templates/{name}/screenshot/{width}", get(screenshot))
}
