#![forbid(unsafe_code)]
//! Loads a Grafana provisioning directory into a store (R2, §7.3): `datasources/*.yaml` and
//! `dashboards/*.yaml` (file providers) under one root, as `pkg/services/provisioning` does at
//! boot.
//!
//! - Datasources: `deleteDatasources` first, then every datasource upserted; a file that carries
//!   raw `secureJsonData` is refused as a whole (credentials are `secretRefs`).
//! - Dashboards: each provider's path must stay inside the root (no `..`, no symlink out, per
//!   file as well); `*.json` files are read recursively, imported as provisioned with the
//!   provider's `allowUiUpdates`, into the provider's folder or, with
//!   `foldersFromFilesStructure`, into one folder per sub-directory.
//! - Only organization 1 (the organization the loader is given) is provisioned; a provider or
//!   datasource file for another `orgId` is refused, never written to the wrong organization.

use crate::provisioning::{
    parse_dashboard_providers_yaml, parse_datasources_yaml, safe_uid_from_name,
};
use crate::{DashboardImportInput, ImportReport, import_dashboards};
use chrono::Utc;
use daiya_model::{
    CrError, CrResult, DaiyaStore, FolderRecord, InputLimits, InstallationId, InstanceId, OrgId,
    Verdict,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Most files a provider directory may hold; a larger tree is refused, not truncated.
pub const MAX_PROVISIONED_FILES: usize = 10_000;
/// Deepest directory level read below a provider path.
const MAX_DEPTH: usize = 8;

/// Where provisioning writes to.
#[derive(Debug, Clone, Copy)]
pub struct ProvisioningTarget {
    pub installation_id: InstallationId,
    pub org_id: OrgId,
    pub instance_id: InstanceId,
}

/// What one provisioning run wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningLoadReport {
    pub datasources_upserted: usize,
    pub datasources_deleted: usize,
    pub folders: usize,
    pub dashboards: usize,
    pub reports: Vec<ImportReport>,
}

fn io_err(path: &Path, e: &std::io::Error) -> CrError {
    CrError::BadRequest(format!(
        "provisioning: cannot read '{}': {e}",
        path.display()
    ))
}

/// The `*.yaml` / `*.yml` files of `dir`, sorted; none when `dir` does not exist.
fn yaml_files(dir: &Path) -> CrResult<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| io_err(dir, &e))? {
        let path = entry.map_err(|e| io_err(dir, &e))?.path();
        if path.is_file()
            && matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("yaml" | "yml")
            )
        {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn yaml_org_ids(text: &str, list_key: &str) -> CrResult<Vec<i64>> {
    let doc = crate::yaml::parse_yaml_to_json(text)?;
    let list = doc
        .get(list_key)
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| doc.as_array().cloned())
        .unwrap_or_default();
    Ok(list
        .iter()
        .filter_map(|item| item.get("orgId").or_else(|| item.get("org_id")))
        .filter_map(Value::as_i64)
        .collect())
}

/// Loads `root` into `store` for `target`'s organization.
pub async fn load_provisioning_dir(
    store: &dyn DaiyaStore,
    target: ProvisioningTarget,
    root: &Path,
    limits: &InputLimits,
) -> CrResult<ProvisioningLoadReport> {
    let root = root.canonicalize().map_err(|e| io_err(root, &e))?;
    let mut report = ProvisioningLoadReport::default();

    for file in yaml_files(&root.join("datasources"))? {
        let text = std::fs::read_to_string(&file).map_err(|e| io_err(&file, &e))?;
        if let Some(org) = yaml_org_ids(&text, "datasources")?
            .into_iter()
            .find(|o| *o != 1)
        {
            return Err(CrError::BadRequest(format!(
                "provisioning: '{}' names orgId {org}; only orgId 1 is provisioned",
                file.display()
            )));
        }
        let outcome = parse_datasources_yaml(
            &text,
            target.installation_id,
            target.org_id,
            target.instance_id,
        )?;
        if let Some(refusal) = outcome
            .findings
            .iter()
            .find(|f| f.verdict == Verdict::Refuses)
        {
            return Err(CrError::BadRequest(format!(
                "provisioning: '{}': {}",
                file.display(),
                refusal.message
            )));
        }
        for name in &outcome.delete_names {
            if let Some(ds) = store.get_datasource_by_name(target.org_id, name).await? {
                // A provisioned datasource is read-only to the UI, not to provisioning.
                let mut writable = ds.clone();
                writable.read_only = false;
                store.upsert_datasource(writable).await?;
                if store.delete_datasource(target.org_id, &ds.uid).await? {
                    report.datasources_deleted += 1;
                }
            }
        }
        for ds in outcome.datasources {
            store.upsert_datasource(ds).await?;
            report.datasources_upserted += 1;
        }
    }

    for file in yaml_files(&root.join("dashboards"))? {
        let text = std::fs::read_to_string(&file).map_err(|e| io_err(&file, &e))?;
        let providers = parse_dashboard_providers_yaml(&text, Some(&root))?;
        for provider in providers {
            if provider.org_id != 1 {
                return Err(CrError::BadRequest(format!(
                    "provisioning: provider '{}' names orgId {}; only orgId 1 is provisioned",
                    provider.name, provider.org_id
                )));
            }
            let base = PathBuf::from(&provider.path);
            let mut files = Vec::new();
            collect_json_files(&root, &base, &base, 0, &mut files)?;
            files.sort();

            // Group by target folder.
            let mut by_folder: BTreeMap<Option<(String, String)>, Vec<DashboardImportInput>> =
                BTreeMap::new();
            for (rel_dir, path) in files {
                let text = std::fs::read_to_string(&path).map_err(|e| io_err(&path, &e))?;
                let raw_json: Value = serde_json::from_str(&text).map_err(|e| {
                    CrError::BadRequest(format!(
                        "provisioning: '{}' is not JSON: {e}",
                        path.display()
                    ))
                })?;
                let folder = if provider.folders_from_files_structure {
                    rel_dir
                        .filter(|d| !d.is_empty())
                        .map(|d| (safe_uid_from_name(&d), d))
                } else if !provider.folder_uid.is_empty() || !provider.folder.is_empty() {
                    let title = if provider.folder.is_empty() {
                        provider.folder_uid.clone()
                    } else {
                        provider.folder.clone()
                    };
                    let uid = if provider.folder_uid.is_empty() {
                        safe_uid_from_name(&provider.folder)
                    } else {
                        provider.folder_uid.clone()
                    };
                    Some((uid, title))
                } else {
                    None
                };
                let external_id = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                by_folder
                    .entry(folder.clone())
                    .or_default()
                    .push(DashboardImportInput {
                        raw_json,
                        folder_uid: folder.map(|(uid, _)| uid),
                        is_provisioned: true,
                        allow_ui_updates: provider.allow_ui_updates,
                        external_id: Some(external_id),
                    });
            }

            for (folder, inputs) in by_folder {
                if let Some((uid, title)) = &folder
                    && store.get_folder(target.org_id, uid).await?.is_none()
                {
                    let now = Utc::now();
                    store
                        .upsert_folder(FolderRecord {
                            installation_id: target.installation_id,
                            org_id: target.org_id,
                            instance_id: target.instance_id,
                            project_id: None,
                            uid: uid.clone(),
                            parent_uid: None,
                            title: title.clone(),
                            version: 1,
                            managed_by: Some(format!("provisioning:{}", provider.name)),
                            created_by: "provisioning".into(),
                            updated_by: "provisioning".into(),
                            created_at: now,
                            updated_at: now,
                        })
                        .await?;
                    report.folders += 1;
                }
                let imported = import_dashboards(
                    store,
                    target.installation_id,
                    target.org_id,
                    target.instance_id,
                    "provisioning",
                    &inputs,
                    false,
                    limits,
                )
                .await?;
                if imported.refused_count > 0 {
                    return Err(CrError::Unprocessable {
                        detail: format!(
                            "provisioning: provider '{}' has dashboards with refused features",
                            provider.name
                        ),
                        errors: imported
                            .dashboards
                            .iter()
                            .flat_map(|d| {
                                d.findings
                                    .iter()
                                    .filter(|f| f.verdict == Verdict::Refuses)
                                    .map(|f| format!("{}: {}", d.uid, f.message))
                            })
                            .collect(),
                    });
                }
                report.dashboards += imported.dashboards.len();
                report.reports.push(imported);
            }
        }
    }
    Ok(report)
}

/// Collects `*.json` files below `dir` (relative directory name for folder mapping), each
/// confirmed inside `root` after resolving symlinks.
fn collect_json_files(
    root: &Path,
    base: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<(Option<String>, PathBuf)>,
) -> CrResult<()> {
    if depth > MAX_DEPTH {
        return Err(CrError::BadRequest(format!(
            "provisioning: '{}' is nested deeper than {MAX_DEPTH} levels",
            dir.display()
        )));
    }
    if !dir.is_dir() {
        return Err(CrError::BadRequest(format!(
            "provisioning: dashboard path '{}' is not a directory",
            dir.display()
        )));
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| io_err(dir, &e))?
        .map(|e| e.map(|e| e.path()).map_err(|err| io_err(dir, &err)))
        .collect::<CrResult<_>>()?;
    entries.sort();
    for path in entries {
        let resolved = path.canonicalize().map_err(|e| io_err(&path, &e))?;
        if !resolved.starts_with(root) {
            return Err(CrError::Forbidden(format!(
                "provisioning: '{}' leads outside the provisioning root",
                path.display()
            )));
        }
        if resolved.is_dir() {
            collect_json_files(root, base, &path, depth + 1, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            if out.len() >= MAX_PROVISIONED_FILES {
                return Err(CrError::BadRequest(format!(
                    "provisioning: more than {MAX_PROVISIONED_FILES} dashboard files"
                )));
            }
            // Grafana names the folder after the file's own directory (`filepath.Base`), the
            // provider path itself being the General folder.
            let rel_dir = path
                .parent()
                .filter(|p| *p != base)
                .and_then(Path::file_name)
                .map(|p| p.to_string_lossy().into_owned());
            out.push((rel_dir, resolved));
        }
    }
    Ok(())
}
