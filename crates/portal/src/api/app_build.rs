//! The build of a `static` application on the forge, as its App page shows it (AP-103, ADR-N-028).
//!
//! The build is the repository's `.gitea/workflows/build.yml` (AP-100): the Portal starts
//! nothing, it links the repository, the newest run and the package, and Rebuild dispatches the
//! workflow on the default branch. The repository is `{project}_{app}` (AP-75), derived from the
//! path and never read from the manifest, so no manifest points a dispatch at another repository.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::agents::repository;
use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::git::{Author, GitError, GiteaClient, WorkflowRun};
use crate::permissions::ORG_NAMESPACE;
use crate::state::AppState;

/// The workflow every application repository carries (AP-100).
const WORKFLOW_FILE: &str = "build.yml";

/// Where an application's build is, and whether this person may ask for another (AP-103).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppBuild {
    /// The App's own repository (its whole source, T-3039); `null` for an App not built on the
    /// forge.
    pub repository_url: Option<String>,
    /// The project's configuration repository, the one its Changes merge into; `null` for a
    /// caller the forge does not let read it (AP-103, PF-87, T-3039).
    pub configuration_url: Option<String>,
    /// The newest workflow run, `null` before the first.
    pub run: Option<WorkflowRun>,
    /// How long the newest finished successful run took, in seconds: the estimate a running
    /// build is shown against (T-3245); `null` before the first one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typical_seconds: Option<u64>,
    /// The package of `status.build.commit`, `null` while the App has no build.
    pub package_url: Option<String>,
    pub rebuild: Rebuild,
}

/// Whether Rebuild is offered, and why not when it is not (PF-50, UI-44).
#[derive(Debug, Serialize, ToSchema)]
pub struct Rebuild {
    pub allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/apps/{name}/build", get(build))
        .route("/projects/{project}/apps/{name}/rebuild", post(rebuild))
        .route("/projects/{project}/apps/{name}/builds", get(builds))
        .route("/projects/{project}/apps/{name}/restore", post(restore))
}

/// The App as this person may see it, its repository when it is built on the forge, and the
/// forge client. An App the caller may not read is the same `404` as one that does not exist.
pub(crate) fn app_of(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    name: &str,
) -> Result<(serde_json::Value, Option<GiteaClient>), ApiError> {
    let not_found = || ApiError::NotFound(format!("app '{name}' not found"));
    let app = state
        .mirror
        .get(project, "App", name)
        .and_then(|app| serde_json::to_value(app).ok())
        .ok_or_else(not_found)?;
    if !crate::permissions::for_request(state, &user.0.identity, project)
        .may_read_manifest("App", &app)
    {
        return Err(not_found());
    }
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no forge is configured".into()))?;
    let on_forge = app
        .pointer("/spec/source/git")
        .is_some_and(|git| !git.is_null());
    Ok((
        app,
        on_forge.then(|| gitea.for_application(repository::name(project, name))),
    ))
}

/// The project's configuration repository, for a caller the forge lets read it (PF-87, T-3039):
/// in layout 2 a person the project's bindings place in its readers, in layout 1 a person with a
/// binding at the organization, and an organization administrator in both.
fn configuration_url(state: &AppState, user: &CurrentUser, project: &str) -> Option<String> {
    let identity = &user.0.identity;
    let forge = state.forge_for(project)?;
    let organization = crate::permissions::for_request(state, identity, ORG_NAMESPACE);
    let reads = organization.administers_organization()
        || if state.mirror.repository_of(project).is_some() {
            let readers =
                crate::permissions::project_members(&state.mirror, project, chrono::Utc::now())
                    .readers;
            [Some(identity.username.as_str()), identity.email.as_deref()]
                .into_iter()
                .flatten()
                .any(|who| readers.contains(&who.trim().to_ascii_lowercase()))
        } else {
            organization.may_read_project()
        };
    reads.then(|| forge.repository_page_url())
}

/// Why this person may not rebuild `app`, or `None` when they may: propose judged on the App
/// itself, so a grant bound to one App or space reaches no other (F-POR1-005).
fn rebuild_refusal(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    app: &serde_json::Value,
) -> Option<String> {
    crate::permissions::for_request(state, &user.0.identity, project)
        .check("App", Verb::Propose, Some(app))
        .is_err()
        .then(|| format!("Rebuild needs propose on App in project {project}"))
}

pub(crate) const NOT_ON_FORGE: &str =
    "the App is not built on the forge: it names no spec.source.git of its own (AP-100)";

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/apps/{name}/build",
    summary = "Where An Application's Build Is",
    description = "The repository, the newest workflow run and the package of an application built on the forge, and whether Rebuild is offered.",
    tag = "apps",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "App name"),
    ),
    responses(
        (status = 200, description = "The build's links", body = AppBuild),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such App the caller may read", body = ProblemDetails),
        (status = 503, description = "No forge is configured", body = ProblemDetails)
    )
)]
pub async fn build(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<AppBuild>, ApiError> {
    let (app, repo) = app_of(&state, &user, &project, &name)?;
    let configuration_url = configuration_url(&state, &user, &project);
    let Some(repo) = repo else {
        return Ok(Json(AppBuild {
            repository_url: None,
            configuration_url,
            run: None,
            typical_seconds: None,
            package_url: None,
            rebuild: Rebuild {
                allowed: false,
                reason: Some(NOT_ON_FORGE.into()),
            },
        }));
    };
    // ponytail: the estimate is the newest successful run of the last five; a median when one
    // slow run makes it lie.
    let runs = match repo.latest_runs(5).await {
        Ok(runs) => runs,
        // A repository with Actions off, or not created yet, has no run to link.
        Err(GitError::NotFound) => Vec::new(),
        Err(err) => {
            tracing::warn!(app = %name, error = %err, "the application's workflow runs were not read");
            Vec::new()
        }
    };
    let typical_seconds = runs
        .iter()
        .filter(|run| run.conclusion.as_deref() == Some("success"))
        .find_map(WorkflowRun::seconds);
    let run = runs.into_iter().next();
    let build = |field: &str| {
        app.pointer(&format!("/status/build/{field}"))
            .and_then(serde_json::Value::as_str)
    };
    let package_url = build("commit")
        .zip(build("digest"))
        .map(|(commit, digest)| {
            repo.package_page_url(
                &crate::apps::built::package(&name),
                &crate::apps::built::version(commit, digest),
            )
        });
    let refusal = rebuild_refusal(&state, &user, &project, &app);
    Ok(Json(AppBuild {
        // The applications' organization reads every App's repository to every signed-in
        // person (PF-79, T-3030), and a caller who reached this line may read the App.
        repository_url: Some(repo.repository_page_url()),
        configuration_url,
        run,
        typical_seconds,
        package_url,
        rebuild: Rebuild {
            allowed: refusal.is_none(),
            reason: refusal,
        },
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/apps/{name}/rebuild",
    summary = "Rebuild An Application",
    description = "Dispatches the application's build.yml on its repository's default branch.",
    tag = "apps",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "App name"),
    ),
    responses(
        (status = 202, description = "The forge accepted the dispatch"),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller may not propose App here", body = ProblemDetails),
        (status = 404, description = "No such App the caller may read", body = ProblemDetails),
        (status = 409, description = "The App is not built on the forge", body = ProblemDetails),
        (status = 503, description = "No forge, or the forge refused the dispatch", body = ProblemDetails)
    )
)]
pub async fn rebuild(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let (app, repo) = app_of(&state, &user, &project, &name)?;
    if let Some(reason) = rebuild_refusal(&state, &user, &project, &app) {
        return Err(ApiError::Denied(reason));
    }
    let repo = repo.ok_or_else(|| ApiError::Conflict(NOT_ON_FORGE.into()))?;
    let refused = |err: GitError| {
        ApiError::Unavailable(format!(
            "the forge did not start the build of '{name}': {err}"
        ))
    };
    let branch = repo.default_branch().await.map_err(refused)?;
    repo.dispatch_workflow(WORKFLOW_FILE, &branch)
        .await
        .map_err(refused)?;
    tracing::info!(app = %name, project = %project, by = %user.0.identity.username, "rebuild dispatched");
    Ok(StatusCode::ACCEPTED.into_response())
}

/// How many runs the builds list reads: the forge's page size.
const BUILDS_READ: u32 = 50;

const DATA_NOTE: &str =
    "Restore brings back the App's source only; its schema, files and data stay as they are.";

/// One successful build of an App, as Restore offers it (AP-171).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BuildEntry {
    pub commit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// The build `status.build.commit` names.
    pub current: bool,
}

/// The successful builds, newest first, and whether Restore is offered (AP-171).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppBuilds {
    pub builds: Vec<BuildEntry>,
    pub restore: Rebuild,
    /// What Restore does not roll back.
    pub data_note: &'static str,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RestoreRequest {
    /// The commit of one of the listed builds.
    pub commit: String,
}

/// The merge request Restore opened.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    pub branch: String,
    pub pull_request_url: String,
}

/// The successful runs of the repository, one per commit, newest first.
async fn successful_builds(repo: &GiteaClient, current: Option<&str>) -> Vec<BuildEntry> {
    let runs = match repo.latest_runs(BUILDS_READ).await {
        Ok(runs) => runs,
        Err(GitError::NotFound) => Vec::new(),
        Err(err) => {
            tracing::warn!(error = %err, "the application's workflow runs were not read");
            Vec::new()
        }
    };
    let mut builds: Vec<BuildEntry> = Vec::new();
    for run in runs {
        if run.conclusion.as_deref() != Some("success")
            || builds.iter().any(|b| b.commit == run.commit)
        {
            continue;
        }
        builds.push(BuildEntry {
            current: current == Some(run.commit.as_str()),
            commit: run.commit,
            number: run.number,
            completed_at: run.completed_at,
        });
    }
    builds
}

fn current_commit(app: &serde_json::Value) -> Option<&str> {
    app.pointer("/status/build/commit")
        .and_then(serde_json::Value::as_str)
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/apps/{name}/builds",
    summary = "The Builds Of An Application",
    description = "The successful builds of an application built on the forge, one per commit, newest first, and whether Restore is offered (AP-171).",
    tag = "apps",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "App name"),
    ),
    responses(
        (status = 200, description = "The builds", body = AppBuilds),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such App the caller may read", body = ProblemDetails),
        (status = 503, description = "No forge is configured", body = ProblemDetails)
    )
)]
pub async fn builds(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<AppBuilds>, ApiError> {
    let (app, repo) = app_of(&state, &user, &project, &name)?;
    let Some(repo) = repo else {
        return Ok(Json(AppBuilds {
            builds: Vec::new(),
            restore: Rebuild {
                allowed: false,
                reason: Some(NOT_ON_FORGE.into()),
            },
            data_note: DATA_NOTE,
        }));
    };
    let builds = successful_builds(&repo, current_commit(&app)).await;
    let refusal = restore_refusal(&state, &user, &project, &app);
    Ok(Json(AppBuilds {
        builds,
        restore: Rebuild {
            allowed: refusal.is_none(),
            reason: refusal,
        },
        data_note: DATA_NOTE,
    }))
}

/// Why this person may not restore, or `None` when they may: the right of Rebuild (AP-171).
fn restore_refusal(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    app: &serde_json::Value,
) -> Option<String> {
    rebuild_refusal(state, user, project, app)
        .map(|reason| reason.replacen("Rebuild", "Restore", 1))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/apps/{name}/restore",
    summary = "Restore An Earlier Build Of An Application",
    description = "Opens a merge request on the application's repository that brings its default branch back to the commit of an earlier successful build; the build lane builds it once merged (AP-171).",
    tag = "apps",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "App name"),
    ),
    request_body(
        content = RestoreRequest,
        example = json!({ "commit": "9a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b" })
    ),
    responses(
        (status = 201, description = "The merge request is open", body = Restored),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "The caller may not propose App here", body = ProblemDetails),
        (status = 404, description = "No such App the caller may read", body = ProblemDetails),
        (status = 409, description = "Not built on the forge, not a listed build, the current build, nothing to restore, or a file that is not text", body = ProblemDetails),
        (status = 503, description = "No forge, or the forge refused", body = ProblemDetails)
    )
)]
pub async fn restore(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Json(request): Json<RestoreRequest>,
) -> Result<Response, ApiError> {
    let (app, repo) = app_of(&state, &user, &project, &name)?;
    if let Some(reason) = restore_refusal(&state, &user, &project, &app) {
        return Err(ApiError::Denied(reason));
    }
    let repo = repo.ok_or_else(|| ApiError::Conflict(NOT_ON_FORGE.into()))?;
    let commit = request.commit.trim();
    // Only a commit the forge built successfully for this repository: a digest or commit of
    // another App is not among them (AP-171).
    let builds = successful_builds(&repo, current_commit(&app)).await;
    let build = builds.iter().find(|b| b.commit == commit).ok_or_else(|| {
        ApiError::Conflict(format!(
            "{commit} is not a successful build of the App '{name}'"
        ))
    })?;
    if build.current {
        return Err(ApiError::Conflict(format!(
            "{commit} is the build the App serves now"
        )));
    }
    let refused =
        |err: GitError| ApiError::Unavailable(format!("the forge did not restore '{name}': {err}"));
    let default = repo.default_branch().await.map_err(refused)?;
    let then = repo.list_tree_blobs(commit).await.map_err(refused)?;
    let now = repo.list_tree_blobs(&default).await.map_err(refused)?;
    let mut uploads = Vec::new();
    for (path, sha) in &then {
        if now.iter().any(|(p, s)| p == path && s == sha) {
            continue;
        }
        let file = match repo.get_file(path, commit).await {
            Ok(Some(file)) => file,
            Ok(None) => return Err(refused(GitError::NotFound)),
            // ponytail: the change API takes text, so a changed binary file cannot be restored.
            Err(GitError::Transport(msg)) if msg.contains("utf-8") => {
                return Err(ApiError::Conflict(format!(
                    "{path} is not UTF-8 text and cannot be restored through the forge's change API"
                )))
            }
            Err(err) => return Err(refused(err)),
        };
        uploads.push((path.clone(), file.content));
    }
    let deletes: Vec<(String, String)> = now
        .iter()
        .filter(|(path, _)| !then.iter().any(|(p, _)| p == path))
        .cloned()
        .collect();
    if uploads.is_empty() && deletes.is_empty() {
        return Err(ApiError::Conflict(format!(
            "the default branch already holds the files of {commit}"
        )));
    }
    let short = &commit[..commit.len().min(7)];
    let branch = format!("restore/{short}-{}", chrono::Utc::now().timestamp());
    repo.create_branch(&branch, &default)
        .await
        .map_err(refused)?;
    let (author_name, author_email) =
        crate::api::mutate::author_credentials(&user.0.identity, &project);
    let message = format!("Restore the build of {commit} (AP-171)");
    repo.change_files(
        &branch,
        &message,
        Author {
            name: &author_name,
            email: &author_email,
        },
        &uploads,
        &deletes,
    )
    .await
    .map_err(refused)?;
    let pull = repo
        .create_pull_request(
            &branch,
            &default,
            &message,
            &format!(
                "Brings {default} back to the source of the build of {commit}. Merging it builds that source; {DATA_NOTE}"
            ),
        )
        .await
        .map_err(refused)?;
    tracing::info!(app = %name, project = %project, commit = %commit, by = %user.0.identity.username, "restore proposed");
    Ok((
        StatusCode::CREATED,
        Json(Restored {
            branch,
            pull_request_url: pull.url,
        }),
    )
        .into_response())
}
