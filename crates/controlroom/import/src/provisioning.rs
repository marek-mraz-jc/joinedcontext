#![forbid(unsafe_code)]
//! Grafana provisioning YAML parser and validator (`GF pkg/services/provisioning/{dashboards,datasources}`):
//! - Dashboard providers (`dashboardproviders.yaml` v0 and v1).
//! - Datasources (`datasources.yaml` v0 and v1, including `safeUIDFromName` SHA-256 fallback).
//! - Enforces uniqueness of default datasource per organization (`ErrInvalidConfigToManyDefault`).
//! - Refuses raw credentials in `secureJsonData` / `secure_json_data` (requiring `secretRefs` instead, §7.1).
//! - Refuses path traversal (`..`, absolute escape outside allowed base, symlink escape) in dashboard
//!   provider paths (CVE-2021-43798 / R12).

use crate::yaml::parse_yaml_to_json;
use chrono::Utc;
use controlroom_model::{
    CrError, CrResult, DatasourceRecord, FeatureFinding, InstallationId, InstanceId, OrgId,
    Verdict, sha256_hex,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// Parsed dashboard provider entry (`GF pkg/services/provisioning/dashboards/types.go:config`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardProviderConfig {
    pub name: String,
    pub provider_type: String,
    pub org_id: i64,
    pub folder: String,
    pub folder_uid: String,
    pub editable: bool,
    pub disable_deletion: bool,
    pub update_interval_seconds: i64,
    pub allow_ui_updates: bool,
    pub path: String,
    pub folders_from_files_structure: bool,
}

/// Outcome of parsing a datasource provisioning YAML file.
#[derive(Debug, Clone)]
pub struct DatasourceProvisioningOutcome {
    pub datasources: Vec<DatasourceRecord>,
    pub delete_names: Vec<String>,
    pub findings: Vec<FeatureFinding>,
}

/// Computes Grafana's deterministic fallback UID when a provisioned datasource omits `uid`
/// (`GF pkg/services/provisioning/datasources/types.go:safeUIDFromName`).
#[must_use]
pub fn safe_uid_from_name(name: &str) -> String {
    let hex_full = sha256_hex(name.as_bytes());
    format!("P{}", &hex_full[..16]).to_ascii_uppercase()
}

/// Validates that `candidate_path` contains no `..` traversal components and resolves inside
/// `allowed_root` when `allowed_root` is provided (CVE-2021-43798 / R12).
pub fn validate_provisioning_path(
    candidate_path: &str,
    allowed_root: Option<&Path>,
) -> CrResult<PathBuf> {
    if candidate_path.trim().is_empty() || candidate_path.contains('\0') {
        return Err(CrError::BadRequest(
            "provisioning path must not be empty or contain null bytes".into(),
        ));
    }
    let path = Path::new(candidate_path);
    for comp in path.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(CrError::Forbidden(format!(
                "path traversal ('..') refused in provisioning path '{candidate_path}'"
            )));
        }
    }

    if let Some(root) = allowed_root {
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        if let (Ok(canon_root), Ok(canon_target)) = (root.canonicalize(), joined.canonicalize()) {
            if !canon_target.starts_with(&canon_root) {
                return Err(CrError::Forbidden(format!(
                    "provisioning path '{candidate_path}' escapes allowed root directory"
                )));
            }
            return Ok(canon_target);
        } else if path.is_absolute() && !joined.starts_with(root) {
            return Err(CrError::Forbidden(format!(
                "provisioning path '{candidate_path}' is outside allowed root directory"
            )));
        }
        return Ok(joined);
    }

    Ok(path.to_path_buf())
}

/// Parses a `dashboardproviders.yaml` document (v0 or v1) and validates its paths (`config_reader.go`).
pub fn parse_dashboard_providers_yaml(
    yaml_text: &str,
    allowed_root: Option<&Path>,
) -> CrResult<Vec<DashboardProviderConfig>> {
    let doc = parse_yaml_to_json(yaml_text)?;
    let providers_arr = if let Some(arr) = doc.as_array() {
        arr.clone()
    } else if let Some(arr) = doc.get("providers").and_then(Value::as_array) {
        arr.clone()
    } else {
        return Err(CrError::BadRequest(
            "dashboard provisioning YAML must contain a 'providers' list".into(),
        ));
    };

    let mut seen_names = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(providers_arr.len());

    for p in providers_arr {
        let Some(obj) = p.as_object() else {
            continue;
        };
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if name.is_empty() {
            return Err(CrError::BadRequest(
                "dashboard provider is missing 'name'".into(),
            ));
        }
        if !seen_names.insert(name.clone()) {
            return Err(CrError::BadRequest(format!(
                "dashboard name \"{name}\" is not unique"
            )));
        }

        let org_id = obj
            .get("orgId")
            .or_else(|| obj.get("org_id"))
            .and_then(Value::as_i64)
            .unwrap_or(1);
        let provider_type = obj
            .get("type")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("file")
            .to_owned();
        if provider_type != "file" {
            return Err(CrError::BadRequest(format!(
                "type {provider_type} is not supported"
            )));
        }

        let folder = obj
            .get("folder")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let folder_uid = obj
            .get("folderUid")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let editable = obj
            .get("editable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let disable_deletion = obj
            .get("disableDeletion")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let allow_ui_updates = obj
            .get("allowUiUpdates")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let update_interval_seconds = obj
            .get("updateIntervalSeconds")
            .and_then(Value::as_i64)
            .unwrap_or(10);

        let options = obj.get("options").and_then(Value::as_object);
        let raw_path = options
            .and_then(|o| o.get("path").or_else(|| o.get("folder")))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CrError::BadRequest("failed to load dashboards, path param is not a string".into())
            })?;

        validate_provisioning_path(raw_path, allowed_root)?;

        let folders_from_files_structure = options
            .and_then(|o| o.get("foldersFromFilesStructure"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if folders_from_files_structure && !folder.is_empty() && !folder_uid.is_empty() {
            return Err(CrError::BadRequest(
                "'folder' and 'folderUID' should be empty using 'foldersFromFilesStructure' option"
                    .into(),
            ));
        }

        out.push(DashboardProviderConfig {
            name,
            provider_type,
            org_id,
            folder,
            folder_uid,
            editable,
            disable_deletion,
            update_interval_seconds,
            allow_ui_updates,
            path: raw_path.to_owned(),
            folders_from_files_structure,
        });
    }

    Ok(out)
}

/// Parses a `datasources.yaml` document (v0 or v1) into [`DatasourceRecord`]s and §7.1 findings.
/// Refuses raw secrets in `secureJsonData` (`provisioning.secureJsonData`, §7.1).
pub fn parse_datasources_yaml(
    yaml_text: &str,
    installation_id: InstallationId,
    org_id: OrgId,
    instance_id: InstanceId,
) -> CrResult<DatasourceProvisioningOutcome> {
    let doc = parse_yaml_to_json(yaml_text)?;
    let obj = doc.as_object().ok_or_else(|| {
        CrError::BadRequest("datasource provisioning YAML must be a mapping".into())
    })?;

    let ds_arr = obj
        .get("datasources")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let del_arr = obj
        .get("deleteDatasources")
        .or_else(|| obj.get("delete_datasources"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut default_count = 0usize;
    let mut datasources = Vec::with_capacity(ds_arr.len());
    let mut findings = Vec::new();
    let now = Utc::now();

    for (idx, item) in ds_arr.iter().enumerate() {
        let Some(m) = item.as_object() else {
            continue;
        };
        let ptr = format!("/datasources/{idx}");
        let name = m
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if name.is_empty() {
            return Err(CrError::BadRequest(format!(
                "datasource at {ptr} is missing 'name'"
            )));
        }
        let kind = m
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("prometheus")
            .to_owned();
        let uid = m
            .get("uid")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map_or_else(|| safe_uid_from_name(&name), str::to_owned);
        let access_raw = m.get("access").and_then(Value::as_str).unwrap_or("proxy");
        let access = if access_raw == "direct" || access_raw == "proxy" {
            access_raw.to_owned()
        } else {
            "proxy".to_owned()
        };
        let url = m
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let is_default = m
            .get("isDefault")
            .or_else(|| m.get("is_default"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if is_default {
            default_count += 1;
            if default_count > 1 {
                return Err(CrError::BadRequest(
                    "datasource.yaml config is invalid. Only one datasource per organization can be marked as default"
                        .into(),
                ));
            }
        }
        let editable = m.get("editable").and_then(Value::as_bool).unwrap_or(false);
        let json_data = m
            .get("jsonData")
            .or_else(|| m.get("json_data"))
            .cloned()
            .unwrap_or_else(|| json!({}));

        // Check secureJsonData: raw secrets are refused per §7.1; only `secretRefs` are accepted.
        if let Some(sec) = m
            .get("secureJsonData")
            .or_else(|| m.get("secure_json_data"))
            .and_then(Value::as_object)
            && !sec.is_empty()
        {
            findings.push(FeatureFinding {
                pointer: format!("{ptr}/secureJsonData"),
                code: "provisioning.secureJsonData".into(),
                verdict: Verdict::Refuses,
                message: format!(
                    "datasource '{name}' carries raw secureJsonData; name a secretRef instead"
                ),
            });
        }

        let secret_refs: BTreeMap<String, String> = m
            .get("secretRefs")
            .and_then(Value::as_object)
            .map(|sr| {
                sr.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                    .collect()
            })
            .unwrap_or_default();

        let mut allow_hosts = Vec::new();
        if let Ok(parsed_url) = url::Url::parse(&url)
            && let Some(h) = parsed_url.host_str()
        {
            allow_hosts.push(h.to_owned());
        }

        findings.push(FeatureFinding {
            pointer: ptr,
            code: "provisioning.datasource".into(),
            verdict: Verdict::Maps,
            message: format!("provisioned datasource '{name}' ({uid}) mapped"),
        });

        datasources.push(DatasourceRecord {
            installation_id,
            org_id,
            instance_id,
            project_id: None,
            uid,
            name,
            kind,
            access,
            url,
            is_default,
            read_only: !editable,
            json_data,
            secret_refs,
            allow_hosts,
            wasm_digest: None,
            version: 1,
            created_at: now,
            updated_at: now,
        });
    }

    let delete_names = del_arr
        .iter()
        .filter_map(|d| d.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect();

    Ok(DatasourceProvisioningOutcome {
        datasources,
        delete_names,
        findings,
    })
}
