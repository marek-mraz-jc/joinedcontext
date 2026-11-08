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
//! - `disableDeletion: false` (Grafana's default): a dashboard an earlier run provisioned from a
//!   provider's path whose file is gone is deleted; `true` keeps it.
//! - String values of the YAML files expand `$NAME` and `${NAME}` (`$$` is a literal `$`) from
//!   the variables the caller hands in, never from the process environment: the binary passes
//!   only `DAIYA_VAR_*` variables. An unset name is refused, not replaced by an empty string.
//! - Only organization 1 (the organization the loader is given) is provisioned; a provider or
//!   datasource file for another `orgId` is refused, never written to the wrong organization.

use crate::provisioning::{
    DashboardProviderConfig, parse_dashboard_providers_doc, parse_datasources_doc,
    safe_uid_from_name,
};
use crate::{DashboardImportInput, ImportReport, import_dashboards};
use chrono::Utc;
use daiya_model::{
    CrError, CrResult, DaiyaStore, DatasourceRecord, FolderRecord, InputLimits, InstallationId,
    InstanceId, OrgId, Verdict,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
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
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisioningLoadReport {
    pub datasources_upserted: usize,
    pub datasources_deleted: usize,
    pub folders: usize,
    pub dashboards: usize,
    pub dashboards_deleted: usize,
    pub reports: Vec<ImportReport>,
    /// The datasources as the files wrote them (left out of the printed report).
    #[serde(skip)]
    pub datasources: Vec<DatasourceRecord>,
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

/// The `orgId` / `org_id` values the entries of `list_key` name, as written.
fn org_ids(doc: &Value, list_key: &str) -> Vec<Value> {
    doc.get(list_key)
        .and_then(Value::as_array)
        .or_else(|| doc.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|item| item.get("orgId").or_else(|| item.get("org_id")))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Expands `$NAME`, `${NAME}` and `$$` in every string value of `doc` from `vars`.
///
/// # Errors
/// A name `vars` does not hold, or an unclosed `${`.
pub fn expand_variables(doc: &mut Value, vars: &BTreeMap<String, String>) -> CrResult<()> {
    match doc {
        Value::String(text) if text.contains('$') => *text = expand_text(text, vars)?,
        Value::Array(items) => {
            for item in items {
                expand_variables(item, vars)?;
            }
        }
        Value::Object(map) => {
            for value in map.values_mut() {
                expand_variables(value, vars)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn expand_text(text: &str, vars: &BTreeMap<String, String>) -> CrResult<String> {
    let is_name = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, tail) = if let Some(tail) = after.strip_prefix('$') {
            out.push('$');
            rest = tail;
            continue;
        } else if let Some(braced) = after.strip_prefix('{') {
            let end = braced.find('}').ok_or_else(|| {
                CrError::BadRequest(format!("provisioning: unclosed '${{' in '{text}'"))
            })?;
            (&braced[..end], &braced[end + 1..])
        } else {
            let end = after.find(|c: char| !is_name(c)).unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        if name.is_empty()
            || name.starts_with(|c: char| c.is_ascii_digit())
            || !name.chars().all(is_name)
        {
            // Not a variable reference (`$ 5`, `$-`): the `$` stays.
            out.push('$');
            rest = after;
            continue;
        }
        let value = vars.get(name).ok_or_else(|| {
            CrError::BadRequest(format!(
                "provisioning: variable '{name}' is not set (the server reads DAIYA_VAR_{name}); write $$ for a literal $"
            ))
        })?;
        out.push_str(value);
        rest = tail;
    }
    out.push_str(rest);
    Ok(out)
}

/// One provider's dashboards for one target folder, read and parsed but not yet written.
struct FolderBatch {
    provider: String,
    folder: Option<(String, String)>,
    inputs: Vec<DashboardImportInput>,
}

/// Loads `root` into `store` for `target`'s organization.
///
/// Every file is read and checked before anything is written: a refused datasource file or a
/// dashboard with a refused feature fails the run with nothing of that kind stored.
pub async fn load_provisioning_dir(
    store: &dyn DaiyaStore,
    target: ProvisioningTarget,
    root: &Path,
    limits: &InputLimits,
    vars: &BTreeMap<String, String>,
) -> CrResult<ProvisioningLoadReport> {
    let root = root.canonicalize().map_err(|e| io_err(root, &e))?;
    let mut report = ProvisioningLoadReport::default();

    let mut ds_outcomes = Vec::new();
    for file in yaml_files(&root.join("datasources"))? {
        let text = std::fs::read_to_string(&file).map_err(|e| io_err(&file, &e))?;
        // A `deleteDatasources` entry for another organization must not delete in this one.
        // Anything but the number 1 is refused, a quoted "2" included.
        let mut doc = crate::yaml::parse_yaml_to_json(&text)?;
        expand_variables(&mut doc, vars)?;
        if let Some(org) = ["datasources", "deleteDatasources", "delete_datasources"]
            .into_iter()
            .flat_map(|key| org_ids(&doc, key))
            .find(|o| o.as_i64() != Some(1))
        {
            return Err(CrError::BadRequest(format!(
                "provisioning: '{}' names orgId {org}; only orgId 1 is provisioned",
                file.display()
            )));
        }
        let outcome = parse_datasources_doc(
            &doc,
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
        ds_outcomes.push(outcome);
    }

    let mut batches = Vec::new();
    // Paths (below the root) of the providers that delete what their files no longer hold.
    let mut pruned: Vec<PathBuf> = Vec::new();
    for file in yaml_files(&root.join("dashboards"))? {
        let text = std::fs::read_to_string(&file).map_err(|e| io_err(&file, &e))?;
        let mut doc = crate::yaml::parse_yaml_to_json(&text)?;
        expand_variables(&mut doc, vars)?;
        for provider in parse_dashboard_providers_doc(&doc, Some(&root))? {
            batches.extend(read_provider(&root, &provider)?);
            if !provider.disable_deletion {
                let base = Path::new(&provider.path);
                let base = base.canonicalize().map_err(|e| io_err(base, &e))?;
                if let Ok(prefix) = base.strip_prefix(&root) {
                    pruned.push(prefix.to_path_buf());
                }
            }
        }
    }

    for outcome in ds_outcomes {
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
            store.upsert_datasource(ds.clone()).await?;
            report.datasources.push(ds);
            report.datasources_upserted += 1;
        }
    }

    // A dry run of every batch first, against the datasources just provisioned.
    for batch in &batches {
        let checked = import_dashboards(
            store,
            target.installation_id,
            target.org_id,
            target.instance_id,
            "provisioning",
            &batch.inputs,
            true,
            limits,
        )
        .await?;
        if checked.refused_count > 0 {
            return Err(CrError::Unprocessable {
                detail: format!(
                    "provisioning: provider '{}' has dashboards with refused features",
                    batch.provider
                ),
                errors: checked
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
    }

    let seen: BTreeSet<String> = batches
        .iter()
        .flat_map(|b| b.inputs.iter().filter_map(|i| i.external_id.clone()))
        .collect();
    for batch in batches {
        if let Some((uid, title)) = &batch.folder
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
                    managed_by: Some(format!("provisioning:{}", batch.provider)),
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
            &batch.inputs,
            false,
            limits,
        )
        .await?;
        report.dashboards += imported.dashboards.len();
        report.reports.push(imported);
    }
    if !pruned.is_empty() {
        for d in store.list_dashboards(target.org_id).await? {
            let gone = d.is_provisioned
                && d.provisioned_external_id.as_ref().is_some_and(|ext| {
                    !seen.contains(ext) && pruned.iter().any(|p| Path::new(ext).starts_with(p))
                });
            if gone && store.delete_dashboard(target.org_id, &d.uid).await? {
                report.dashboards_deleted += 1;
            }
        }
    }
    Ok(report)
}

/// Reads one file provider's dashboards, grouped by target folder.
fn read_provider(root: &Path, provider: &DashboardProviderConfig) -> CrResult<Vec<FolderBatch>> {
    if provider.org_id != 1 {
        return Err(CrError::BadRequest(format!(
            "provisioning: provider '{}' names orgId {}; only orgId 1 is provisioned",
            provider.name, provider.org_id
        )));
    }
    let base = PathBuf::from(&provider.path);
    let mut files = Vec::new();
    collect_json_files(root, &base, &base, 0, &mut files)?;
    files.sort();

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
            .strip_prefix(root)
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
    Ok(by_folder
        .into_iter()
        .map(|(folder, inputs)| FolderBatch {
            provider: provider.name.clone(),
            folder,
            inputs,
        })
        .collect())
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
