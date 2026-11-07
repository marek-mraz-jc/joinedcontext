//! `POST /api/v1/projects/{project}/apps/rename-shapes` (AP-124, API/01 §12b, T-2940).
//!
//! For one release `static` and `fullstack` are read as `ui` and `ui-rust`. A project's Apps
//! page offers one click that rewrites every App still written with an old name, as one Change
//! in the clicking person's name (owner decision 2026-10-06). Each rewritten App passes the
//! single-manifest door first, as a dry run, so the refusals, the permission and the lane are the
//! ones a form would meet; then the files go out together through the forge half of an import.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use jc_core::kinds::Verb;

use crate::api::import::{digest, propose_files, riskiest, Proposal};
use crate::api::mutate::{propose_with_identity, ProposeOutcome};
use crate::auth::CurrentUser;
use crate::change::{Lane, Operation, PlanSummary};
use crate::error::{ApiError, ProblemDetails};
use crate::resource::{self, ResourceEnvelope};
use crate::state::AppState;
use crate::store::ListOptions;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/apps/rename-shapes",
        post(rename_shapes),
    )
}

/// What the person is told when there is nothing to rename.
const NOTHING_TO_RENAME: &str =
    "no App of this project is written with `static` or `fullstack`, so there is nothing to rename (AP-124)";

/// The project's Apps written with an old shape name, each with `spec.kind` rewritten to the new
/// one and nothing else changed, in name order.
pub fn renamed_apps(state: &AppState, project: &str) -> Vec<ResourceEnvelope> {
    state
        .mirror
        .list(project, "App", &ListOptions::default())
        .items
        .into_iter()
        .filter_map(|mut app| {
            let written = app.spec.get("kind")?.as_str()?;
            let (_, new) = jc_core::kinds::AppClass::renamed(written)?;
            app.spec["kind"] = serde_json::Value::String(new.to_owned());
            app.strip_status();
            Some(app)
        })
        .collect()
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/apps/rename-shapes",
    summary = "Rename The App Shapes Of A Project",
    description = "Proposes one Change, in the caller's name, that rewrites every App of the project written with `static` or `fullstack` to `ui` or `ui-rust` (AP-124).",
    tag = "apps",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 202, description = "The Change that renames them"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller may not propose App here", body = ProblemDetails),
        (status = 404, description = "No such project the caller may read", body = ProblemDetails),
        (status = 409, description = "No App of the project carries an old shape name", body = ProblemDetails),
        (status = 400, description = "A rewritten App does not pass the App door", body = ProblemDetails),
        (status = 503, description = "No forge", body = ProblemDetails)
    )
)]
pub async fn rename_shapes(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
) -> Result<Response, ApiError> {
    let identity = &user.0.identity;
    let effective = crate::permissions::for_request(&state, identity, &project);
    if !resource::is_dns1123(&project) || !effective.may_read_project() {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    // Who may not propose an App learns nothing about which Apps there are.
    effective.check("App", Verb::Propose, None)?;

    let apps = renamed_apps(&state, &project);
    if apps.is_empty() {
        return Err(ApiError::Conflict(NOTHING_TO_RENAME.into()));
    }
    let info = resource::by_kind("App")
        .ok_or_else(|| ApiError::Internal("no catalogue entry for App".into()))?;

    let mut lane = Lane::Green;
    let mut files = Vec::with_capacity(apps.len());
    for app in &apps {
        let body = serde_json::to_value(app)
            .map_err(|e| ApiError::Internal(format!("the App did not serialise: {e}")))?;
        // The single-manifest door, as a dry run: permission on this App, its invariants, its
        // name, its lane. A refusal names the App, so the person knows which one to open.
        let checked = propose_with_identity(
            identity,
            &state,
            &project,
            info.plural,
            Some(&app.metadata.name),
            Operation::Update,
            true,
            body,
        )
        .await
        .map_err(|err| refused(&app.metadata.name, err))?;
        match checked {
            ProposeOutcome::DryRun(result) if result.valid => lane = riskiest(lane, result.lane),
            ProposeOutcome::DryRun(result) => {
                return Err(ApiError::Invalid {
                    detail: format!(
                        "App '{}' does not pass the App door after the rename",
                        app.metadata.name
                    ),
                    errors: result.findings,
                })
            }
            _ => {
                return Err(ApiError::Internal(
                    "a dry run answered with something other than its plan".into(),
                ))
            }
        }
        let path = resource::repository_path(info, &project, None, &app.metadata.name)
            .map_err(ApiError::BadRequest)?;
        let content = serde_norway::to_string(app)
            .map_err(|e| ApiError::Internal(format!("the App did not serialise: {e}")))?;
        files.push((path, content));
    }

    let names: Vec<&str> = apps.iter().map(|app| app.metadata.name.as_str()).collect();
    let body = format!(
        "`static` and `fullstack` are read as `ui` and `ui-rust` for one release and refused by the \
         next (AP-124). This Change writes the new name into {} App(s) and changes nothing else:\n\n{}",
        names.len(),
        apps.iter()
            .map(|app| format!(
                "- `{}`: `kind: {}`",
                app.metadata.name,
                app.spec["kind"].as_str().unwrap_or_default()
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let branch = format!("portal/rename-app-shapes-{project}-{:08x}", digest(&files));
    let change = propose_files(
        &state,
        identity,
        &project,
        Proposal {
            files,
            branch,
            title: format!("rename the App shapes of {project} (AP-124)"),
            body,
            lane,
            summary: PlanSummary::new(0, names.len(), 0),
        },
    )
    .await?;
    tracing::info!(project = %project, apps = names.len(), by = %identity.username, "App shapes rename proposed");
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

/// A refusal of one App's dry run, with the App named: every other status keeps its own.
fn refused(app: &str, err: ApiError) -> ApiError {
    match err {
        ApiError::BadRequest(why) => ApiError::BadRequest(format!("App '{app}': {why}")),
        ApiError::Invalid { detail, errors } => ApiError::Invalid {
            detail: format!("App '{app}': {detail}"),
            errors,
        },
        other => other,
    }
}
