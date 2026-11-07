//! A project of layout 2 imported from its git export (MF-45, MF-46, MF-47, MF-42, CC-85, CC-88).
//!
//! The archive of `GET …/export?format=git` lands as a new project: each repository created
//! empty and filled by pushing its bundle, each head read back against the index, the project
//! remounted onto its new slug when it differs, and the registry entry with this deployment's
//! parameter values proposed as the organization's `Change`. Both or neither: a failure after
//! the first repository exists removes every repository the import created.

use std::collections::BTreeMap;
use std::io::Read;

use axum::http::StatusCode;
use jc_core::kinds::{Bundle, BundleRole, Project};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::api::import::{enclosed_path, ImportReport, Verified, MAX_UPLOAD_BYTES};
use crate::auth::session::Identity;
use crate::error::ApiError;
use crate::git::GiteaClient;
use crate::state::AppState;

/// What a `format=git` import is given besides the archive.
#[derive(Debug, Default)]
pub struct GitImport {
    /// Values for the parameters `project.yaml` declares (CC-88).
    pub parameters: serde_json::Map<String, Value>,
    pub display_name: Option<String>,
    /// `format=app` only: the App's name in the project it lands in (UI-87).
    pub name: Option<String>,
    pub dry_run: bool,
}

/// One repository a git import creates.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlannedRepository {
    /// The name the index lists it under.
    pub name: String,
    /// `project` or `application`.
    pub role: String,
    /// The repository it becomes in this forge.
    pub repository: String,
    /// The commit its bundle ends at, and its `main` after the push.
    pub head: String,
}

/// The dry run of a git import: what it would create, and the parameters the project declares,
/// which the import form is drawn from (CC-88).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GitImportPlan {
    pub repositories: Vec<PlannedRepository>,
    /// `spec.parameters` of the project's own file, name to declaration.
    #[schema(value_type = Object)]
    pub parameters: Value,
    /// What the import does with each organization model the archive carries (MF-50).
    pub models: Vec<crate::api::bundle_models::PlannedModel>,
}

/// The archive's files by path. An entry that leaves the root, or an archive past the upload
/// limits, is refused before anything is read into the import.
pub(crate) fn unpack(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, ApiError> {
    const MAX_ENTRIES: usize = 256;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|err| ApiError::BadRequest(format!("the upload is not a zip archive: {err}")))?;
    if archive.len() > MAX_ENTRIES {
        return Err(ApiError::BadRequest(format!(
            "a git export holds a bundle per repository; {} entries is not one",
            archive.len()
        )));
    }
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|err| ApiError::BadRequest(format!("archive entry {index}: {err}")))?;
        if entry.is_dir() {
            continue;
        }
        let Some(path) = enclosed_path(&entry).map(|p| p.to_string_lossy().into_owned()) else {
            return Err(ApiError::BadRequest(format!(
                "the archive entry '{}' leaves the archive",
                // zip 9: a name that is not UTF-8 is an error, not a reason to say nothing
                entry
                    .name()
                    .unwrap_or_else(|_| "(a name that is not text)".into())
            )));
        };
        let mut content = Vec::new();
        (&mut entry)
            .take((MAX_UPLOAD_BYTES - total + 1) as u64)
            .read_to_end(&mut content)
            .map_err(|err| ApiError::BadRequest(format!("archive entry '{path}': {err}")))?;
        total += content.len();
        if total > MAX_UPLOAD_BYTES {
            return Err(ApiError::BadRequest(format!(
                "the archive unpacks past {} MiB",
                MAX_UPLOAD_BYTES / (1024 * 1024)
            )));
        }
        files.insert(path, content);
    }
    Ok(files)
}

/// The tags of `{name}.tags`, `(commit, refs/tags/{tag})`; a line that is not one is refused.
pub(crate) fn tags(text: &[u8], name: &str) -> Result<Vec<(String, String)>, ApiError> {
    let text = std::str::from_utf8(text)
        .map_err(|_| ApiError::BadRequest(format!("{name}.tags is not text")))?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (commit, reference) = line.split_once(' ').unwrap_or((line, ""));
            let tag = reference.strip_prefix("refs/tags/").unwrap_or_default();
            let commit_ok = commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit());
            let tag_ok = !tag.is_empty()
                && !tag.contains("..")
                && tag
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/'));
            if commit_ok && tag_ok {
                Ok((commit.to_ascii_lowercase(), reference.to_owned()))
            } else {
                Err(ApiError::BadRequest(format!(
                    "{name}.tags: '{line}' is not `{{commit}} refs/tags/{{tag}}`"
                )))
            }
        })
        .collect()
}

/// MF-42, MF-46: every file the index names is there and is what was exported, every bundle it
/// lists carries a checksum, and each bundle ends at the head the index lists; nothing else counts.
pub(crate) fn verify_files(
    index: &Bundle,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ApiError> {
    for listed in &index.spec.files {
        let bytes = files.get(&listed.path).ok_or_else(|| {
            ApiError::BadRequest(format!(
                "bundle.yaml lists {}, which the archive does not hold",
                listed.path
            ))
        })?;
        if format!("{:x}", Sha256::digest(bytes)) != listed.sha256 {
            return Err(ApiError::BadRequest(format!(
                "{} is not the file that was exported: its SHA-256 differs from bundle.yaml (MF-42)",
                listed.path
            )));
        }
    }
    for repository in &index.spec.repositories {
        if !index.spec.files.iter().any(|f| f.path == repository.file) {
            return Err(ApiError::BadRequest(format!(
                "the bundle of '{}' carries no checksum in bundle.yaml (MF-42)",
                repository.name
            )));
        }
        if crate::api::export_git::bundle_head(&files[&repository.file]).as_deref()
            != Some(repository.head.as_str())
        {
            return Err(ApiError::BadRequest(format!(
                "{} does not end at {}, the head bundle.yaml lists for '{}' (MF-46)",
                repository.file, repository.head, repository.name
            )));
        }
    }
    Ok(())
}

/// Every check that runs before anything is created: the index, each file's checksum, the
/// project's own file and the parameters against it (MF-42, MF-47, CC-88).
struct Checked {
    index: Bundle,
    files: BTreeMap<String, Vec<u8>>,
    own: Project,
}

fn check(bytes: &[u8]) -> Result<Checked, ApiError> {
    let files = unpack(bytes)?;
    let index = files.get("bundle.yaml").ok_or_else(|| {
        ApiError::BadRequest(
            "the archive has no bundle.yaml; import the archive of a format=git export \
                 (MF-45)"
                .into(),
        )
    })?;
    let index: Bundle = serde_norway::from_slice(index)
        .map_err(|err| ApiError::BadRequest(format!("bundle.yaml is not a Bundle: {err}")))?;
    index
        .validate()
        .map_err(|err| ApiError::BadRequest(format!("bundle.yaml: {err}")))?;
    let roles: Vec<BundleRole> = index.spec.repositories.iter().map(|r| r.role).collect();
    if roles.contains(&BundleRole::Organization) {
        return Err(ApiError::BadRequest(
            "the archive carries an organization repository; a project import lands one \
             project and its applications (MF-45)"
                .into(),
        ));
    }
    if roles
        .iter()
        .filter(|role| **role == BundleRole::Project)
        .count()
        != 1
    {
        return Err(ApiError::BadRequest(
            "bundle.yaml lists no project repository, or more than one (MF-45)".into(),
        ));
    }
    verify_files(&index, &files)?;
    if !index.spec.files.iter().any(|f| f.path == "project.yaml") {
        return Err(ApiError::BadRequest(
            "bundle.yaml carries no checksum of project.yaml; export again (MF-42)".into(),
        ));
    }
    let own = files.get("project.yaml").ok_or_else(|| {
        ApiError::BadRequest(
            "the archive has no project.yaml beside its bundles; export it again (MF-45)".into(),
        )
    })?;
    let own = String::from_utf8_lossy(own);
    // MF-47: this release reads its own apiVersion; an older export goes through `jcctl
    // migrate`, which needs git the Portal does not hold, and a newer one is refused.
    let api_version = serde_norway::from_str::<Value>(&own)
        .ok()
        .and_then(|value| {
            value
                .get("apiVersion")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    if api_version.as_deref() != Some(jc_core::API_VERSION) {
        return Err(ApiError::BadRequest(format!(
            "project.yaml is {}, and this release reads {}: migrate an older one with `jcctl \
             migrate` and export it again; a newer one waits for a newer release (MF-47)",
            api_version.as_deref().unwrap_or("of no apiVersion"),
            jc_core::API_VERSION
        )));
    }
    let own = Project::from_yaml(&own)
        .map_err(|err| ApiError::BadRequest(format!("project.yaml does not load: {err}")))?;
    if own.spec.is_registry_entry() {
        return Err(ApiError::BadRequest(
            "project.yaml is a registry entry, not the project's own file".into(),
        ));
    }
    Ok(Checked { index, files, own })
}

/// The import itself, behind `POST /api/v1/projects/{project}/import?format=git`.
pub async fn import(
    state: &AppState,
    identity: &Identity,
    project: &str,
    bytes: &[u8],
    input: GitImport,
) -> Result<(StatusCode, Value), ApiError> {
    crate::api::projects::may_open(state, identity)?;
    if input.name.is_some() {
        return Err(ApiError::BadRequest(
            "a project import takes its name from the path; 'name' belongs to format=app".into(),
        ));
    }
    if state.mirror.layout() != 2 {
        return Err(ApiError::Conflict(
            "a git import lands a project in a repository of its own, and this organization \
             is of layout 1; import the zip export instead (MF-45)"
                .into(),
        ));
    }
    let gitea = state
        .gitea
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    crate::api::projects::new_project_name(state, gitea, project).await?;
    let Checked { index, files, own } = check(bytes)?;
    let origin = index.metadata.name.clone();

    // The registry entry this deployment runs the project with, and its values checked against
    // what the project declares before anything is created (CC-88).
    let mut entry_files = crate::api::projects::open_project_files(
        state,
        identity,
        &crate::api::projects::OpenProject {
            name: project.to_owned(),
            display_name: input.display_name.clone(),
            description: None,
        },
        project,
    )?;
    crate::api::projects::into_registry_entry(
        &mut entry_files,
        project,
        project,
        Some(&input.parameters),
    )?;
    let entry_path = format!("projects/{project}.yaml");
    let entry = entry_files
        .iter()
        .find(|(path, _)| *path == entry_path)
        .map(|(_, text)| text.clone())
        .ok_or_else(|| ApiError::Internal("the registry entry is missing".into()))?;
    let entry = Project::from_yaml(&entry)
        .map_err(|err| ApiError::BadRequest(format!("the registry entry does not load: {err}")))?;
    jc_core::project::resolve_parameters(&own.spec, &entry.spec)
        .map_err(|err| ApiError::BadRequest(format!("{err} (CC-88)")))?;

    let mut planned = Vec::new();
    for repository in &index.spec.repositories {
        let (role, target) = match repository.role {
            BundleRole::Project => ("project", project.to_owned()),
            _ => {
                if let Some(refusal) =
                    crate::agents::repository::name_refusal(project, &repository.name)
                {
                    return Err(ApiError::BadRequest(refusal));
                }
                (
                    "application",
                    crate::agents::repository::name(project, &repository.name),
                )
            }
        };
        planned.push(PlannedRepository {
            name: repository.name.clone(),
            role: role.to_owned(),
            repository: target,
            head: repository.head.clone(),
        });
    }

    // Map or land, per carried model, decided before anything is created (MF-50).
    let (models, artifacts) = crate::api::bundle_models::plan(state, &index, &files).await?;

    // PF-57: the dry run is the check, over the archive as sent, the slug and the values.
    let subject = serde_json::json!({
        "bundle": format!("{:x}", Sha256::digest(bytes)),
        "format": "git",
        "targetNamespace": project,
        "parameters": input.parameters,
        "displayName": input.display_name,
    });
    if input.dry_run {
        crate::ops::record_import_check(state, identity, project, &subject).await;
        let plan = GitImportPlan {
            repositories: planned,
            parameters: serde_json::to_value(&own.spec.parameters)
                .map_err(|e| ApiError::Internal(e.to_string()))?,
            models,
        };
        return Ok((
            StatusCode::OK,
            serde_json::to_value(plan).map_err(|e| ApiError::Internal(e.to_string()))?,
        ));
    }
    crate::ops::verdict_for_import(state, identity, project, &subject).await?;

    // Nothing is created while one of the names is taken: a repository already there is
    // somebody's, and is never adopted (PF-86).
    let clients: Vec<GiteaClient> = planned
        .iter()
        .map(|plan| {
            // An application's repository goes where the generated ones live (PF-106).
            if plan.role == "application" {
                gitea.for_application(plan.repository.clone())
            } else {
                gitea.for_repository(plan.repository.clone())
            }
        })
        .collect();
    for (plan, client) in planned.iter().zip(&clients) {
        if client.default_branch().await.is_ok() {
            return Err(ApiError::Conflict(format!(
                "a repository named '{}' is already in the forge; an import lands only in new \
                 ones (PF-86)",
                plan.repository
            )));
        }
    }

    let mut created: Vec<&GiteaClient> = Vec::new();
    let landing = Landing {
        project,
        origin: &origin,
        index: &index,
        files: &files,
        planned: &planned,
        clients: &clients,
        models: &models,
        artifacts: &artifacts,
    };
    let landed = land(state, identity, &landing, &mut created, entry_files).await;
    match landed {
        Ok(change) => {
            crate::ops::forget_import_check(state, identity, project).await;
            Ok((
                StatusCode::ACCEPTED,
                serde_json::to_value(change).map_err(|e| ApiError::Internal(e.to_string()))?,
            ))
        }
        // Both or neither (CC-85): what this import created goes again.
        Err(error) => {
            for client in created {
                if let Err(cleanup) = client.delete_repository().await {
                    tracing::error!(repository = %client.repo, error = %cleanup, "a repository of an import that failed is left behind");
                }
            }
            Err(error)
        }
    }
}

/// What one import lands: the checked archive and the repositories it becomes.
struct Landing<'a> {
    project: &'a str,
    origin: &'a str,
    index: &'a Bundle,
    files: &'a BTreeMap<String, Vec<u8>>,
    planned: &'a [PlannedRepository],
    clients: &'a [GiteaClient],
    models: &'a [crate::api::bundle_models::PlannedModel],
    artifacts: &'a BTreeMap<String, crate::tools::model_tools::Artifacts>,
}

/// Creates, fills and verifies every repository, remounts the project, and proposes the
/// registry entry; `created` records each repository as soon as it exists.
async fn land<'a>(
    state: &AppState,
    identity: &Identity,
    landing: &Landing<'a>,
    created: &mut Vec<&'a GiteaClient>,
    entry_files: Vec<(String, String)>,
) -> Result<crate::change::Change, ApiError> {
    let Landing {
        project,
        origin,
        index,
        files,
        planned,
        clients,
        models,
        artifacts,
    } = *landing;
    let mut verified = Vec::new();
    let mut refused_tags = Vec::new();
    for ((repository, plan), client) in index.spec.repositories.iter().zip(planned).zip(clients) {
        if !client
            .create_empty_repository(&format!("{} of the project {project}", plan.role))
            .await?
        {
            return Err(ApiError::Conflict(format!(
                "a repository named '{}' appeared in the forge during the import",
                plan.repository
            )));
        }
        created.push(client);
        let mut refs = vec![(repository.head.clone(), "refs/heads/main".to_owned())];
        if let Some(listed) = files.get(&format!("{}.tags", repository.name)) {
            refs.extend(tags(listed, &repository.name)?);
        }
        // A tag whose commit the bundle does not hold (one off the default branch) is refused on
        // its own and reported; the branch is the import.
        for refused in client.push_bundle(&files[&repository.file], &refs).await? {
            if refused.starts_with("refs/heads/") {
                return Err(ApiError::Conflict(format!(
                    "the forge refused {refused} of '{}'",
                    plan.repository
                )));
            }
            refused_tags.push(format!("{}: {refused}", plan.repository));
        }
        // MF-46: the head the forge now holds is the head the index lists.
        let head = client.branch_head("main").await?;
        if head != repository.head {
            return Err(ApiError::Conflict(format!(
                "the repository '{}' ends at {head} after the push, and bundle.yaml lists {} \
                 (MF-46)",
                plan.repository, repository.head
            )));
        }
        // The parameters were checked against the archive's project.yaml; the one the pushed
        // head holds is that file, or the import stops (CC-88).
        if repository.role == BundleRole::Project {
            let pushed = client
                .for_project(client.repo.clone(), project)
                .get_file(&format!("projects/{project}/project.yaml"), &head)
                .await?
                .map(|file| file.content.into_bytes());
            if pushed.as_deref() != files.get("project.yaml").map(Vec::as_slice) {
                return Err(ApiError::Conflict(format!(
                    "the project.yaml at {head} is not the one beside the bundle; export again \
                     (MF-46)"
                )));
            }
        }
        verified.push(Verified {
            path: repository.file.clone(),
            equal: true,
        });
    }

    // Under another slug the project is remounted in one commit; an Endpoint slug another
    // project here serves is drawn anew, and every App builds from its new repository.
    let project_client = clients
        .iter()
        .zip(planned)
        .find(|(_, plan)| plan.role == "project")
        .map(|(client, _)| client.clone())
        .ok_or_else(|| ApiError::Internal("the project repository is missing".into()))?;
    project_client.let_the_reader_in().await?;
    let mounted = project_client.for_project(project_client.repo.clone(), project);
    let applications: BTreeMap<String, String> = clients
        .iter()
        .zip(planned)
        .filter(|(_, plan)| plan.role == "application")
        .map(|(client, plan)| {
            (
                crate::agents::repository::name(origin, &plan.name),
                client.clone_url(),
            )
        })
        .collect();
    let served: Vec<String> = state
        .mirror
        .matching(|envelope| envelope.kind == "Endpoint")
        .into_iter()
        .filter_map(|envelope| {
            envelope
                .spec
                .get("slug")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    let fresh = |slug: &str| served.iter().any(|taken| taken == slug);
    crate::api::projects::remount_copy(
        &mounted,
        identity,
        &crate::api::projects::Remount {
            origin,
            name: project,
            fresh_slug: &fresh,
            applications: &applications,
            keep_secrets: true,
            message: format!("Import {origin} as {project} (MF-45)"),
        },
    )
    .await?;
    // The carried models the destination does not hold land in the project, before `main` is
    // protected; the organization repository is never written (MF-50).
    crate::api::bundle_models::land(&mounted, identity, project, index, files, models, artifacts)
        .await?;
    for client in clients {
        client.protect_branch("main").await?;
    }

    let report = ImportReport {
        created: entry_files.iter().map(|(path, _)| path.clone()).collect(),
        replaced: Vec::new(),
        skipped: refused_tags,
        renamed: Default::default(),
        reassigned: Default::default(),
        native_files: 0,
        lane: crate::change::Lane::Yellow,
        source: Some(format!("{origin} at {}", index.spec.source_revision)),
        verified,
        needs: Vec::new(),
    };
    crate::api::import::propose_bundle(
        state,
        identity,
        project,
        report,
        entry_files,
        Some(("Project", project)),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::tags;

    const COMMIT: &str = "3a386a2d5ff9fc1e18d3032677ae5bbf2ea5d4e7";

    #[test]
    fn a_tags_file_is_read_as_refs() {
        let text = format!("{COMMIT} refs/tags/v1.0.0\n\n{COMMIT} refs/tags/release/2\n");
        assert_eq!(
            tags(text.as_bytes(), "p").expect("tags"),
            [
                (COMMIT.to_owned(), "refs/tags/v1.0.0".to_owned()),
                (COMMIT.to_owned(), "refs/tags/release/2".to_owned()),
            ]
        );
    }

    #[test]
    fn a_line_that_is_not_a_tag_is_refused() {
        for line in [
            format!("{COMMIT} refs/heads/main"),
            format!("{COMMIT} refs/tags/../../heads/main"),
            format!("{COMMIT} refs/tags/"),
            "abc refs/tags/v1".to_owned(),
            COMMIT.to_owned(),
            format!("{COMMIT} refs/tags/v1 extra"),
        ] {
            assert!(tags(line.as_bytes(), "p").is_err(), "{line}");
        }
    }
}
