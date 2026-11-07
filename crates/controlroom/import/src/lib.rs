#![forbid(unsafe_code)]
//! `controlroom-import`: Import, export, offline `check`, `plan`, and `apply` with stale-plan
//! refusal, exit codes `0/1/2/3/4`, provisioning YAML support, and JSON / Markdown reports (R2, §7).

pub mod provisioning;
pub mod yaml;

pub use provisioning::{
    DashboardProviderConfig, DatasourceProvisioningOutcome, parse_dashboard_providers_yaml,
    parse_datasources_yaml, safe_uid_from_name, validate_provisioning_path,
};

use chrono::Utc;
use controlroom_model::{
    CanonicalJson, ControlroomStore, CrError, CrResult, DataSourceInfo, DashboardDocument,
    DashboardRecord, DatasourceIndex, DatasourceRecord, FeatureFinding, FolderRecord,
    ImportReportRecord, InputLimits, InstallationId, InstanceId, LibraryPanelRecord,
    MigrationReport, OrgId, Verdict, classify_and_validate, migrate_to_latest, sha256_hex,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// CLI / CI exit codes for `import`, `check`, `plan`, and `apply` (R2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum ExitCode {
    /// `0`: Clean success, no refusals, or plan has no changes.
    Ok = 0,
    /// `1`: Validation error or unallowed feature refusal.
    Refused = 1,
    /// `2`: Plan has changes or import completed with warnings/mappings.
    ChangesOrWarnings = 2,
    /// `3`: Stale plan conflict (live state changed since `plan` was computed).
    StalePlan = 3,
    /// `4`: Store, I/O, or quota error.
    StoreOrIoError = 4,
}

impl ExitCode {
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// Per-dashboard report produced by `check` or `import` (R2, §7.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardImportReport {
    pub uid: String,
    pub title: String,
    pub folder_uid: Option<String>,
    pub migration: MigrationReport,
    pub panel_count: usize,
    pub findings: Vec<FeatureFinding>,
    pub missing_datasources: Vec<String>,
}

/// Complete report for an `import`, `check`, or `apply` run (R2, §7.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub dry_run: bool,
    pub exit_code: i32,
    pub dashboards: Vec<DashboardImportReport>,
    pub provisioning_findings: Vec<FeatureFinding>,
    pub total_panels: usize,
    pub refused_count: usize,
    pub mapped_count: usize,
    pub supported_count: usize,
}

impl ImportReport {
    /// Renders the report as pretty-printed JSON (R2).
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// Renders the report as a Markdown summary table and finding list (R2).
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str(&format!(
            "# controlroom Import Report (dry_run={}, exit_code={})\n\n",
            self.dry_run, self.exit_code
        ));
        md.push_str("| UID | Title | Schema Migration | Panels | Refusals | Mappings |\n");
        md.push_str("|---|---|---|---|---|---|\n");
        for d in &self.dashboards {
            let ref_cnt = d
                .findings
                .iter()
                .filter(|f| f.verdict == Verdict::Refuses)
                .count();
            let map_cnt = d
                .findings
                .iter()
                .filter(|f| f.verdict == Verdict::Maps)
                .count();
            md.push_str(&format!(
                "| `{}` | {} | {} -> {} | {} | {} | {} |\n",
                d.uid,
                d.title,
                d.migration.original_schema_version,
                d.migration.final_schema_version,
                d.panel_count,
                ref_cnt,
                map_cnt
            ));
        }
        if !self.provisioning_findings.is_empty()
            || self.dashboards.iter().any(|d| {
                d.findings
                    .iter()
                    .any(|f| f.verdict != Verdict::Supports)
            })
        {
            md.push_str("\n## Findings\n\n");
            for pf in &self.provisioning_findings {
                md.push_str(&format!(
                    "- **[{:?}]** `{}` (`{}`): {}\n",
                    pf.verdict, pf.code, pf.pointer, pf.message
                ));
            }
            for d in &self.dashboards {
                for f in &d.findings {
                    if f.verdict != Verdict::Supports {
                        md.push_str(&format!(
                            "- `{}` **[{:?}]** `{}` (`{}`): {}\n",
                            d.uid, f.verdict, f.code, f.pointer, f.message
                        ));
                    }
                }
            }
        }
        md
    }
}

/// Input item for `import_dashboards` or `check_dashboards`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardImportInput {
    pub raw_json: Value,
    #[serde(default)]
    pub folder_uid: Option<String>,
    #[serde(default)]
    pub is_provisioned: bool,
    #[serde(default)]
    pub allow_ui_updates: bool,
    #[serde(default)]
    pub external_id: Option<String>,
}

/// Inlines any `libraryPanel` reference on panels using `library_panels` in the organization (§1.3, §7.1).
pub fn inline_library_panels(
    dash_val: &mut Value,
    library_panels: &BTreeMap<String, LibraryPanelRecord>,
) {
    let Some(panels) = dash_val.get_mut("panels").and_then(Value::as_array_mut) else {
        return;
    };
    for p in panels {
        let Some(p_obj) = p.as_object_mut() else {
            continue;
        };
        let lib_uid = p_obj
            .get("libraryPanel")
            .and_then(|lp| lp.get("uid"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(uid) = lib_uid {
            if let Some(lib_rec) = library_panels.get(&uid) {
                if let Some(model_obj) = lib_rec.model.as_object() {
                    for (k, v) in model_obj {
                        if k != "id" && k != "gridPos" && k != "libraryPanel" {
                            p_obj.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                    }
                }
            }
        }
    }
}

/// Prepares, migrates to `schemaVersion: 42`, and validates a single dashboard JSON document (R1, R2).
pub fn prepare_dashboard(
    raw_input: &Value,
    ds_index: &DatasourceIndex,
    known_ds_uids: &BTreeSet<String>,
    library_panels: &BTreeMap<String, LibraryPanelRecord>,
    limits: &InputLimits,
) -> CrResult<(DashboardDocument, Value, DashboardImportReport)> {
    // Support both raw dashboard JSON and Grafana API wrapper `{"dashboard": {...}, "folderUid": ...}`
    let mut dash_val = raw_input
        .get("dashboard")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| raw_input.clone());

    let raw_bytes_len = serde_json::to_vec(&dash_val)
        .map_err(|e| CrError::BadRequest(format!("invalid dashboard JSON: {e}")))?
        .len();

    inline_library_panels(&mut dash_val, library_panels);

    let migration = migrate_to_latest(&mut dash_val, ds_index)?;
    let canonical_val = CanonicalJson::export_dashboard_json(&dash_val);

    let doc: DashboardDocument = serde_json::from_value(canonical_val.clone()).map_err(|e| {
        CrError::Unprocessable {
            detail: format!("dashboard failed schema validation: {e}"),
            errors: vec![e.to_string()],
        }
    })?;

    let val_report = classify_and_validate(raw_bytes_len, &doc, known_ds_uids, limits)?;
    let idx = doc.extract_index();

    let mut findings = val_report.findings;
    for w in &migration.warnings {
        findings.push(FeatureFinding {
            pointer: "/schemaVersion".to_owned(),
            code: "schema.migrationWarning".to_owned(),
            verdict: Verdict::Maps,
            message: w.clone(),
        });
    }

    let report = DashboardImportReport {
        uid: idx.uid,
        title: idx.title,
        folder_uid: raw_input
            .get("folderUid")
            .and_then(Value::as_str)
            .map(str::to_owned),
        migration,
        panel_count: idx.panel_count,
        findings,
        missing_datasources: val_report.missing_datasources,
    };

    Ok((doc, canonical_val, report))
}

/// Offline `check` (no database or network needed, R2, §7.3): validates, migrates, and classifies
/// a slice of dashboards against an optional list of known datasource UIDs and an allow-list of
/// refusal codes.
pub fn check_dashboards_offline(
    inputs: &[DashboardImportInput],
    known_datasources: &[DataSourceInfo],
    allowed_refusal_codes: &BTreeSet<String>,
    limits: &InputLimits,
) -> CrResult<ImportReport> {
    let ds_index = DatasourceIndex::new(known_datasources.to_vec());
    let known_uids: BTreeSet<String> = known_datasources.iter().map(|d| d.uid.clone()).collect();
    let empty_libs = BTreeMap::new();

    let mut dash_reports = Vec::with_capacity(inputs.len());
    let mut total_panels = 0usize;
    let mut refused_count = 0usize;
    let mut mapped_count = 0usize;
    let mut supported_count = 0usize;

    for item in inputs {
        let (_, _, mut rep) = prepare_dashboard(
            &item.raw_json,
            &ds_index,
            &known_uids,
            &empty_libs,
            limits,
        )?;
        if rep.folder_uid.is_none() {
            rep.folder_uid.clone_from(&item.folder_uid);
        }
        total_panels += rep.panel_count;
        for f in &rep.findings {
            match f.verdict {
                Verdict::Supports => supported_count += 1,
                Verdict::Maps => mapped_count += 1,
                Verdict::Refuses => {
                    if !allowed_refusal_codes.contains(&f.code) {
                        refused_count += 1;
                    }
                }
            }
        }
        dash_reports.push(rep);
    }

    let exit_code = if refused_count > 0 {
        ExitCode::Refused.code()
    } else if mapped_count > 0 {
        ExitCode::ChangesOrWarnings.code()
    } else {
        ExitCode::Ok.code()
    };

    Ok(ImportReport {
        dry_run: true,
        exit_code,
        dashboards: dash_reports,
        provisioning_findings: Vec::new(),
        total_panels,
        refused_count,
        mapped_count,
        supported_count,
    })
}

/// Imports one or more dashboards into `store` (or performs a dry run when `dry_run == true`) (R2, §7.2).
pub async fn import_dashboards(
    store: &dyn ControlroomStore,
    installation_id: InstallationId,
    org_id: OrgId,
    instance_id: InstanceId,
    actor: &str,
    inputs: &[DashboardImportInput],
    dry_run: bool,
    limits: &InputLimits,
) -> CrResult<ImportReport> {
    let live_ds = store.list_datasources(org_id).await?;
    let ds_infos: Vec<DataSourceInfo> = live_ds
        .iter()
        .map(|d| DataSourceInfo {
            uid: d.uid.clone(),
            name: d.name.clone(),
            ds_type: d.kind.clone(),
            api_version: String::new(),
            is_default: d.is_default,
        })
        .collect();
    let ds_index = DatasourceIndex::new(ds_infos);
    let known_uids: BTreeSet<String> = live_ds.iter().map(|d| d.uid.clone()).collect();

    let lib_list = store.list_library_panels(org_id).await?;
    let lib_map: BTreeMap<String, LibraryPanelRecord> =
        lib_list.into_iter().map(|l| (l.uid.clone(), l)).collect();

    let mut dash_reports = Vec::with_capacity(inputs.len());
    let mut total_panels = 0usize;
    let mut refused_count = 0usize;
    let mut mapped_count = 0usize;
    let mut supported_count = 0usize;
    let now = Utc::now();

    for item in inputs {
        let (doc, canonical_val, mut rep) =
            prepare_dashboard(&item.raw_json, &ds_index, &known_uids, &lib_map, limits)?;
        let folder_uid = item.folder_uid.clone().or_else(|| rep.folder_uid.clone());
        rep.folder_uid.clone_from(&folder_uid);
        total_panels += rep.panel_count;

        for f in &rep.findings {
            match f.verdict {
                Verdict::Supports => supported_count += 1,
                Verdict::Maps => mapped_count += 1,
                Verdict::Refuses => refused_count += 1,
            }
        }

        if !dry_run {
            let idx = doc.extract_index();
            let uid = if idx.uid.is_empty() {
                uuid::Uuid::now_v7().to_string()
            } else {
                idx.uid
            };
            let record = DashboardRecord {
                installation_id,
                org_id,
                instance_id,
                project_id: None,
                uid,
                folder_uid,
                title: idx.title,
                slug: idx.slug,
                tags: idx.tags,
                panel_count: idx.panel_count as i32,
                datasource_uids: idx.datasource_uids,
                version: doc.version.unwrap_or(1) as i32,
                schema_version: 42,
                is_provisioned: item.is_provisioned,
                provisioned_external_id: item.external_id.clone(),
                allow_ui_updates: item.allow_ui_updates || !item.is_provisioned,
                data: canonical_val,
                created_by: actor.to_owned(),
                updated_by: actor.to_owned(),
                created_at: now,
                updated_at: now,
            };
            store.upsert_dashboard(record, "imported", true).await?;
        }

        dash_reports.push(rep);
    }

    let exit_code = if refused_count > 0 {
        ExitCode::Refused.code()
    } else if mapped_count > 0 {
        ExitCode::ChangesOrWarnings.code()
    } else {
        ExitCode::Ok.code()
    };

    let report = ImportReport {
        dry_run,
        exit_code,
        dashboards: dash_reports,
        provisioning_findings: Vec::new(),
        total_panels,
        refused_count,
        mapped_count,
        supported_count,
    };

    let audit = ImportReportRecord {
        id: uuid::Uuid::now_v7().to_string(),
        org_id,
        instance_id,
        dry_run,
        status: if refused_count == 0 {
            "success".into()
        } else {
            "completed_with_refusals".into()
        },
        dashboard_count: report.dashboards.len() as i32,
        refused_count: refused_count as i32,
        mapped_count: mapped_count as i32,
        summary: report.to_json(),
        created_at: now,
    };
    store.insert_import_report(audit).await?;

    Ok(report)
}

/// Exports a stored dashboard as byte-stable canonical Grafana classic JSON at `schemaVersion: 42`,
/// with `id` removed (R2, §7.2).
pub async fn export_dashboard(
    store: &dyn ControlroomStore,
    org_id: OrgId,
    uid: &str,
) -> CrResult<Value> {
    let record = store
        .get_dashboard(org_id, uid)
        .await?
        .ok_or_else(|| CrError::NotFound(format!("Dashboard '{uid}' not found")))?;
    let mut exported = CanonicalJson::export_dashboard_json(&record.data);
    // Strip internal numeric version if original didn't set it or keep canonical schemaVersion 42
    if let Some(obj) = exported.as_object_mut() {
        obj.remove("id");
    }
    Ok(CanonicalJson::canonicalize(&exported))
}

/// Diff entry in a [`PlanBundle`] (R2, §7.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDiffItem {
    pub kind: String,
    pub uid: String,
    pub action: String, // "create", "update", "unchanged"
}

/// Desired state bundle for `plan` and `apply` (R2, §7.3).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesiredBundle {
    #[serde(default)]
    pub folders: Vec<FolderRecord>,
    #[serde(default)]
    pub datasources: Vec<DatasourceRecord>,
    #[serde(default)]
    pub dashboards: Vec<DashboardImportInput>,
}

/// Verified plan bundle carrying `base_etag` for stale-plan refusal on `apply` (R2, §7.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanBundle {
    pub org_id: OrgId,
    pub base_etag: String,
    pub has_changes: bool,
    pub diffs: Vec<PlanDiffItem>,
    pub dry_run_report: ImportReport,
    pub desired: Value,
}

/// Computes a deterministic SHA-256 digest of the organization's live folders, datasources, and dashboards.
pub async fn compute_org_state_etag(store: &dyn ControlroomStore, org_id: OrgId) -> CrResult<String> {
    let folders = store.list_folders(org_id).await?;
    let datasources = store.list_datasources(org_id).await?;
    let dashboards = store.list_dashboards(org_id).await?;

    let summary = json!({
        "folders": folders.iter().map(|f| (&f.uid, f.version, &f.title)).collect::<Vec<_>>(),
        "datasources": datasources.iter().map(|d| (&d.uid, d.version, &d.url)).collect::<Vec<_>>(),
        "dashboards": dashboards.iter().map(|d| (&d.uid, d.version, &d.data)).collect::<Vec<_>>(),
    });
    let canonical = CanonicalJson::canonicalize(&summary);
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    Ok(sha256_hex(&bytes))
}

/// Computes a [`PlanBundle`] diffing `desired` against the live state of `org_id` (R2, §7.3).
pub async fn compute_plan(
    store: &dyn ControlroomStore,
    installation_id: InstallationId,
    org_id: OrgId,
    instance_id: InstanceId,
    actor: &str,
    desired: &DesiredBundle,
    limits: &InputLimits,
) -> CrResult<PlanBundle> {
    let base_etag = compute_org_state_etag(store, org_id).await?;
    let live_folders: BTreeMap<String, FolderRecord> = store
        .list_folders(org_id)
        .await?
        .into_iter()
        .map(|f| (f.uid.clone(), f))
        .collect();
    let live_ds: BTreeMap<String, DatasourceRecord> = store
        .list_datasources(org_id)
        .await?
        .into_iter()
        .map(|d| (d.uid.clone(), d))
        .collect();
    let live_dash: BTreeMap<String, DashboardRecord> = store
        .list_dashboards(org_id)
        .await?
        .into_iter()
        .map(|d| (d.uid.clone(), d))
        .collect();

    let mut diffs = Vec::new();
    for f in &desired.folders {
        let action = match live_folders.get(&f.uid) {
            None => "create",
            Some(ex) if ex.title != f.title || ex.parent_uid != f.parent_uid => "update",
            _ => "unchanged",
        };
        diffs.push(PlanDiffItem {
            kind: "Folder".into(),
            uid: f.uid.clone(),
            action: action.into(),
        });
    }

    for ds in &desired.datasources {
        let action = match live_ds.get(&ds.uid) {
            None => "create",
            Some(ex) if ex.url != ds.url || ex.name != ds.name || ex.json_data != ds.json_data => {
                "update"
            }
            _ => "unchanged",
        };
        diffs.push(PlanDiffItem {
            kind: "Datasource".into(),
            uid: ds.uid.clone(),
            action: action.into(),
        });
    }

    let dry_run_report = import_dashboards(
        store,
        installation_id,
        org_id,
        instance_id,
        actor,
        &desired.dashboards,
        true,
        limits,
    )
    .await?;

    for d_rep in &dry_run_report.dashboards {
        let action = if live_dash.contains_key(&d_rep.uid) {
            "update"
        } else {
            "create"
        };
        diffs.push(PlanDiffItem {
            kind: "Dashboard".into(),
            uid: d_rep.uid.clone(),
            action: action.into(),
        });
    }

    let has_changes = diffs.iter().any(|d| d.action != "unchanged");
    let desired_val = serde_json::to_value(desired).unwrap_or(Value::Null);

    Ok(PlanBundle {
        org_id,
        base_etag,
        has_changes,
        diffs,
        dry_run_report,
        desired: desired_val,
    })
}

/// Applies a [`PlanBundle`] to `store`, refusing stale plans if the live state etag has changed
/// since `compute_plan` (R2, §7.3: exit code 3 / `CrError::StalePlan`).
pub async fn apply_plan(
    store: &dyn ControlroomStore,
    installation_id: InstallationId,
    org_id: OrgId,
    instance_id: InstanceId,
    actor: &str,
    plan: &PlanBundle,
    limits: &InputLimits,
) -> CrResult<ImportReport> {
    if plan.org_id != org_id {
        return Err(CrError::NotFound("plan organization mismatch".into()));
    }
    let current_etag = compute_org_state_etag(store, org_id).await?;
    if current_etag != plan.base_etag {
        return Err(CrError::StalePlan(format!(
            "organization state changed since plan was created (expected etag {}, found {})",
            plan.base_etag, current_etag
        )));
    }

    let desired: DesiredBundle = serde_json::from_value(plan.desired.clone())
        .map_err(|e| CrError::BadRequest(format!("invalid desired state in plan: {e}")))?;

    for mut folder in desired.folders {
        folder.installation_id = installation_id;
        folder.org_id = org_id;
        folder.instance_id = instance_id;
        store.upsert_folder(folder).await?;
    }

    for mut ds in desired.datasources {
        ds.installation_id = installation_id;
        ds.org_id = org_id;
        ds.instance_id = instance_id;
        store.upsert_datasource(ds).await?;
    }

    import_dashboards(
        store,
        installation_id,
        org_id,
        instance_id,
        actor,
        &desired.dashboards,
        false,
        limits,
    )
    .await
}
