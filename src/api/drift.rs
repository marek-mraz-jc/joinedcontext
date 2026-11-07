//! Drift: what a space holds and the repository does not declare, or no longer holds as it
//! does (CC-21, CC-38, UI-25, UI-26; API/01 §20).
//!
//! - `GET  /api/v1/projects/{project}/drift` — what the last scan found, with its instant
//! - `POST /api/v1/projects/{project}/drift/{space}/{id}/revert` — write Git's entity back
//! - `POST /api/v1/projects/{project}/drift/{space}/{id}/adopt` — propose the live one
//!
//! Configuration cannot drift: every component reads it from the repository, so the repository
//! is what it is running (CC-72, and T-0421 settled it). What can is a space's seed entities,
//! and the reconciler compares them on its tick — the list is what that tick found, so a page
//! renders without waiting on a broker.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use std::hash::{Hash, Hasher};

use crate::auth::session::CurrentUser;
use crate::change::{Change, ChangePhase, ChangeStatus, Operation, PlanSummary};
use crate::error::ApiError;
use crate::error::ProblemDetails;
use crate::resource::is_dns1123;
use crate::state::AppState;
use jc_core::kinds::Verb;

const API_VERSION: &str = "joinedcontext.com/v1alpha1";

/// A project the caller is a member of, or `404`. Never `403`: a route that tells the two
/// apart says which projects exist (R20).
fn member_of(state: &AppState, user: &CurrentUser, project: &str) -> Result<(), ApiError> {
    if !is_dns1123(project) {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    if crate::permissions::for_request(state, &user.0.identity, project).may_read_project() {
        return Ok(());
    }
    Err(ApiError::NotFound(format!("project '{project}' not found")))
}

/// A resolution is a write: revert writes the live space, adopt writes the repository, so
/// neither is a read and both are held to `propose` on `Entity` (API/01 §20).
fn may_resolve(state: &AppState, user: &CurrentUser, project: &str) -> Result<(), ApiError> {
    member_of(state, user, project)?;
    crate::permissions::for_request(state, &user.0.identity, project).check(
        "Entity",
        Verb::Propose,
        None,
    )
}

/// The drifted entity the last scan found under this space and id, or `404`.
///
/// Resolving something the scan did not report would be a write nobody asked for: the list is
/// what the operator is looking at, and it is what the two buttons act on.
fn drifted(
    state: &AppState,
    project: &str,
    space: &str,
    id: &str,
) -> Result<crate::reconciler::drift::Drifted, ApiError> {
    state
        .drift
        .of(project)
        .and_then(|found| {
            found
                .entities
                .into_iter()
                .find(|entity| entity.space == space && entity.id == id)
        })
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "the last scan reported no drift for '{id}' in space '{space}'"
            ))
        })
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/drift",
    summary = "List Drift",
    description = "What the last scan found live in the project's spaces that differs from the repository. Members of the project only.",
    tag = "drift",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "What the last scan found"),
        (status = 401, description = "No session", body = ProblemDetails),
        (status = 404, description = "No such project, or not one the caller belongs to", body = ProblemDetails),
    ),
)]
pub async fn list_drift(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(project): Path<String>,
) -> Result<Json<Value>, ApiError> {
    member_of(&state, &user, &project)?;
    let mut found = state.drift.of(&project);
    // A space's drift (its declared and live values) reaches only the readers bound to that
    // space, as its activity does (T-4876, T-5698).
    if let (Some(found), Some(reach)) = (
        found.as_mut(),
        crate::permissions::for_request(&state, &user.0.identity, &project).spaces_reached(),
    ) {
        found
            .entities
            .retain(|entity| reach.contains(&entity.space));
    }
    // `observedAt` absent means no scan has run, which is a different answer from "nothing
    // drifted" and the page says so rather than showing an empty clean list.
    let metadata = match &found {
        Some(found) => json!({ "observedAt": found.observed_at.to_rfc3339() }),
        None => json!({}),
    };
    Ok(Json(json!({
        "apiVersion": API_VERSION,
        "kind": "List",
        "metadata": metadata,
        "items": found.map(|found| found.entities).unwrap_or_default(),
    })))
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/drift/{space}/{id}/revert",
    summary = "Revert Drift",
    description = "Writes the repository's entity back into the space, undoing a live edit the scan found. Needs `propose` on Entity; the scan must have reported this drift.",
    tag = "drift",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "The Context Space holding the entity"),
        ("id" = String, Path, description = "The entity's URN, percent-encoded"),
    ),
    responses(
        (status = 204, description = "Git's entity written back into the space"),
        (status = 401, description = "No session", body = ProblemDetails),
        (status = 403, description = "The caller may not propose an Entity in this project", body = ProblemDetails),
        (status = 404, description = "No such project, or the last scan reported no such drift", body = ProblemDetails),
        (status = 503, description = "The space or the repository cannot be reached now", body = ProblemDetails),
    ),
)]
pub async fn revert_drift(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((project, space, id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    may_resolve(&state, &user, &project)?;
    let entity = drifted(&state, &project, &space, &id)?;
    let watch = state.drift_watch.as_ref().ok_or_else(|| {
        ApiError::Unavailable(
            "no space surface is configured, so nothing can be written back".to_owned(),
        )
    })?;
    // What the file declares, and only that: revert never deletes an entity or an attribute,
    // so an attribute a device wrote beside a seeded one survives it (CC-19).
    watch
        .put(&space, &entity.declared)
        .await
        .map_err(ApiError::Unavailable)?;
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.clone(),
        space: Some(space.clone()),
        kind: "config.drifted".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "{} reverted '{id}' to what the repository declares",
            user.0.identity.username
        ),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({ "object": id, "resolution": "revert", "source": entity.source }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(entity = %id, error = %err, "the revert is not on the activity feed");
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/drift/{space}/{id}/adopt",
    summary = "Adopt Drift",
    description = "Proposes, as a change a person approves, the seed file with the entity's declared attributes as the space holds them now. Needs `propose` on Entity; the live entity is read with the caller's own access token, so the file holds only what their Policy shows; refused when the entity is no longer in the space.",
    tag = "drift",
    params(
        ("project" = String, Path, description = "Project name"),
        ("space" = String, Path, description = "The Context Space holding the entity"),
        ("id" = String, Path, description = "The entity's URN, percent-encoded"),
    ),
    responses(
        (status = 202, description = "The change that writes the live entity into its seed file, waiting for an approver", body = Change),
        (status = 401, description = "No session", body = ProblemDetails),
        (status = 403, description = "The caller may not propose an Entity in this project", body = ProblemDetails),
        (status = 404, description = "No such project, the last scan reported no such drift, or no live entity the caller may read (also when the session carries no access token of the person)", body = ProblemDetails),
        (status = 409, description = "The entity is not in the space or no longer in its seed file, or an adoption of it is already open", body = ProblemDetails),
        (status = 503, description = "The space or the repository cannot be reached now", body = ProblemDetails),
    ),
)]
pub async fn adopt_drift(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
    Path((project, space, id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    may_resolve(&state, &user, &project)?;
    let entity = drifted(&state, &project, &space, &id)?;
    if entity.drift == crate::reconciler::drift::Kind::Missing {
        // Adopting "it is gone" means proposing an empty file, which is a deletion and goes
        // through the explicit-deletion path rather than through a button (CC-19, UI-26).
        return Err(ApiError::Conflict(format!(
            "'{id}' is not in the space, so there is no live entity to adopt; \
             revert writes it back, and removing it from the repository is an explicit deletion"
        )));
    }
    // Read as the person, never with the Portal's own account: what their Policy hides of the
    // entity stays out of the file it proposes; a hidden and a gone entity answer the same 404
    // (T-5698).
    let live = crate::api::entity_comments::read_as_person(&state, &headers, &space, &id).await?;
    let seed_dir = format!("projects/{project}/spaces/{space}/entities/seed/");
    if crate::api::changes::seed_envelope(&entity.source, "{}").is_none()
        || !entity.source.starts_with(&seed_dir)
    {
        return Err(ApiError::Conflict(format!(
            "'{}' is no seed file of space '{space}', so adopting cannot rewrite it",
            entity.source
        )));
    }
    let gitea = state
        .forge_for(&project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let gitea: &crate::git::GiteaClient = &gitea;
    let default_branch = gitea.default_branch().await?;
    let file = gitea
        .get_file(&entity.source, &default_branch)
        .await?
        .ok_or_else(|| {
            ApiError::Conflict(format!(
                "'{}' is no longer in the repository; the next scan reports what is left",
                entity.source
            ))
        })?;
    let content = adopted(&file.content, &id, &live).map_err(ApiError::Conflict)?;
    let envelope = crate::api::changes::seed_envelope(&entity.source, &content)
        .ok_or_else(|| ApiError::Internal("the adopted seed file is no JSON".to_owned()))?;

    // Held until the merge request is open, so a second adoption finds it (F-POR2-011).
    let held = crate::api::mutate::turn(gitea, &adopt_branch(&project, &space, &id)).await;
    let branch = crate::api::mutate::create_or_reuse_branch(gitea, &held, &default_branch).await?;
    let (author_name, author_email) =
        crate::api::mutate::author_credentials(&user.0.identity, &project);
    let what = format!("adopt the live '{id}'");
    gitea
        .update_files(
            &branch,
            &what,
            crate::git::gitea::Author {
                name: &author_name,
                email: &author_email,
            },
            &[(entity.source.clone(), content, file.sha)],
        )
        .await?;
    let opened = gitea
        .create_pull_request(
            &branch,
            &default_branch,
            &what,
            &format!(
                "Writes into `{}` what space `{space}` holds for `{id}` now, for every attribute \
                 the file declares, so the drift ends when this merges.",
                entity.source
            ),
        )
        .await?;
    let status = ChangeStatus::new(
        crate::change::classify_manifest(&envelope, Operation::Update),
        ChangePhase::PendingApproval,
        PlanSummary::new(0, 1, 0),
    )
    .in_repository(&opened.repository)
    .proposed_through(&opened.body)
    .with_merge_request(opened.url);
    Ok((
        StatusCode::ACCEPTED,
        Json(Change::new(
            crate::api::changes::change_meta(&state, gitea, opened.number, &project),
            status,
        )),
    ))
}

/// The branch an adoption lives on: one per entity, so a second adoption while the first is
/// open finds it (409) instead of opening a second change.
fn adopt_branch(project: &str, space: &str, id: &str) -> String {
    let mut hasher = std::hash::DefaultHasher::new();
    (project, space, id).hash(&mut hasher);
    format!("portal/adopt-{:016x}", hasher.finish())
}

/// What the space's members own and no seed file declares.
const BROKER_OWNED: [&str; 4] = ["createdAt", "modifiedAt", "deletedAt", "instanceId"];

/// The seed file `text` with entity `id`'s declared attributes as `live` holds them: each one
/// the file declares takes the live value, one the space no longer holds leaves the file, and
/// what the space holds beside them (a device's own attribute) stays out. Every other entity of
/// the file and the file's layout (one entity or an array) stay as they are.
fn adopted(text: &str, id: &str, live: &Value) -> Result<String, String> {
    let mut document: Value =
        serde_json::from_str(text).map_err(|e| format!("the seed file is no JSON: {e}"))?;
    let entities = match &mut document {
        Value::Array(items) => items.iter_mut().collect::<Vec<_>>(),
        one @ Value::Object(_) => vec![one],
        _ => Vec::new(),
    };
    let declared = entities
        .into_iter()
        .find(|entity| entity.get("id").and_then(Value::as_str) == Some(id))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("the seed file no longer declares '{id}'"))?;
    let names: Vec<String> = declared
        .keys()
        .filter(|name| !matches!(name.as_str(), "id" | "type" | "@context"))
        .cloned()
        .collect();
    for name in names {
        match live.get(&name) {
            Some(value) => {
                let mut value = value.clone();
                if let Some(members) = value.as_object_mut() {
                    members.retain(|member, _| !BROKER_OWNED.contains(&member.as_str()));
                }
                declared.insert(name, value);
            }
            None => {
                declared.remove(&name);
            }
        }
    }
    let mut out = serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(out)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{project}/drift", get(list_drift))
        .route(
            "/projects/{project}/drift/{space}/{id}/revert",
            post(revert_drift),
        )
        .route(
            "/projects/{project}/drift/{space}/{id}/adopt",
            post(adopt_drift),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopting_rewrites_only_the_declared_attributes_of_the_one_entity() {
        let file = json!({ "id": "urn:a", "type": "T",
            "kept": { "type": "Property", "value": 1 },
            "gone": { "type": "Property", "value": 2 } })
        .to_string();
        let live = json!({ "id": "urn:a", "type": "T",
            "kept": { "type": "Property", "value": 5, "createdAt": "x", "observedAt": "y" },
            "device": { "type": "Property", "value": 9 } });
        let out: Value =
            serde_json::from_str(&adopted(&file, "urn:a", &live).expect("adopted")).expect("json");
        assert_eq!(
            out,
            json!({ "id": "urn:a", "type": "T",
                "kept": { "type": "Property", "value": 5, "observedAt": "y" } })
        );
        assert!(adopted(&file, "urn:b", &live).is_err(), "not in the file");
        assert!(adopted("not json", "urn:a", &live).is_err());
        assert!(adopted("[1, 2]", "urn:a", &live).is_err());
    }

    #[test]
    fn one_entity_has_one_adoption_branch() {
        assert_eq!(
            adopt_branch("p", "s", "urn:a"),
            adopt_branch("p", "s", "urn:a")
        );
        assert_ne!(
            adopt_branch("p", "s", "urn:a"),
            adopt_branch("p", "s", "urn:b")
        );
        assert!(adopt_branch("p", "s", "urn:a").starts_with("portal/adopt-"));
    }
}
