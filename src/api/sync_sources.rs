//! What a `SyncSource` reports and what an operator can do to it (MF-28, MF-30).
//!
//! The manifest itself is served by the generic resource routes like every other kind; this is
//! the running loop around it — the phase, the revision it carries, the merge request it is
//! waiting on, and the three buttons the project page offers.
//!
//! The three differ in what they touch, which is the whole reason there are three:
//!
//! - **Sync now** runs the loop once, whatever the schedule says. It changes nothing but the
//!   run's memory, and what it produces is a merge request like any other run's.
//! - **Pause** switches the loop off. It is an operator's decision about a running process, not
//!   a change to the repository, so it is recorded beside the run's memory and no merge
//!   request is opened for it.
//! - **Detach** stops syncing for good, and that *is* a change to the repository: the manifest
//!   goes away, so the change is proposed and reviewed like every other one (CC-03). The
//!   resources the source brought in stay where they are — detaching a source is not deleting
//!   what it published.
//!
//! The webhook route lives here too. A source with `schedule: { webhook: true }` runs when its
//! origin says it moved, and the caller proves themselves with an HMAC-SHA256 signature over the
//! request body — against **that source's own** secret, `spec.webhook.secretRef`, which the
//! reconciler resolves each pass and leaves in [`crate::sync::webhook_secrets`] (MF-44, T-2297).
//! The signature covers the body and not the path, so one shared secret would have let the origin
//! of any source force a run of every other; and an unauthenticated trigger would be a way for
//! anybody on the internet to make the Portal fetch a remote repository as often as they liked.

use std::collections::BTreeMap;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::resource;
use crate::state::AppState;
use crate::sync::driver::{Driver, RunError};
use crate::sync::proposal::{self, Proposal};
use crate::sync::state::Stored;
use jc_core::kinds::Verb;

/// The kind this module drives.
const KIND: &str = "SyncSource";

/// What the project page shows for one source (MF-30).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SyncSourceStatus {
    pub project: String,
    pub name: String,
    /// `Synced`, `OutOfSync`, `PendingApproval`, `Error` or `Paused`.
    pub phase: String,
    /// The source revision the repository carries.
    pub observed_revision: Option<String>,
    /// When the last run happened, in seconds since the epoch.
    pub last_run_at: Option<u64>,
    /// The merge request a run opened and nobody has answered yet.
    pub merge_request: Option<String>,
    /// Why the last run did not finish, when it did not.
    pub last_error: Option<String>,
    pub paused: bool,
    /// Whether a restart keeps this. `false` on a Portal without a database, where a restart
    /// costs one duplicate proposal per source with a run in flight.
    pub durable: bool,
}

impl SyncSourceStatus {
    fn of(project: &str, name: &str, stored: &Stored, durable: bool) -> Self {
        Self {
            project: project.to_owned(),
            name: name.to_owned(),
            phase: stored.phase().as_str().to_owned(),
            observed_revision: stored.state.observed_revision.clone(),
            last_run_at: stored.state.last_run_at,
            merge_request: stored.merge_request.clone(),
            last_error: stored.last_error.clone(),
            paused: stored.state.paused,
            durable,
        }
    }
}

/// What one run did, and where the source stands afterwards.
#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SyncRunReport {
    /// The `kind: Change` envelope of every merge request the run opened.
    pub proposed: Vec<Value>,
    /// Whether the run changed anything at all.
    pub unchanged: usize,
    /// What the run could not do, in sentences a person can act on.
    pub flags: Vec<String>,
    pub status: SyncSourceStatus,
}

/// Whether syncing is on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
pub struct PauseRequest {
    /// `true` switches the loop off, `false` switches it back on.
    pub paused: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/syncsources/{name}/status",
    summary = "Read Sync State",
    description = "Where one SyncSource stands: its phase, the revision it saw and why the last run stopped.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "SyncSource name"),
    ),
    responses(
        (status = 200, description = "What the source reports about itself", body = SyncSourceStatus),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such project or source", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails),
    )
)]
pub async fn status(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<SyncSourceStatus>, ApiError> {
    // A read (PF-59, T-1361): a caller who may not read the project's sync sources gets the
    // answer of a source that is not there, before anything about the loop is said.
    if !crate::permissions::for_request(&state, &user.0.identity, &project).may_read(KIND) {
        return Err(ApiError::NotFound(format!(
            "sync source '{name}' not found in project '{project}'"
        )));
    }
    let driver = driver(&state)?;
    let (project, name) = named(&state, &project, &name)?;
    let stored = driver.status(&project, &name).await;
    Ok(Json(SyncSourceStatus::of(
        &project,
        &name,
        &stored,
        driver.is_durable(),
    )))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/syncsources/{name}/sync",
    summary = "Sync Now",
    description = "Runs one SyncSource at once; what it finds becomes changes a person approves.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "SyncSource name"),
    ),
    responses(
        (status = 200, description = "The run and what the source reports afterwards", body = SyncRunReport),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such project or source", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails),
    )
)]
pub async fn sync_now(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<SyncRunReport>, ApiError> {
    may_drive(&state, &user, &project, Verb::Propose)?;
    let driver = driver(&state)?;
    let (project, name) = named(&state, &project, &name)?;
    Ok(Json(run(driver, &project, &name).await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/syncsources/{name}/pause",
    summary = "Pause Or Resume Syncing",
    description = "Switches one SyncSource's loop off, or back on; nothing already proposed is touched.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "SyncSource name"),
    ),
    request_body(content = PauseRequest, example = json!({ "paused": true })),
    responses(
        (status = 200, description = "What the source reports afterwards", body = SyncSourceStatus),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such project or source", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails),
    )
)]
pub async fn pause(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Json(request): Json<PauseRequest>,
) -> Result<Json<SyncSourceStatus>, ApiError> {
    may_drive(&state, &user, &project, Verb::Propose)?;
    let driver = driver(&state)?;
    let (project, name) = named(&state, &project, &name)?;
    driver.pause(&project, &name, request.paused).await;
    let stored = driver.status(&project, &name).await;
    Ok(Json(SyncSourceStatus::of(
        &project,
        &name,
        &stored,
        driver.is_durable(),
    )))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/syncsources/{name}/detach",
    summary = "Detach A Sync Source",
    description = "Stops the loop and proposes removing the SyncSource; what it imported stays.",
    tag = "resources",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "SyncSource name"),
    ),
    responses(
        (status = 202, description = "The merge request that removes the source", body = Object),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "No such project or source", body = ProblemDetails),
        (status = 503, description = "No repository configured", body = ProblemDetails),
    )
)]
pub async fn detach(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let (pull, status) = detach_for(&state, &user, &project, &name).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "mergeRequest": pull.url,
            "number": pull.number,
            "status": status,
        })),
    )
        .into_response())
}

/// The detach itself, for the route and for `jc_syncsource_detach`: the merge request that removes
/// the source, and where the source stands once its loop is paused. The operation answers that
/// merge request as a red-lane Change, the shape every other proposal of the registry answers.
pub(crate) async fn detach_for(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    name: &str,
) -> Result<(crate::git::PullRequest, SyncSourceStatus), ApiError> {
    may_drive(state, user, project, Verb::Delete)?;
    let driver = driver(state)?;
    let gitea = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let gitea: &crate::git::GiteaClient = &gitea;
    let (project, name) = named(state, project, name)?;

    let info = resource::by_kind(KIND)
        .ok_or_else(|| ApiError::Internal("the SyncSource kind is not in the catalogue".into()))?;
    let path =
        resource::repository_path(info, &project, None, &name).map_err(ApiError::BadRequest)?;

    // Off before the merge request is answered, not after: a source that kept syncing while its
    // own removal waited for review would open merge requests nobody wants to read.
    driver.pause(&project, &name, true).await;

    let pull = proposal::open(
        gitea,
        &Proposal {
            branch: &format!("detach/{project}-{name}"),
            title: &format!("detach sync source {project}/{name}"),
            body: &format!(
                "Removes `{path}`, so `{project}/{name}` stops syncing.\n\nThe resources it \
                 brought into the project stay where they are; detaching a source is not \
                 deleting what it published. Syncing is already paused, and stays paused if \
                 this is closed rather than merged.\n"
            ),
            files: BTreeMap::new(),
            removed: vec![path],
            // Never: removing the thing that keeps a project aligned with a standard is a
            // decision a person makes (CC-70).
            auto_merge: false,
            line: None,
        },
    )
    .await?;

    let status = SyncSourceStatus::of(
        &project,
        &name,
        &driver.status(&project, &name).await,
        driver.is_durable(),
    );
    Ok((pull, status))
}

#[utoipa::path(
    post,
    path = "/api/v1/webhooks/sync/{project}/{name}",
    summary = "Sync Source Webhook",
    description = "Called by a SyncSource's origin to run it now. The body must carry the signature of the source's own secret; every other case answers the same 401, so the call reveals nothing (MF-44, PF-59).",
    tag = "system",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "SyncSource name"),
        ("x-gitea-signature" = String, Header, description = "HMAC-SHA256 of the request body"),
    ),
    request_body(
        content = serde_json::Value,
        description = "Whatever the origin sends; the body is what the signature covers",
        content_type = "application/json",
        example = json!({ "ref": "refs/heads/main", "after": "4f2a9c1d0e8b7a6f5e4d3c2b1a0f9e8d7c6b5a49" }),
    ),
    responses(
        (status = 200, description = "The run the source's origin asked for", body = SyncRunReport),
        (status = 401, description = "The signature is not one this source's own secret makes over this body — the same answer as for a source that is not there, one with no `spec.webhook`, and one whose reference this instance cannot resolve (MF-44, PF-59)", body = ProblemDetails),
        (status = 503, description = "No repository configured, or no sync loop running", body = ProblemDetails),
    )
)]
pub async fn webhook(
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<SyncRunReport>, ApiError> {
    // The source's own secret, and nothing platform-wide: the signature covers the body and not
    // the path, so a shared secret would make every source's path reachable by whoever holds one
    // of them (MF-44, T-2297). `verifies` answers no for a source that is not there, one with no
    // `spec.webhook`, and one whose reference did not resolve — all three are this `401`, because
    // the door is unauthenticated until the signature verifies and a `404` here would be an
    // existence oracle over the sources of every project (PF-59, R20).
    let presented = headers
        .get(crate::api::webhook::SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !state
        .webhook_secrets
        .verifies(&project, &name, &body, presented)
    {
        return Err(ApiError::Unauthorized);
    }

    // Past the signature the caller has proved they hold this source's secret, so what is left
    // may name it: a source the reconciler holds a secret for and the mirror does not is a
    // pass-old race, not something a stranger can reach.
    let driver = driver(&state)?;
    let (project, name) = named(&state, &project, &name)?;
    Ok(Json(run(driver, &project, &name).await?))
}

/// One forced run and the status that follows it.
async fn run(driver: &Driver, project: &str, name: &str) -> Result<SyncRunReport, ApiError> {
    let now = crate::auth::session::now_unix().max(0) as u64;
    let outcome = driver.run_now(project, name, now).await.map_err(failed)?;
    let stored = driver.status(project, name).await;
    Ok(SyncRunReport {
        proposed: outcome.proposed,
        unchanged: outcome.unchanged,
        flags: outcome.flags,
        status: SyncSourceStatus::of(project, name, &stored, driver.is_durable()),
    })
}

/// Who may drive a source (T-0800, PF-50): a run or a pause changes what the project syncs,
/// which is `propose` on the SyncSource; a detach removes it, which is `delete`. Checked
/// before the loop is asked for, so a person without a binding learns nothing about it.
fn may_drive(
    state: &AppState,
    user: &CurrentUser,
    project: &str,
    verb: Verb,
) -> Result<(), ApiError> {
    crate::permissions::for_request(state, &user.0.identity, project).check(KIND, verb, None)
}

/// The loop, or the reason there is none.
fn driver(state: &AppState) -> Result<&Driver, ApiError> {
    state
        .sync
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("the sync loop is not running".into()))
}

/// The project and source a request names, refused when the repository holds no such source.
///
/// Read from the Portal's mirror of the repository rather than from the forge: it is the same
/// view every other resource route answers from, so a source that is not in it is a source the
/// Portal does not have (MF-04).
pub(crate) fn named(
    state: &AppState,
    project: &str,
    name: &str,
) -> Result<(String, String), ApiError> {
    if !resource::is_dns1123(project) || !resource::is_dns1123(name) {
        return Err(ApiError::NotFound(format!(
            "sync source '{name}' not found in project '{project}'"
        )));
    }
    if state.mirror.get(project, KIND, name).is_none() {
        return Err(ApiError::NotFound(format!(
            "sync source '{name}' not found in project '{project}'"
        )));
    }
    Ok((project.to_owned(), name.to_owned()))
}

/// A run that could not happen, as the answer the caller gets.
fn failed(err: RunError) -> ApiError {
    match err {
        RunError::NoSuchSource(name, project) => ApiError::NotFound(format!(
            "sync source '{name}' not found in project '{project}'"
        )),
        // Everything else is the forge or the origin: the request was right and the Portal
        // could not act on it.
        other => ApiError::Unavailable(other.to_string()),
    }
}

/// The four routes a session drives.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/syncsources/{name}/status", get(status))
        .route(
            "/projects/{project}/syncsources/{name}/sync",
            post(sync_now),
        )
        .route("/projects/{project}/syncsources/{name}/pause", post(pause))
        .route(
            "/projects/{project}/syncsources/{name}/detach",
            post(detach),
        )
}

/// The webhook, which is a signature and not a session.
pub fn webhook_router() -> Router<AppState> {
    Router::new().route("/webhooks/sync/{project}/{name}", post(webhook))
}
