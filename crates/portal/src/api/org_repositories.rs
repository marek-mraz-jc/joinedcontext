//! Administration > Repositories (T-7409): every forge repository of the request's Organization,
//! each downloadable as a `.tar.gz` of its default branch's head. The forge keeps every
//! Organization of an installation under one owner, so the Portal decides which repositories are
//! this Organization's: its projects' own repositories, their Apps' repositories, and the
//! configuration repository for the installation's Organization alone, since that one holds every
//! Organization's settings. A repository outside that list is 404, the same answer as one that
//! does not exist (ISO-03). Every door answers the Organization's administrators only.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::forge::ShakoClient;
use crate::permissions::ORG_NAMESPACE;
use crate::state::AppState;

/// The largest archive the Portal streams: what an import reads back (MF-45). The organization
/// limit catalog has no repository class yet, so the import's bound is the one that holds.
pub const MAX_ARCHIVE_BYTES: u64 = crate::api::import::MAX_UPLOAD_BYTES as u64;

const REFUSAL: &str =
    "the Organization's repositories are for its administrators, on Administration > Repositories";

/// What a repository holds for the Organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum RepositoryRole {
    /// The installation's configuration repository: every Organization's settings.
    Configuration,
    /// A project's own repository: its spaces, endpoints, pipelines and models.
    Project,
    /// An App's source.
    Application,
}

/// One repository as Administration > Repositories lists it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationRepository {
    /// The forge's name, e.g. `ovzdusie_air-map`; the download names it.
    pub name: String,
    pub role: RepositoryRole,
    /// The project it belongs to; `null` for the configuration repository.
    pub project: Option<String>,
    /// The App whose source it is; `null` for any other repository.
    pub app: Option<String>,
    /// e.g. `main`
    pub default_branch: String,
    /// RFC 3339, when the repository last changed.
    pub updated_at: String,
    pub size_bytes: u64,
    pub archived: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationRepositories {
    pub organization: String,
    pub repositories: Vec<OrganizationRepository>,
    /// The largest archive a download streams, in bytes.
    pub max_archive_bytes: u64,
}

struct Owned {
    name: String,
    role: RepositoryRole,
    project: Option<String>,
    app: Option<String>,
    client: ShakoClient,
}

/// The repositories `organization` owns, from the Portal's mirror: no forge call, and nothing of
/// another Organization's projects.
fn owned(state: &AppState, organization: &str) -> Result<Vec<Owned>, ApiError> {
    let shako = state
        .shako
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("this Portal has no forge configured".into()))?;
    let mut out = Vec::new();
    if crate::permissions::installation_organization(&state.mirror).as_deref() == Some(organization)
    {
        out.push(Owned {
            name: shako.repo.clone(),
            role: RepositoryRole::Configuration,
            project: None,
            app: None,
            client: shako.for_repository(shako.repo.clone()),
        });
    }
    for project in crate::org_backup::projects_of(&state.mirror, organization) {
        if let Some(repository) = state.mirror.repository_of(&project) {
            out.push(Owned {
                client: shako.for_repository(repository.clone()),
                name: repository,
                role: RepositoryRole::Project,
                project: Some(project.clone()),
                app: None,
            });
        }
        let apps = state
            .mirror
            .list(&project, "App", &crate::store::ListOptions::default());
        for app in apps.items {
            // An App building from outside this forge has no repository here to list.
            if let Ok(Some(repository)) =
                crate::api::export_git::application_repository(&app, shako)
            {
                out.push(Owned {
                    client: shako.for_application(repository.clone()),
                    name: repository,
                    role: RepositoryRole::Application,
                    project: Some(project.clone()),
                    app: Some(app.metadata.name.clone()),
                });
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|repo| seen.insert(repo.name.clone()));
    Ok(out)
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/repositories",
    operation_id = "organization_repositories",
    summary = "List Organization Repositories",
    description = "Every forge repository of the request's Organization: its projects' repositories, their Apps' repositories and, for the installation's Organization, the configuration repository, with default branch, last change, size and whether it is archived. Administrators only (T-7409).",
    tag = "system",
    responses(
        (status = 200, description = "The Organization's repositories", body = OrganizationRepositories),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 503, description = "No forge, or the forge did not answer", body = ProblemDetails),
    )
)]
pub async fn list(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<OrganizationRepositories>, ApiError> {
    let organization =
        crate::api::org_backups::administered_for(&state, &user.0.identity, REFUSAL)?;
    let mut repositories = Vec::new();
    for repo in owned(&state, &organization)? {
        // A repository the mirror names but the forge lost is left out: nothing to download.
        let Some(info) = repo.client.repository_info().await? else {
            continue;
        };
        repositories.push(OrganizationRepository {
            name: repo.name,
            role: repo.role,
            project: repo.project,
            app: repo.app,
            default_branch: info.default_branch,
            updated_at: info.updated_at,
            size_bytes: info.size_bytes,
            archived: info.archived,
        });
    }
    Ok(Json(OrganizationRepositories {
        organization,
        repositories,
        max_archive_bytes: MAX_ARCHIVE_BYTES,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/repositories/{name}/archive",
    operation_id = "organization_repository_archive",
    summary = "Download Organization Repository",
    description = "One repository of the request's Organization as a `.tar.gz` of its default branch's head commit, streamed; the file name carries the repository and the commit (`{name}-{commit}.tar.gz`) and `X-Commit` names it too. A repository of another Organization is 404, as an unknown one is. Every download is one `repository.downloaded` activity entry. Administrators only (T-7409).",
    tag = "system",
    params(
        ("name" = String, Path, description = "The repository, e.g. `ovzdusie_air-map`"),
    ),
    responses(
        (status = 200, description = "The archive", content_type = "application/gzip"),
        (status = 403, description = "Not an administrator of the Organization", body = ProblemDetails),
        (status = 404, description = "No such repository in this Organization", body = ProblemDetails),
        (status = 413, description = "The archive is larger than the Portal streams", body = ProblemDetails),
        (status = 503, description = "No forge, or the forge did not answer", body = ProblemDetails),
    )
)]
pub async fn archive(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    let identity = &user.0.identity;
    let organization = crate::api::org_backups::administered_for(&state, identity, REFUSAL)?;
    let not_found = || ApiError::NotFound(format!("no repository '{name}' in this Organization"));
    let repo = owned(&state, &organization)?
        .into_iter()
        .find(|repo| repo.name == name)
        .ok_or_else(not_found)?;
    let info = repo.client.repository_info().await?.ok_or_else(not_found)?;
    // The archive is asked for by commit, so its content is the head this answer names even
    // when the branch moves meanwhile.
    let commit = repo.client.branch_head(&info.default_branch).await?;
    let response = repo
        .client
        .archive_response(&commit, Duration::from_secs(600))
        .await?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARCHIVE_BYTES)
    {
        return Err(too_large(&name));
    }
    audit(&state, identity, &organization, &name, &commit).await;
    let mut sent = 0u64;
    let body = response.bytes_stream().map(move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        sent += chunk.len() as u64;
        if sent > MAX_ARCHIVE_BYTES {
            // The forge sent no length: cut the transfer, the browser reports it incomplete.
            return Err(std::io::Error::other(
                "the archive passed the Portal's limit",
            ));
        }
        Ok(chunk)
    });
    Ok((
        [
            (header::CONTENT_TYPE, "application/gzip".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}-{commit}.tar.gz\""),
            ),
            (header::HeaderName::from_static("x-commit"), commit),
        ],
        axum::body::Body::from_stream(body),
    )
        .into_response())
}

fn too_large(name: &str) -> ApiError {
    ApiError::TooLarge(format!(
        "the archive of '{name}' is larger than {} MiB, the most the Portal streams; clone the \
         repository with git instead",
        MAX_ARCHIVE_BYTES / (1024 * 1024)
    ))
}

async fn audit(
    state: &AppState,
    identity: &crate::auth::session::Identity,
    organization: &str,
    repository: &str,
    commit: &str,
) {
    let event = crate::activity::ActivityEvent {
        time: jc_time::TimestampTz::now(),
        project: ORG_NAMESPACE.to_owned(),
        space: None,
        kind: "repository.downloaded".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "{} downloaded the repository {repository} at {commit}",
            identity.username
        ),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({
            "actor": identity.username,
            "organization": organization,
            "repository": repository,
            "commit": commit,
        }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(repository, error = %err, "the repository download is not on the activity feed");
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/organization/repositories", get(list))
        .route("/organization/repositories/{name}/archive", get(archive))
}
