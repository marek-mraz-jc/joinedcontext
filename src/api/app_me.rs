//! The caller's roles in one published application, for its backend (AP-109, Architecture/16
//! §13).
//!
//! A `fullstack` backend never sees the static host that writes `#jc-config` (AP-95), so it asks
//! here with the edge's `X-Access-Token` as `Authorization: Bearer`: a token of the App's own
//! client `app-{name}`, whose `resource_access.app-{name}.roles` are the caller's roles, as AP-92
//! reads them for the static host. Nothing else the caller sends counts. The answer is about the
//! caller alone and names no other resource, so it needs no `read` on the project: an
//! `organization` App admits people who hold no rule in it.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use jc_core::kinds::{AppLifecycle, AppSpec};
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::{ApiError, ProblemDetails};
use crate::state::AppState;

/// The caller as a published App sees them: the object the static host writes into
/// `#jc-config` as `user` (AP-95).
#[derive(Debug, Serialize, ToSchema)]
pub struct AppMe {
    /// The Keycloak `sub`.
    pub id: String,
    pub name: String,
    pub email: Option<String>,
    /// The App's roles this person holds, in the order `spec.roles` declares them; empty when
    /// none (AP-92).
    pub roles: Vec<String>,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/projects/{project}/apps/{name}/me", get(me))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/apps/{name}/me",
    operation_id = "app_me",
    summary = "The Caller's Roles In An Application",
    description = "The caller's id, name, e-mail and roles in one published App, for the App's backend (AP-109).",
    tag = "apps",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "App name"),
    ),
    responses(
        (status = 200, description = "The caller in this App", body = AppMe),
        (status = 401, description = "No valid token", body = ProblemDetails),
        (status = 404, description = "No published App of this name in the project", body = ProblemDetails)
    )
)]
pub async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    // Who is asking comes first, so an anonymous caller learns nothing about which Apps exist.
    let verified = crate::apps::roles::verified(&state, &headers, &name).await?;
    let not_found = || ApiError::NotFound(format!("app '{name}' not found"));
    let spec: AppSpec = state
        .mirror
        .get(&project, "App", &name)
        .and_then(|app| serde_json::from_value(app.spec).ok())
        .ok_or_else(not_found)?;
    if spec.lifecycle != AppLifecycle::Published {
        return Err(not_found());
    }
    let person = crate::apps::roles::AppPerson::of(&spec, verified);
    // A person the App's visibility keeps out learns no more than of a name never created (T-4910).
    if !crate::apps::roles::may_open(&state, &project, &name, &spec, Some(&person)) {
        return Err(not_found());
    }
    let identity = &person.identity;
    // The API's own middleware marks every answer `no-store`, so no shared cache keeps one
    // person's roles for the next (AP-95).
    Ok(Json(AppMe {
        id: identity.subject.clone(),
        name: identity
            .name
            .clone()
            .unwrap_or_else(|| identity.username.clone()),
        email: identity.email.clone(),
        roles: person.roles,
    })
    .into_response())
}
