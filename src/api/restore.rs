//! Bringing back what a merged removal took (T-3247, API/01 §5).
//!
//! A removal is a Change like every other write, so what it removed is still in the repository at
//! the commit its branch was cut from. Restoring reads every file it deleted there and proposes
//! them again in one new Change, decided by an approver like the first one. What the reconciler
//! threw away with the resource (a pipeline's refused records and runs) does not come back; the
//! entities of a space were never removed with it.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Json, Router};
use jc_core::kinds::Verb;

use crate::api::changes::{change_meta, parse_change_id, resolve_change};
use crate::api::resources::{ListMeta, ResourceList};
use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::change::{self, Change, ChangePhase, ChangeStatus, Lane, Operation, PlanSummary};
use crate::error::{ApiError, ProblemDetails};
use crate::git::gitea::Author;
use crate::resource::{ResourceEnvelope, API_VERSION};
use crate::state::AppState;

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/changes/{id}/restore",
    summary = "Restore What a Change Removed",
    description = "Proposes again, as one new change, every file a merged change deleted, read at the commit its branch was cut from. Needs `propose` on every kind it brings back.",
    tag = "changes",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "The merged change that removed something: chg- + 8 hex digits"),
    ),
    responses(
        (status = 202, description = "The restoring change, waiting for an approver", body = Change),
        (status = 400, description = "Not a change id", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "No `propose` on a kind it brings back", body = ProblemDetails),
        (status = 404, description = "No such change the caller may read", body = ProblemDetails),
        (status = 409, description = "Not merged, removed nothing, or the resource is there again", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn restore_change(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<Change>), ApiError> {
    let change = restore_change_for(&state, &user.0.identity, &project, &id).await?;
    Ok((StatusCode::ACCEPTED, Json(change)))
}

/// The merged change `id` of `project` as `identity` may see it, with the caller's rights there
/// (T-3247, T-3274, T-3975): `404` unless they read the project and it is a Portal or workspace
/// change of it, `409` saying `unmerged` while it is not merged. A project's own changes only: an
/// organization change (`chg-org-`) is not one.
async fn merged_change(
    state: &AppState,
    identity: &Identity,
    project: &str,
    id: &str,
    unmerged: &str,
) -> Result<
    (
        crate::permissions::Effective,
        std::sync::Arc<crate::git::GiteaClient>,
        u64,
        crate::git::PullRequest,
    ),
    ApiError,
> {
    let missing = || ApiError::NotFound(format!("change '{id}' not found in project '{project}'"));
    let effective = crate::permissions::for_request(state, identity, project);
    if !effective.may_read_project() {
        return Err(missing());
    }
    parse_change_id(id)?;
    let (gitea, number) = resolve_change(state, project, id)?;
    let pr = gitea.pull_request(number).await?;
    let branch = pr
        .head_branch
        .strip_prefix("refs/heads/")
        .unwrap_or(&pr.head_branch);
    if !branch.starts_with("portal/") && !branch.starts_with("workspace/") {
        return Err(missing());
    }
    if !pr.merged {
        return Err(ApiError::Conflict(format!(
            "change {id} is not merged, so {unmerged}"
        )));
    }
    Ok((effective, gitea, number, pr))
}

/// Restores what the merged change `id` removed, as `identity` (T-3247).
pub async fn restore_change_for(
    state: &AppState,
    identity: &Identity,
    project: &str,
    id: &str,
) -> Result<Change, ApiError> {
    let (effective, forge, number, pr) = merged_change(
        state,
        identity,
        project,
        id,
        "it removed nothing yet; reject it instead",
    )
    .await?;
    let gitea: &crate::git::GiteaClient = &forge;
    let home = format!("projects/{project}/");
    let removed: Vec<String> = gitea
        .pull_request_files(number)
        .await?
        .into_iter()
        .filter(|file| file.deleted && file.path.starts_with(&home))
        .map(|file| file.path)
        .collect();
    if removed.is_empty() {
        return Err(ApiError::Conflict(format!(
            "change {id} removed nothing in project '{project}', so there is nothing to restore"
        )));
    }
    if pr.merge_base.is_empty() {
        return Err(ApiError::Unavailable(format!(
            "the forge did not say which commit change {id} was cut from; try again"
        )));
    }

    let default_branch = gitea.default_branch().await?;
    let mut uploads = Vec::with_capacity(removed.len());
    let mut restored: Vec<ResourceEnvelope> = Vec::new();
    for path in &removed {
        if gitea.get_file(path, &default_branch).await?.is_some() {
            return Err(ApiError::Conflict(format!(
                "'{path}' is in the project again, so change {id} has nothing left to restore"
            )));
        }
        let file = gitea.get_file(path, &pr.merge_base).await?.ok_or_else(|| {
            ApiError::Conflict(format!(
                "'{path}' is not in the commit change {id} was cut from; it cannot be restored"
            ))
        })?;
        // Every file answers to a kind: a manifest to its own, a native file (a LinkML source, a
        // bento.yaml) to the kind its directory names, the way an approval reads it (T-1400). A
        // file of no kind is restored by nobody, as it is approved by nobody.
        let manifest = serde_norway::from_str::<ResourceEnvelope>(&file.content).ok();
        let kind = match &manifest {
            Some(envelope) => envelope.kind.clone(),
            None => crate::api::import::native_kind(path)
                .map(str::to_owned)
                .ok_or_else(|| {
                    ApiError::Conflict(format!(
                        "'{path}' belongs to no kind this platform serves, so it cannot be restored"
                    ))
                })?,
        };
        // The manifest put back is judged as a proposal of it is, its space and constraints
        // included; a native file by the kind its folder names (T-5698).
        match &manifest {
            Some(envelope) => crate::api::import::authorize_manifest(
                state, identity, project, &effective, envelope,
            )?,
            None => effective.check(&kind, Verb::Propose, None).map_err(|_| {
                ApiError::Denied(format!(
                    "restoring '{path}' needs `propose` on {kind} in project '{project}'"
                ))
            })?,
        }
        if let Some(envelope) = manifest {
            restored.push(envelope);
        }
        uploads.push((path.clone(), file.content));
    }
    let Some(headline) = restored.first() else {
        return Err(ApiError::Conflict(format!(
            "change {id} removed no manifest in project '{project}', so there is nothing to restore"
        )));
    };

    let held = crate::api::mutate::turn(gitea, &format!("portal/restore-{number:08x}")).await;
    let restore_branch =
        crate::api::mutate::create_or_reuse_branch(gitea, &held, &default_branch).await?;
    let (author_name, author_email) = crate::api::mutate::author_credentials(identity, project);
    let what = format!("{} {}", headline.kind, headline.metadata.name);
    gitea
        .change_files(
            &restore_branch,
            &format!("restore {what}, removed by {id}"),
            Author {
                name: &author_name,
                email: &author_email,
            },
            &uploads,
            &[],
        )
        .await?;
    let opened = gitea
        .create_pull_request(
            &restore_branch,
            &default_branch,
            &format!("restore {what}"),
            &format!(
                "Restores what change {id} removed from project `{project}`, as it was before: {}.",
                removed.join(", ")
            ),
        )
        .await?;

    let lane = restored.iter().fold(Lane::Green, |lane, envelope| {
        crate::api::import::riskiest(lane, change::classify_manifest(envelope, Operation::Create))
    });
    let status = ChangeStatus::new(
        lane,
        ChangePhase::PendingApproval,
        PlanSummary::new(restored.len(), 0, 0),
    )
    .in_repository(&opened.repository)
    .proposed_through(&opened.body)
    .with_merge_request(opened.url);
    Ok(Change::new(
        change_meta(state, gitea, opened.number, project),
        status,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/changes/{id}/undo",
    summary = "Undo What a Change Altered",
    description = "Proposes, as one new change, every file a merged change modified as it was before, read at the commit its branch was cut from, while each file still holds what that change wrote. Needs `propose` on every kind it puts back (T-3274).",
    tag = "changes",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "The merged change that modified something: chg- + 8 hex digits"),
    ),
    responses(
        (status = 202, description = "The undoing change, waiting for an approver", body = Change),
        (status = 400, description = "Not a change id", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "No `propose` on a kind it puts back", body = ProblemDetails),
        (status = 404, description = "No such change the caller may read", body = ProblemDetails),
        (status = 409, description = "Not merged, modified nothing, or a file changed again since", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn undo_change(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<Change>), ApiError> {
    let change = undo_change_for(&state, &user.0.identity, &project, &id).await?;
    Ok((StatusCode::ACCEPTED, Json(change)))
}

/// Puts back what the merged change `id` modified, as `identity` (T-3274). Only while nothing
/// changed the same files after it: undoing over a later change would quietly drop that one too.
pub async fn undo_change_for(
    state: &AppState,
    identity: &Identity,
    project: &str,
    id: &str,
) -> Result<Change, ApiError> {
    let (effective, forge, number, pr) = merged_change(
        state,
        identity,
        project,
        id,
        "it altered nothing yet; reject it instead",
    )
    .await?;
    let gitea: &crate::git::GiteaClient = &forge;
    let home = format!("projects/{project}/");
    let modified: Vec<String> = gitea
        .pull_request_files(number)
        .await?
        .into_iter()
        .filter(|file| !file.added && !file.deleted && file.path.starts_with(&home))
        .map(|file| file.path)
        .collect();
    if modified.is_empty() {
        return Err(ApiError::Conflict(format!(
            "change {id} modified nothing in project '{project}': a removal is restored, a creation is removed"
        )));
    }
    if pr.merge_base.is_empty() || pr.head_sha.is_empty() {
        return Err(ApiError::Unavailable(format!(
            "the forge did not say which commits change {id} was made of; try again"
        )));
    }

    let default_branch = gitea.default_branch().await?;
    let mut uploads = Vec::with_capacity(modified.len());
    let mut put_back: Vec<ResourceEnvelope> = Vec::new();
    for path in &modified {
        let now = gitea.get_file(path, &default_branch).await?;
        let made = gitea.get_file(path, &pr.head_sha).await?;
        if now.as_ref().map(|f| &f.content) != made.as_ref().map(|f| &f.content) {
            return Err(ApiError::Conflict(format!(
                "'{path}' changed again after change {id}; undo the later change first"
            )));
        }
        let before = gitea.get_file(path, &pr.merge_base).await?.ok_or_else(|| {
            ApiError::Conflict(format!(
                "'{path}' is not in the commit change {id} was cut from; it cannot be undone"
            ))
        })?;
        let manifest = serde_norway::from_str::<ResourceEnvelope>(&before.content).ok();
        let kind = match &manifest {
            Some(envelope) => envelope.kind.clone(),
            None => crate::api::import::native_kind(path)
                .map(str::to_owned)
                .ok_or_else(|| {
                    ApiError::Conflict(format!(
                        "'{path}' belongs to no kind this platform serves, so it cannot be undone"
                    ))
                })?,
        };
        // The manifest put back is judged as a proposal of it is, its space and constraints
        // included; a native file by the kind its folder names (T-5698).
        match &manifest {
            Some(envelope) => crate::api::import::authorize_manifest(
                state, identity, project, &effective, envelope,
            )?,
            None => effective.check(&kind, Verb::Propose, None).map_err(|_| {
                ApiError::Denied(format!(
                    "undoing '{path}' needs `propose` on {kind} in project '{project}'"
                ))
            })?,
        }
        if let Some(envelope) = manifest {
            put_back.push(envelope);
        }
        uploads.push((path.clone(), before.content));
    }
    let Some(headline) = put_back.first() else {
        return Err(ApiError::Conflict(format!(
            "change {id} modified no manifest in project '{project}', so there is nothing to undo"
        )));
    };

    let held = crate::api::mutate::turn(gitea, &format!("portal/undo-{number:08x}")).await;
    let undo_branch =
        crate::api::mutate::create_or_reuse_branch(gitea, &held, &default_branch).await?;
    let (author_name, author_email) = crate::api::mutate::author_credentials(identity, project);
    let what = format!("{} {}", headline.kind, headline.metadata.name);
    gitea
        .change_files(
            &undo_branch,
            &format!("undo {id} on {what}"),
            Author {
                name: &author_name,
                email: &author_email,
            },
            &uploads,
            &[],
        )
        .await?;
    let opened = gitea
        .create_pull_request(
            &undo_branch,
            &default_branch,
            &format!("undo {id} on {what}"),
            &format!(
                "Puts back what change {id} altered in project `{project}`, as it was before: {}.",
                modified.join(", ")
            ),
        )
        .await?;
    let lane = put_back.iter().fold(Lane::Green, |lane, envelope| {
        crate::api::import::riskiest(lane, change::classify_manifest(envelope, Operation::Update))
    });
    let status = ChangeStatus::new(
        lane,
        ChangePhase::PendingApproval,
        PlanSummary::new(0, put_back.len(), 0),
    )
    .in_repository(&opened.repository)
    .proposed_through(&opened.body)
    .with_merge_request(opened.url);
    Ok(Change::new(
        change_meta(state, gitea, opened.number, project),
        status,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/changes/{id}/manifests",
    summary = "Read the Manifests a Change Wrote",
    description = "The manifests a merged change wrote, each read at the change's own last commit: the version a resource's editor offers to restore through a new, checked Change. A kind the caller does not read, a native file and a deleted file are left out (T-3975).",
    tag = "changes",
    params(
        ("project" = String, Path, description = "Project name"),
        ("id" = String, Path, description = "The merged change: chg- + 8 hex digits"),
    ),
    responses(
        (status = 200, description = "The manifests it wrote", body = ResourceList),
        (status = 400, description = "Not a change id", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No such change the caller may read", body = ProblemDetails),
        (status = 409, description = "Not merged", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn change_manifests(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, id)): Path<(String, String)>,
) -> Result<Json<ResourceList>, ApiError> {
    change_manifests_for(&state, &user.0.identity, &project, &id)
        .await
        .map(Json)
}

/// What the merged change `id` wrote, as `identity` may read it (T-3975). Nothing is written:
/// the editor loads a version into its form and proposes it like any edit.
pub async fn change_manifests_for(
    state: &AppState,
    identity: &Identity,
    project: &str,
    id: &str,
) -> Result<ResourceList, ApiError> {
    let (effective, forge, number, pr) = merged_change(
        state,
        identity,
        project,
        id,
        "what it proposed never was a version",
    )
    .await?;
    let gitea: &crate::git::GiteaClient = &forge;
    if pr.head_sha.is_empty() {
        return Err(ApiError::Unavailable(format!(
            "the forge did not say which commit change {id} ended on; try again"
        )));
    }
    let home = format!("projects/{project}/");
    let mut items = Vec::new();
    for file in gitea.pull_request_files(number).await? {
        if file.deleted || !file.path.starts_with(&home) {
            continue;
        }
        let Some(read) = gitea.get_file(&file.path, &pr.head_sha).await? else {
            continue;
        };
        // A native file (a LinkML source, a bento.yaml) is no manifest and is left out.
        let Ok(envelope) = serde_norway::from_str::<ResourceEnvelope>(&read.content) else {
            continue;
        };
        // A grant bound to one context space reads that space's manifests alone (T-5698).
        if effective.may_read_envelope(&envelope.kind, &envelope) {
            items.push(envelope);
        }
    }
    Ok(ResourceList {
        api_version: API_VERSION.to_owned(),
        kind: "List".to_owned(),
        metadata: ListMeta {
            continue_token: None,
            remaining_item_count: None,
        },
        items,
        next_cursor: None,
        total_count: None,
    })
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/projects/{project}/changes/{id}/restore",
            axum::routing::post(restore_change),
        )
        .route(
            "/projects/{project}/changes/{id}/undo",
            axum::routing::post(undo_change),
        )
        .route(
            "/projects/{project}/changes/{id}/manifests",
            axum::routing::get(change_manifests),
        )
}
