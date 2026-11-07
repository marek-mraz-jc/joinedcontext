//! A project of layout 2 exported as Git (MF-45, MF-42, CC-88).
//!
//! The archive carries one `git bundle` per repository, the project's and each application
//! repository its App manifests name, the project's registry entry with no parameter values,
//! and the platform's `kind: Bundle` index listing each bundle with the head commit it ends at.
//! The Portal holds no git: each bundle is the forge's own of the default branch, which carries
//! that branch's whole history and no other ref, so the tags travel beside it as
//! `{name}.tags`, one `{commit} refs/tags/{name}` line per tag, and the import sets them again.

use std::io::Write;

use jc_core::kinds::{Bundle, BundleFile, BundleRepository, BundleRole, BundleSpec};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::ApiError;
use crate::git::GiteaClient;
use crate::state::AppState;

/// The archive and the head commit of the project repository it was taken at.
pub struct Archive {
    pub bytes: Vec<u8>,
    pub head: String,
}

/// The commit a bundle's `HEAD` names, from its header: `# v2 git bundle`, then one
/// `{commit} {ref}` line per ref, then a blank line and the pack.
pub fn bundle_head(bundle: &[u8]) -> Option<String> {
    let end = bundle.windows(2).position(|pair| pair == b"\n\n")?;
    let header = std::str::from_utf8(&bundle[..end]).ok()?;
    let mut lines = header.lines();
    if !lines.next()?.starts_with("# v2 git bundle") {
        return None;
    }
    lines
        .filter_map(|line| line.split_once(' '))
        .find(|(_, name)| *name == "HEAD")
        .map(|(commit, _)| commit.to_owned())
        .filter(|commit| commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The forge repository an App's `source.git` names, `None` for an App with no `source.git`. One
/// that is not in this forge's applications organization is refused by name, because an export
/// that left it out would import an application with no source (MF-45).
pub(crate) fn application_repository(
    app: &crate::resource::ResourceEnvelope,
    forge: &GiteaClient,
) -> Result<Option<String>, ApiError> {
    let Some(url) = app.spec.pointer("/source/git/url").and_then(Value::as_str) else {
        return Ok(None);
    };
    let mut segments = url.trim_end_matches('/').rsplit('/');
    let name = segments.next().unwrap_or_default().trim_end_matches(".git");
    let owner = segments.next().unwrap_or_default();
    let applications = forge.applications().owner;
    if owner != applications || name.is_empty() {
        return Err(ApiError::Conflict(format!(
            "the App '{}' builds from {url}, which is not a repository of this forge's \
             organization '{applications}'; a git export carries only the forge's \
             repositories (MF-45)",
            app.metadata.name
        )));
    }
    Ok(Some(name.to_owned()))
}

/// Every application repository of `project`, as `(App name, forge repository)`. The index lists
/// it under the App's name, a DNS-1123 label as the index requires, where the forge's is
/// `{project}_{app}` (AP-75).
fn application_repositories(
    state: &AppState,
    project: &str,
    forge: &GiteaClient,
) -> Result<Vec<(String, String)>, ApiError> {
    let apps = state
        .mirror
        .list(project, "App", &crate::store::ListOptions::default());
    let mut names = Vec::new();
    for app in apps.items {
        if let Some(repository) = application_repository(&app, forge)? {
            names.push((app.metadata.name.clone(), repository));
        }
    }
    names.sort();
    Ok(names)
}

/// One repository's default branch as the forge bundles it: the bundle, the head it ends at, and
/// its tags as `{commit} refs/tags/{tag}` lines (empty when it has none). A bundle that ends
/// elsewhere than the head read before it is refused: the branch moved (MF-46).
pub(crate) async fn take(
    name: &str,
    repository: &GiteaClient,
) -> Result<(Vec<u8>, String, String), ApiError> {
    let branch = repository.default_branch().await?;
    let head = repository.branch_head(&branch).await?;
    let bundle = repository.bundle(&branch).await?;
    match bundle_head(&bundle) {
        Some(bundled) if bundled == head => {}
        Some(bundled) => {
            return Err(ApiError::Conflict(format!(
                "the repository '{name}' moved from {head} to {bundled} during the export; \
                 export again"
            )))
        }
        None => {
            return Err(ApiError::Internal(format!(
                "the forge's bundle of '{name}' is not a git bundle"
            )))
        }
    }
    let tags: String = repository
        .list_tags()
        .await?
        .into_iter()
        .map(|(tag, commit)| format!("{commit} refs/tags/{tag}\n"))
        .collect();
    Ok((bundle, head, tags))
}

/// The files as one zip archive, stored: a bundle is a packfile, compressed already.
pub(crate) fn stored_zip(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, ApiError> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (path, bytes) in entries {
        writer
            .start_file(path, stored)
            .map_err(|err| ApiError::Internal(format!("archive entry failed: {err}")))?;
        writer
            .write_all(bytes)
            .map_err(|e| ApiError::Internal(format!("archive write failed: {e}")))?;
    }
    let cursor = writer
        .finish()
        .map_err(|err| ApiError::Internal(format!("archive did not close: {err}")))?;
    Ok(cursor.into_inner())
}

/// `total` plus `more` bytes, refused past what an import takes: an export no import reads back
/// is refused while it is assembled, before every bundle sits in memory at once (F-R2POR2-023).
fn within_import(total: usize, more: usize) -> Result<usize, ApiError> {
    let total = total.saturating_add(more);
    if total > crate::api::import::MAX_UPLOAD_BYTES {
        return Err(ApiError::TooLarge(format!(
            "the project's repositories bundle to more than {} bytes, the most an import reads \
             back; move large files out of the application repositories (MF-45)",
            crate::api::import::MAX_UPLOAD_BYTES
        )));
    }
    Ok(total)
}

/// The export of `project`, whose own repository `forge` speaks to (MF-45).
pub async fn archive(
    state: &AppState,
    forge: &GiteaClient,
    project: &str,
    exported_by: &str,
) -> Result<Archive, ApiError> {
    let project_repository = forge.for_repository(forge.repo.clone());
    let mut sources = vec![(forge.repo.clone(), project_repository, BundleRole::Project)];
    for (app, repository) in application_repositories(state, project, forge)? {
        sources.push((
            app,
            forge.for_application(repository),
            BundleRole::Application,
        ));
    }

    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut repositories = Vec::new();
    let mut total = 0usize;
    for (name, repository, role) in sources {
        let (bundle, head, tags) = take(&name, &repository).await?;
        total = within_import(total, bundle.len())?;
        let file = format!("{name}.bundle");
        entries.push((file.clone(), bundle));
        // The project's own file at that head, where an import reads the parameters it declares
        // without unpacking the bundle (CC-88).
        if role == BundleRole::Project {
            let own = forge
                .get_file(&format!("projects/{project}/project.yaml"), &head)
                .await?
                .ok_or_else(|| {
                    ApiError::Conflict(format!(
                        "the repository '{name}' has no project.yaml at {head} (PF-86)"
                    ))
                })?;
            entries.push(("project.yaml".to_owned(), own.content.into_bytes()));
        }
        if !tags.is_empty() {
            entries.push((format!("{name}.tags"), tags.into_bytes()));
        }
        repositories.push(BundleRepository {
            name,
            role,
            file,
            head,
        });
    }

    // The registry entry with no parameter values: the target sets its own (CC-88).
    let organization = state
        .mirror
        .get(crate::permissions::ORG_NAMESPACE, "Project", project)
        .and_then(|entry| entry.spec.get("organizationRef").cloned())
        .ok_or_else(|| {
            ApiError::Unavailable(format!(
                "the registry entry of '{project}' is not in the mirror yet"
            ))
        })?;
    let entry = serde_json::json!({
        "apiVersion": jc_core::API_VERSION,
        "kind": "Project",
        "metadata": { "name": project, "namespace": crate::permissions::ORG_NAMESPACE },
        "spec": {
            "organizationRef": organization.clone(),
            "repository": { "name": project },
            "ref": "main",
        },
    });
    let entry = serde_norway::to_string(&entry)
        .map_err(|e| ApiError::Internal(format!("the registry entry did not serialise: {e}")))?;
    entries.push((format!("projects/{project}.yaml"), entry.into_bytes()));

    // Every organization model the project imports travels with it, schema files only (MF-49).
    let organization_name = serde_json::from_value::<jc_core::envelope::Ref>(organization)
        .map_err(|e| ApiError::Internal(format!("the registry entry's organizationRef: {e}")))?;
    let (models, model_files) =
        crate::api::bundle_models::carry(state, project, organization_name.name()).await?;
    entries.extend(model_files);

    let files = entries
        .iter()
        .map(|(path, bytes)| BundleFile {
            path: path.clone(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
        })
        .collect();
    let head = repositories[0].head.clone();
    let index = Bundle {
        api_version: jc_core::API_VERSION.to_owned(),
        kind: "Bundle".to_owned(),
        metadata: jc_core::ObjectMeta {
            name: project.to_owned(),
            namespace: Some(crate::permissions::ORG_NAMESPACE.to_owned()),
            ..Default::default()
        },
        spec: BundleSpec {
            exported_at: chrono::Utc::now(),
            exported_by: exported_by.to_owned(),
            source_instance: None,
            source_revision: head.clone(),
            items: Vec::new(),
            native_files: Vec::new(),
            files,
            omitted: 0,
            readme: None,
            schemas: None,
            repositories,
            models,
        },
        status: None,
    };
    index
        .validate()
        // Two repositories under one name (an App named like its project) cannot be told apart
        // at the target.
        .map_err(|e| ApiError::Conflict(format!("the project cannot be exported as git: {e}")))?;
    let index = serde_norway::to_string(&index)
        .map_err(|e| ApiError::Internal(format!("the bundle index did not serialise: {e}")))?;
    entries.push(("bundle.yaml".to_owned(), index.into_bytes()));

    Ok(Archive {
        bytes: stored_zip(&entries)?,
        head,
    })
}

#[cfg(test)]
mod tests {
    use super::{bundle_head, within_import};

    /// F-R2POR2-023 (T-6226): an export is refused once its bundles add up past what an import
    /// reads back, naming the limit, and passes below it.
    #[test]
    fn an_export_past_what_an_import_reads_is_refused() {
        let max = crate::api::import::MAX_UPLOAD_BYTES;
        assert_eq!(within_import(0, max).expect("at the limit"), max);
        let refused = within_import(max, 1).expect_err("past the limit");
        assert!(
            matches!(&refused, crate::error::ApiError::TooLarge(why) if why.contains(&max.to_string())),
            "{refused:?}"
        );
        assert!(within_import(usize::MAX, 1).is_err());
    }

    const HEAD: &str = "2c245c6bb1efa7a20938c5b11b70379be6838eec";

    #[test]
    fn the_head_is_read_from_the_bundle_header() {
        let bundle = format!("# v2 git bundle\n{HEAD} refs/heads/bundle\n{HEAD} HEAD\n\nPACK\0\0");
        assert_eq!(bundle_head(bundle.as_bytes()).as_deref(), Some(HEAD));
    }

    #[test]
    fn what_is_not_a_bundle_has_no_head() {
        for bytes in [
            b"PACK".as_slice(),
            b"# v2 git bundle\nrefs only\n\n".as_slice(),
            b"# v3 git bundle\n".as_slice(),
            format!("# v1 something\n{HEAD} HEAD\n\n").as_bytes(),
            b"".as_slice(),
        ] {
            assert_eq!(
                bundle_head(bytes),
                None,
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }
}
