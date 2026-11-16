//! `POST /api/v1/projects/{project}/import?format=files` (T-6972): files of a project's
//! repository, as a person or a migration wrote them, landed as one Change.
//!
//! The bundle import rewrites what it lands (namespace, ids, the Policy assigner, a fresh
//! Endpoint slug), which is right for a bundle from another installation and wrong for a file
//! of this one: a forge file keeps `{orgDomain}` (CC-74) and an Endpoint keeps the slug people
//! already use. This door writes each file byte for byte at its path. It judges each one as the
//! loader renders it, with every check of the bundle import and the one judge (P-2, CHG-03), so
//! a file meets the same refusals here as at approval and on `main`.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

use axum::http::StatusCode;

use crate::api::import::{self, Incoming, Proposal, MAX_UPLOAD_BYTES};
use crate::auth::session::Identity;
use crate::change::{self, Lane, Operation};
use crate::error::ApiError;
use crate::resource::{self, ResourceEnvelope};
use crate::state::AppState;

/// What a manifest writes where the organization's domain goes; the loader renders it (CC-74).
const ORG_DOMAIN: &str = "{orgDomain}";

/// What a files import writes, by repository path, answered on a dry run and in the Change.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesReport {
    /// Paths `main` does not hold yet.
    pub created: Vec<String>,
    /// Paths whose file on `main` the upload replaces.
    pub replaced: Vec<String>,
    /// Paths whose file on `main` is already what the upload holds; nothing is written there.
    pub unchanged: Vec<String>,
    /// The riskiest lane of what is written (CC-63).
    pub lane: Lane,
}

/// The import: refused as a whole, or answered with its plan on a dry run, or one Change.
pub async fn import(
    state: &AppState,
    identity: &Identity,
    project: &str,
    bytes: &[u8],
    dry_run: bool,
) -> Result<(StatusCode, Value), ApiError> {
    let files = repository_files(bytes, project)?;
    let domain = crate::api::assistant::org_domain(state, project);
    let rendered: Vec<(String, String)> = files
        .iter()
        .map(|(path, text)| (path.clone(), text.replace(ORG_DOMAIN, &domain)))
        .collect();
    let incoming = judge(state, identity, project, &rendered)?;

    let shako = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let main = shako.default_branch().await?;
    let mut report = FilesReport {
        created: Vec::new(),
        replaced: Vec::new(),
        unchanged: Vec::new(),
        lane: Lane::Green,
    };
    let mut writes: Vec<(String, String)> = Vec::new();
    for (path, text) in files {
        match shako.get_file(&path, &main).await? {
            Some(held) if held.content == crate::forge::shako::committed_text(&path, &text) => {
                report.unchanged.push(path);
                continue;
            }
            Some(_) => report.replaced.push(path.clone()),
            None => report.created.push(path.clone()),
        }
        writes.push((path, text));
    }
    for item in &incoming {
        let (Some(path), Some(envelope)) = (&item.path, &item.envelope) else {
            continue;
        };
        let operation = if report.replaced.contains(path) {
            Operation::Update
        } else if report.created.contains(path) {
            Operation::Create
        } else {
            continue;
        };
        report.lane = import::riskiest(
            report.lane,
            change::classify(&envelope.kind, operation, &envelope.spec),
        );
    }

    // PF-57: the dry run is the check of exactly these bytes; the import needs it fresh.
    let subject = subject(bytes);
    if dry_run {
        crate::ops::record_import_check(state, identity, project, &subject).await;
        return Ok((StatusCode::OK, as_json(&report)?));
    }
    crate::ops::verdict_for_import(state, identity, project, &subject).await?;
    if writes.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "every file of the upload is already on {main}; there is nothing to change"
        )));
    }
    let hash = import::digest(&writes);
    let body = format!(
        "Files written as uploaded (format=files): {} created, {} replaced, {} already on {main}.\n\n{}",
        report.created.len(),
        report.replaced.len(),
        report.unchanged.len(),
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    let change = import::propose_files(
        state,
        identity,
        project,
        Proposal {
            branch: format!("portal/import-{project}-{hash:08x}"),
            title: format!("land {} repository files in {project}", writes.len()),
            body,
            lane: report.lane,
            summary: change::PlanSummary::new(report.created.len(), report.replaced.len(), 0),
            files: writes,
        },
    )
    .await?;
    crate::ops::forget_import_check(state, identity, project).await;
    Ok((StatusCode::ACCEPTED, as_json(&change)?))
}

/// Every refusal of the bundle import and the one judge over the files as the loader renders
/// them, all at once; the parsed files on success. Who proposes is asked per manifest, and the
/// manifests stand as the one Change they open (CC-08, PF-50, CHG-03).
fn judge(
    state: &AppState,
    identity: &Identity,
    project: &str,
    rendered: &[(String, String)],
) -> Result<Vec<Incoming>, ApiError> {
    let refused = import::judge_files(rendered, state.config.apps_dir.as_deref());
    if !refused.is_empty() {
        return Err(ApiError::BadRequest(refused.join("; ")));
    }
    let mut incoming = Vec::new();
    for (path, text) in rendered {
        incoming.extend(import::parse_text(text, Some(path))?);
    }
    let mut judged: Vec<ResourceEnvelope> = Vec::new();
    for item in &incoming {
        let (Some(path), Some(envelope)) = (&item.path, &item.envelope) else {
            continue;
        };
        let declared = envelope.metadata.namespace.as_deref();
        if resource::belongs_to_the_organization(&envelope.kind, declared)
            || declared.is_some_and(|namespace| namespace != project)
        {
            return Err(ApiError::BadRequest(format!(
                "{path}: {} {} is not a resource of project '{project}'; format=files lands the \
                 project's own files, and an organization file is changed in the organization \
                 repository",
                envelope.kind, envelope.metadata.name
            )));
        }
        let mut own = envelope.clone();
        own.metadata.namespace = Some(project.to_owned());
        judged.push(own);
    }
    let bundle = judged
        .iter()
        .filter_map(|e| serde_json::to_value(e).ok())
        .collect();
    crate::references::BUNDLE.sync_scope(bundle, || {
        import::authorize(state, identity, project, &incoming)
    })?;
    crate::judge::change(&crate::judge::Ground::of(state), project, &judged)?;
    Ok(incoming)
}

/// The archive's files as `(projects/{project}/…, text)`, each once. A path is the repository's
/// own (`spaces/air/endpoints/air.yaml`) or the Portal's (`projects/{project}/…`); another
/// project's path, a file that is not text and an unencrypted file under `secrets/` are refused.
fn repository_files(bytes: &[u8], project: &str) -> Result<Vec<(String, String)>, ApiError> {
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(ApiError::BadRequest(format!(
            "the upload is larger than the {MAX_UPLOAD_BYTES} byte import limit"
        )));
    }
    if !bytes.starts_with(b"PK\x03\x04") {
        return Err(ApiError::BadRequest(
            "format=files carries a zip archive of repository files, each at its path from the \
             repository root, e.g. spaces/air/endpoints/air.yaml"
                .into(),
        ));
    }
    let own = format!("projects/{project}/");
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    for (name, raw) in import::archive_entries(bytes)? {
        let path = match name.strip_prefix("projects/") {
            Some(_) if name.starts_with(&own) => name.clone(),
            Some(_) => {
                return Err(ApiError::BadRequest(format!(
                    "'{name}' is a file of another project; this import writes into project \
                     '{project}' only"
                )))
            }
            None => format!("{own}{name}"),
        };
        let text = raw
            .ok_or_else(|| ApiError::BadRequest(format!("'{name}' did not read from the archive")))
            .and_then(|raw| {
                String::from_utf8(raw).map_err(|_| {
                    ApiError::BadRequest(format!(
                        "'{name}' is not UTF-8 text; the repository's files are text"
                    ))
                })
            })?;
        if crate::secret_files::parse_path(&path).is_some() && !crate::secret_files::sealed(&text) {
            return Err(ApiError::Denied(format!(
                "'{name}' is under secrets/ but holds a value SOPS did not encrypt; enter the \
                 credential in the project's Secrets form instead (CC-06)"
            )));
        }
        if files.insert(path, text).is_some() {
            return Err(ApiError::BadRequest(format!(
                "'{name}' is in the archive twice"
            )));
        }
    }
    if files.is_empty() {
        return Err(ApiError::BadRequest("the archive holds no file".into()));
    }
    Ok(files.into_iter().collect())
}

/// What a files import's check is fresh for: the SHA-256 of the archive as sent.
fn subject(bytes: &[u8]) -> Value {
    use sha2::{Digest, Sha256};
    let archive: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    serde_json::json!({ "archive": archive, "format": "files" })
}

fn as_json<T: Serialize>(value: &T) -> Result<Value, ApiError> {
    serde_json::to_value(value).map_err(|e| ApiError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::repository_files;

    fn zipped(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, content) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .expect("entry");
            writer.write_all(content).expect("write");
        }
        writer.finish().expect("finish").into_inner()
    }

    fn refusal(bytes: &[u8]) -> String {
        match repository_files(bytes, "zilina") {
            Err(err) => err.to_string(),
            Ok(files) => panic!("accepted {files:?}"),
        }
    }

    #[test]
    fn a_repository_path_lands_under_the_project_and_a_portal_path_stays() {
        let files = repository_files(
            &zipped(&[
                ("spaces/doprava/endpoints/catalog-write.yaml", b"a: 1\n"),
                ("projects/zilina/spaces/doprava/space.yaml", b"b: 2\n"),
            ]),
            "zilina",
        )
        .expect("accepted");
        let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "projects/zilina/spaces/doprava/endpoints/catalog-write.yaml",
                "projects/zilina/spaces/doprava/space.yaml",
            ]
        );
    }

    #[test]
    fn another_projects_file_binary_and_plain_secrets_are_refused_by_name() {
        assert!(
            refusal(&zipped(&[("projects/praha/a.yaml", b"a: 1\n")])).contains("another project")
        );
        assert!(refusal(&zipped(&[("spaces/a/logo.png", &[0xff, 0xfe, 0x00])])).contains("UTF-8"));
        assert!(refusal(&zipped(&[("secrets/feed.enc.yaml", b"token: plain\n")])).contains("SOPS"));
        assert!(refusal(b"apiVersion: v1\n").contains("zip archive"));
    }

    #[test]
    fn one_path_twice_is_refused() {
        let twice = zipped(&[
            ("spaces/a/x.yaml", b"a: 1\n"),
            ("projects/zilina/spaces/a/x.yaml", b"a: 2\n"),
        ]);
        assert!(refusal(&twice).contains("twice"));
    }
}
