//! Sharing an App (AP-173, AP-174): who each visibility reaches, for the Share dialog, and the
//! Apps other projects shared with the caller's organization, for the Portal home.
//!
//! Neither route writes: a visibility changes only through a `Change` of `spec.visibility`, the
//! Portal's one write path. The list holds an App only when the one check of every door,
//! [`crate::apps::roles::may_open`], admits the caller, so it names nothing they cannot open.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use jc_core::kinds::{AppLifecycle, AppSpec, AppVisibility};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::session::{CurrentUser, Identity};
use crate::error::{ApiError, ProblemDetails};
use crate::state::AppState;
use crate::store::ListOptions;

/// How many people each visibility of one App reaches (AP-173). Counted from the bindings in
/// force and the members of the `Group` manifests they name; a group the identity provider owns
/// lists no members here, so a count is at least that many people.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppAudience {
    /// Members of the App's project.
    pub project: usize,
    /// Everyone in the organization.
    pub organization: usize,
    /// Holders of each role the App declares, in the order `spec.roles` lists them.
    pub roles: Vec<RoleAudience>,
    /// Whether the organization allows public Apps (PF-103).
    pub public_allowed: bool,
}

#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct RoleAudience {
    pub name: String,
    /// Holders by lower-case identifier, counted once each.
    pub people: usize,
}

/// The part of an App's spec its audience counts read.
#[derive(Debug, Default, Deserialize)]
struct Roles {
    #[serde(default)]
    roles: Vec<jc_core::kinds::AppRole>,
    #[serde(default)]
    access: Vec<jc_core::kinds::AppAccess>,
}

/// One App another project shared with the caller's organization (AP-174).
#[derive(Debug, Serialize, ToSchema, PartialEq)]
pub struct SharedApp {
    pub project: String,
    pub name: String,
    /// One string, or a map per locale (UI-50).
    #[schema(value_type = Object)]
    pub title: Option<serde_json::Value>,
    #[schema(value_type = Object)]
    pub description: Option<serde_json::Value>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/shared-apps", get(shared))
        .route("/projects/{project}/apps/{name}/audience", get(audience))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/apps/{name}/audience",
    operation_id = "app_audience",
    summary = "Who Each Visibility Of An App Reaches",
    description = "How many people the project, the organization and each role of the App reach, for the Share dialog (AP-173).",
    tag = "apps",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "App name"),
    ),
    responses(
        (status = 200, description = "The count per visibility", body = AppAudience),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No App of this name the caller may read", body = ProblemDetails)
    )
)]
pub async fn audience(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<AppAudience>, ApiError> {
    audience_of(&state, &user.0.identity, &project, &name).map(Json)
}

fn audience_of(
    state: &AppState,
    identity: &Identity,
    project: &str,
    name: &str,
) -> Result<AppAudience, ApiError> {
    // Someone who may not read the App learns no more than of a name never created (AP-18).
    let not_found = || ApiError::NotFound(format!("app '{name}' not found"));
    let app = state
        .mirror
        .get(project, "App", name)
        .and_then(|app| serde_json::to_value(app).ok())
        .ok_or_else(not_found)?;
    if !crate::permissions::for_request(state, identity, project).may_read_manifest("App", &app) {
        return Err(not_found());
    }
    let spec: Roles = app
        .get("spec")
        .and_then(|spec| serde_json::from_value(spec.clone()).ok())
        .unwrap_or_default();
    let now = chrono::Utc::now();
    let members = crate::permissions::project_members(&state.mirror, project, now);
    let groups = crate::permissions::group_members(&state.mirror);
    let roles = spec
        .roles
        .iter()
        .map(|role| {
            let subjects: Vec<_> = spec
                .access
                .iter()
                .filter(|access| access.role == role.name)
                .flat_map(|access| access.subjects.iter().cloned())
                .collect();
            RoleAudience {
                name: role.name.clone(),
                people: crate::permissions::people_of(&groups, &subjects).len(),
            }
        })
        .collect();
    Ok(AppAudience {
        project: members.readers.union(&members.writers).count(),
        organization: crate::permissions::organization_people(&state.mirror, now).len(),
        roles,
        public_allowed: !crate::api::mutate::public_apps_refused(state),
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/shared-apps",
    operation_id = "apps_shared_with_me",
    summary = "Apps Shared With Me",
    description = "The published Apps of other projects visible to the organization that the caller may open (AP-174).",
    tag = "apps",
    responses(
        (status = 200, description = "The Apps shared with the caller, by project and name", body = [SharedApp]),
        (status = 401, description = "Unauthorized", body = ProblemDetails)
    )
)]
pub async fn shared(user: CurrentUser, State(state): State<AppState>) -> Json<Vec<SharedApp>> {
    Json(shared_with(&state, &user.0.identity))
}

fn shared_with(state: &AppState, identity: &Identity) -> Vec<SharedApp> {
    let person = crate::apps::roles::AppPerson {
        identity: identity.clone(),
        roles: Vec::new(),
    };
    let mut apps = Vec::new();
    for project in state.mirror.namespaces() {
        // The person's own projects list their Apps on their own Apps page.
        if crate::permissions::for_request(state, identity, &project).may_read_project() {
            continue;
        }
        for app in state
            .mirror
            .list(&project, "App", &ListOptions::default())
            .items
        {
            let Ok(spec) = serde_json::from_value::<AppSpec>(app.spec.clone()) else {
                continue;
            };
            let name = &app.metadata.name;
            if spec.visibility != AppVisibility::Organization
                || spec.lifecycle != AppLifecycle::Published
                || !crate::apps::roles::may_open(state, &project, name, &spec, Some(&person))
            {
                continue;
            }
            apps.push(SharedApp {
                project: project.clone(),
                name: name.clone(),
                title: app
                    .metadata
                    .title
                    .as_ref()
                    .and_then(|t| serde_json::to_value(t).ok()),
                description: app
                    .metadata
                    .description
                    .as_ref()
                    .and_then(|t| serde_json::to_value(t).ok()),
            });
        }
    }
    apps
}
