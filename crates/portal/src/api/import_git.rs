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
use crate::forge::ShakoClient;
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
pub fn unpack(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, ApiError> {
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
    // verify_files found every file bundle.yaml lists, project.yaml among them.
    let own = String::from_utf8_lossy(files.get("project.yaml").map_or(&[][..], Vec::as_slice));
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
    let organization = crate::api::projects::opening_organization(state, identity, None)?;
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
    let shako = state
        .shako
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    crate::api::projects::new_project_name(state, shako, project, &organization).await?;
    let Checked { index, files, own } = check(bytes)?;
    let origin = index.metadata.name.clone();

    // The registry entry this deployment runs the project with, and its values checked against
    // what the project declares before anything is created (CC-88).
    let mut entry_files = crate::api::projects::open_project_files(
        identity,
        &crate::api::projects::OpenProject {
            name: project.to_owned(),
            display_name: input.display_name.clone(),
            description: None,
            organization: None,
        },
        project,
        &organization,
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
        crate::ops::record_import_check(state, identity, project, &subject).await?;
        let plan = GitImportPlan {
            repositories: planned,
            parameters: serde_json::to_value(&own.spec.parameters).map_err(ApiError::internal)?,
            models,
        };
        return Ok((
            StatusCode::OK,
            serde_json::to_value(plan).map_err(ApiError::internal)?,
        ));
    }
    crate::ops::verdict_for_import(state, identity, project, &subject).await?;

    // Nothing is created while one of the names is taken: a repository already there is
    // somebody's, and is never adopted (PF-86).
    let clients: Vec<ShakoClient> = planned
        .iter()
        .map(|plan| {
            // An application's repository goes where the generated ones live (PF-106).
            if plan.role == "application" {
                shako.for_application(plan.repository.clone())
            } else {
                shako.for_repository(plan.repository.clone())
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

    let mut created: Vec<&ShakoClient> = Vec::new();
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
                serde_json::to_value(change).map_err(ApiError::internal)?,
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
    clients: &'a [ShakoClient],
    models: &'a [crate::api::bundle_models::PlannedModel],
    artifacts: &'a BTreeMap<String, crate::tools::model_tools::Artifacts>,
}

/// Creates, fills and verifies every repository, remounts the project, and proposes the
/// registry entry; `created` records each repository as soon as it exists.
async fn land<'a>(
    state: &AppState,
    identity: &Identity,
    landing: &Landing<'a>,
    created: &mut Vec<&'a ShakoClient>,
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
            // Every manifest at the pushed head passes the gates a zip import and a proposal
            // pass, before `main` is protected or anything names the project; a refusal removes
            // what was created (CHG-05, MF-24). ponytail: the head only, not every blob of the
            // history; a history scan needs the pack read before the push.
            let refused = judge_head(state, client, &head).await?;
            if !refused.is_empty() {
                return Err(ApiError::Invalid {
                    detail: format!(
                        "the project at {head} holds {} manifest refusal(s); fix them in the \
                         source project and export again",
                        refused.len()
                    ),
                    errors: refused,
                });
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

/// The refusals of every YAML file of the project repository at `head` (see
/// [`crate::api::import::judge_files`]).
async fn judge_head(
    state: &AppState,
    client: &ShakoClient,
    head: &str,
) -> Result<Vec<String>, ApiError> {
    let mut files = Vec::new();
    for path in client.list_tree(head).await? {
        if !(path.ends_with(".yaml") || path.ends_with(".yml")) {
            continue;
        }
        if let Some(file) = client.get_file(&path, head).await? {
            files.push((path, file.content));
        }
    }
    Ok(crate::api::import::judge_files(
        &files,
        state.config.apps_dir.as_deref(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const HEAD: &str = "2c245c6bb1efa7a20938c5b11b70379be6838eec";
    const OWN: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata:\n  \
                       name: ovzdusie\n  namespace: org\nspec:\n  organizationRef: bb\n";

    fn bundle_file(head: &str) -> Vec<u8> {
        format!("# v2 git bundle\n{head} refs/heads/bundle\n{head} HEAD\n\nPACK").into_bytes()
    }

    fn zipped(files: &[(String, Vec<u8>)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (path, bytes) in files {
            writer.start_file(path.as_str(), options).expect("entry");
            writer.write_all(bytes).expect("write");
        }
        writer.finish().expect("zip").into_inner()
    }

    /// An export of project `ovzdusie` whose bundle.yaml lists `repositories`, written after
    /// `edit` changed the files the index checksums; `listed` drops files from the index.
    fn export(
        own: &str,
        repositories: serde_json::Value,
        listed: impl Fn(&str) -> bool,
        edit: impl Fn(&mut Vec<(String, Vec<u8>)>),
    ) -> Vec<u8> {
        let mut files: Vec<(String, Vec<u8>)> = vec![
            ("ovzdusie.bundle".into(), bundle_file(HEAD)),
            ("project.yaml".into(), own.as_bytes().to_vec()),
        ];
        let index = serde_json::json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "Bundle",
            "metadata": { "name": "ovzdusie", "namespace": "org" },
            "spec": {
                "exportedAt": "2026-09-22T12:00:00Z",
                "exportedBy": "jana",
                "sourceRevision": HEAD,
                "items": [],
                "files": files.iter().filter(|(path, _)| listed(path)).map(|(path, bytes)| serde_json::json!({
                    "path": path, "sha256": format!("{:x}", Sha256::digest(bytes))
                })).collect::<Vec<_>>(),
                "omitted": 0,
                "repositories": repositories,
            },
        });
        edit(&mut files);
        files.push((
            "bundle.yaml".into(),
            serde_norway::to_string(&index).expect("yaml").into_bytes(),
        ));
        zipped(&files)
    }

    fn project_only() -> serde_json::Value {
        serde_json::json!([{ "name": "ovzdusie", "role": "project", "file": "ovzdusie.bundle", "head": HEAD }])
    }

    fn refusal(bytes: &[u8]) -> String {
        check(bytes).map_or_else(
            |err| err.to_string().replacen("bad request: ", "", 1),
            |_| "accepted".to_owned(),
        )
    }

    /// MF-42, MF-45, MF-46, MF-47: each archive a format=git export would not write is refused
    /// with the reason, before anything is created.
    #[test]
    fn an_archive_no_export_writes_is_refused_with_its_reason() {
        let all = |_: &str| true;
        let keep = |_: &mut Vec<(String, Vec<u8>)>| {};
        assert_eq!(refusal(&export(OWN, project_only(), all, keep)), "accepted");
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (b"not a zip".to_vec(), "not a zip archive"),
            (
                zipped(&[("README.md".into(), b"hi".to_vec())]),
                "no bundle.yaml",
            ),
            (
                zipped(&[("bundle.yaml".into(), b"kind: [".to_vec())]),
                "not a Bundle",
            ),
            (
                export(
                    OWN,
                    serde_json::json!([{ "name": "Bad Name", "role": "project", "file": "ovzdusie.bundle", "head": HEAD }]),
                    all,
                    keep,
                ),
                "bundle.yaml",
            ),
            (
                export(
                    OWN,
                    serde_json::json!([
                        { "name": "ovzdusie", "role": "project", "file": "ovzdusie.bundle", "head": HEAD },
                        { "name": "org", "role": "organization", "file": "ovzdusie.bundle", "head": HEAD }
                    ]),
                    all,
                    keep,
                ),
                "organization repository",
            ),
            (
                export(
                    OWN,
                    serde_json::json!([{ "name": "air-map", "role": "application", "file": "ovzdusie.bundle", "head": HEAD }]),
                    all,
                    keep,
                ),
                "no project repository",
            ),
            (
                export(OWN, project_only(), all, |files| {
                    files.retain(|(p, _)| p != "project.yaml")
                }),
                "which the archive does not hold",
            ),
            (
                export(OWN, project_only(), |p| p != "ovzdusie.bundle", keep),
                "carries no checksum in bundle.yaml",
            ),
            (
                export(
                    OWN,
                    serde_json::json!([{ "name": "ovzdusie", "role": "project", "file": "ovzdusie.bundle", "head": "b47f8079d2df9e7986d2babf4539cec802f4e68e" }]),
                    all,
                    keep,
                ),
                "does not end at",
            ),
            (
                export(OWN, project_only(), |p| p != "project.yaml", keep),
                "no checksum of project.yaml",
            ),
            (
                export(&format!("{OWN}  nonsense: 1\n"), project_only(), all, keep),
                "project.yaml does not load",
            ),
            (
                export(
                    &format!("{OWN}  repository: {{ name: ovzdusie }}\n"),
                    project_only(),
                    all,
                    keep,
                ),
                "registry entry",
            ),
        ];
        for (bytes, needle) in cases {
            let why = refusal(&bytes);
            assert!(why.contains(needle), "{needle}: {why}");
        }
    }

    /// An archive of more entries than a git export writes, an entry leaving the root or one
    /// unpacking past the upload limit is refused before it is read whole; a directory is skipped.
    #[test]
    fn unpack_refuses_too_many_entries_an_escape_and_a_bomb_and_skips_directories() {
        let many: Vec<(String, Vec<u8>)> =
            (0..257).map(|i| (format!("f{i}"), Vec::new())).collect();
        assert!(
            matches!(unpack(&zipped(&many)), Err(ApiError::BadRequest(why)) if why.contains("257 entries"))
        );
        let escape = zipped(&[("../outside".into(), b"x".to_vec())]);
        assert!(
            matches!(unpack(&escape), Err(ApiError::BadRequest(why)) if why.contains("leaves the archive"))
        );
        let bomb = zipped(&[("zeros".into(), vec![0u8; MAX_UPLOAD_BYTES + 1])]);
        assert!(
            matches!(unpack(&bomb), Err(ApiError::BadRequest(why)) if why.contains("unpacks past"))
        );
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .add_directory("dir/", zip::write::SimpleFileOptions::default())
            .expect("a directory");
        writer
            .start_file("dir/a", zip::write::SimpleFileOptions::default())
            .expect("entry");
        writer.write_all(b"a").expect("write");
        let files = unpack(&writer.finish().expect("zip").into_inner()).expect("unpacked");
        assert_eq!(files.keys().collect::<Vec<_>>(), ["dir/a"]);
    }

    #[test]
    fn a_tags_file_that_is_not_text_is_refused() {
        assert!(
            matches!(tags(&[0xff, 0xfe], "p"), Err(ApiError::BadRequest(why)) if why == "p.tags is not text")
        );
    }

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

    /// T-7917, MF-05: an entry whose local header is not a zip header is a 400 naming its index.
    #[test]
    fn unpack_refuses_an_entry_with_a_broken_header_naming_its_index() {
        let mut bytes = zipped(&[("a.yaml".into(), b"x".to_vec())]);
        bytes[2] = 0x09;
        let refused = unpack(&bytes).expect_err("a broken local header");
        assert!(
            matches!(&refused, ApiError::BadRequest(why) if why.starts_with("archive entry 0:")),
            "{refused:?}"
        );
    }

    /// T-7917, MF-42: a stream whose checksum does not hold is a 400 naming the entry, not a
    /// half-read archive.
    #[test]
    fn unpack_refuses_an_entry_whose_stream_does_not_read_back() {
        let payload = b"unreadable-stream-payload";
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("a.yaml", options).expect("entry");
        writer.write_all(payload).expect("write");
        let mut bytes = writer.finish().expect("zip").into_inner();
        let at = bytes
            .windows(payload.len())
            .position(|window| window == &payload[..])
            .expect("a stored body sits in the archive as it is");
        bytes[at] ^= 0xff;
        let refused = unpack(&bytes).expect_err("a stream that fails its checksum");
        assert!(
            matches!(&refused, ApiError::BadRequest(why) if why.starts_with("archive entry 'a.yaml':")),
            "{refused:?}"
        );
    }
}
