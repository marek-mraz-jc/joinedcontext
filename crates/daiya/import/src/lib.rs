#![forbid(unsafe_code)]
//! `daiya-import`: Import, export, offline `check`, `plan`, and `apply` with stale-plan
//! refusal, exit codes `0/1/2/3/4`, provisioning YAML support, and JSON / Markdown reports (R2, §7).

pub mod loader;
pub mod provisioning;
pub mod yaml;

pub use loader::{
    ProvisioningLoadReport, ProvisioningTarget, expand_variables, load_provisioning_dir,
};

pub use provisioning::{
    DashboardProviderConfig, DatasourceProvisioningOutcome, parse_dashboard_providers_yaml,
    parse_datasources_yaml, safe_uid_from_name, validate_provisioning_path,
};

use daiya_model::{
    ApplyBundle, CanonicalJson, CrError, CrResult, DaiyaStore, DashboardDocument, DashboardRecord,
    DataSourceInfo, DatasourceIndex, DatasourceRecord, FeatureFinding, FolderRecord,
    ImportReportRecord, InputLimits, InstallationId, InstanceId, LibraryPanelRecord,
    MigrationReport, OrgId, PlanRecord, Verdict, classify_and_validate, migrate_to_latest,
    org_state_etag, sha256_hex,
};
use jc_time::TimestampTz;
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
            "# daiya Import Report (dry_run={}, exit_code={})\n\n",
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
            || self
                .dashboards
                .iter()
                .any(|d| d.findings.iter().any(|f| f.verdict != Verdict::Supports))
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

/// Inlines any `libraryPanel` reference on panels using `library_panels` in the organization
/// (§1.3, §7.1), including panels inside collapsed rows (`panels[].panels`), where Grafana keeps
/// a collapsed row's children.
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
        if let Some(uid) = lib_uid
            && let Some(lib_rec) = library_panels.get(&uid)
            && let Some(model_obj) = lib_rec.model.as_object()
        {
            for (k, v) in model_obj {
                if k != "id" && k != "gridPos" && k != "libraryPanel" {
                    p_obj.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
        if p_obj.get("type").and_then(Value::as_str) == Some("row") {
            inline_library_panels(p, library_panels);
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

    let doc: DashboardDocument =
        serde_json::from_value(canonical_val.clone()).map_err(|e| CrError::Unprocessable {
            detail: format!("dashboard failed schema validation: {e}"),
            errors: vec![e.to_string()],
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
        let (_, _, mut rep) =
            prepare_dashboard(&item.raw_json, &ds_index, &known_uids, &empty_libs, limits)?;
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
#[allow(clippy::too_many_arguments)]
pub async fn import_dashboards(
    store: &dyn DaiyaStore,
    installation_id: InstallationId,
    org_id: OrgId,
    instance_id: InstanceId,
    actor: &str,
    inputs: &[DashboardImportInput],
    dry_run: bool,
    limits: &InputLimits,
) -> CrResult<ImportReport> {
    let live_ds = store.list_datasources(org_id).await?;
    let lib_map = library_panels(store, org_id).await?;
    let ids = (installation_id, org_id, instance_id);
    let (mut report, records) =
        prepare_import(&live_ds, &lib_map, ids, actor, inputs, dry_run, limits)?;
    for (rep, record) in report.dashboards.iter_mut().zip(records) {
        rep.uid = store.upsert_dashboard(record, "imported", true).await?.uid;
    }
    store
        .insert_import_report(audit_record(org_id, instance_id, &report))
        .await?;
    Ok(report)
}

async fn library_panels(
    store: &dyn DaiyaStore,
    org_id: OrgId,
) -> CrResult<BTreeMap<String, LibraryPanelRecord>> {
    Ok(store
        .list_library_panels(org_id)
        .await?
        .into_iter()
        .map(|l| (l.uid.clone(), l))
        .collect())
}

/// Validates and migrates `inputs` against `live_ds`; answers the report and, unless
/// `dry_run`, the dashboard records an import writes (one per report entry, same order).
fn prepare_import(
    live_ds: &[DatasourceRecord],
    lib_map: &BTreeMap<String, LibraryPanelRecord>,
    (installation_id, org_id, instance_id): (InstallationId, OrgId, InstanceId),
    actor: &str,
    inputs: &[DashboardImportInput],
    dry_run: bool,
    limits: &InputLimits,
) -> CrResult<(ImportReport, Vec<DashboardRecord>)> {
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

    let mut dash_reports = Vec::with_capacity(inputs.len());
    let mut records = Vec::new();
    let mut total_panels = 0usize;
    let mut refused_count = 0usize;
    let mut mapped_count = 0usize;
    let mut supported_count = 0usize;
    let now = TimestampTz::now();

    for item in inputs {
        let (doc, canonical_val, mut rep) =
            prepare_dashboard(&item.raw_json, &ds_index, &known_uids, lib_map, limits)?;
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
            rep.uid.clone_from(&uid);
            records.push(DashboardRecord {
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
            });
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
    Ok((report, records))
}

fn audit_record(
    org_id: OrgId,
    instance_id: InstanceId,
    report: &ImportReport,
) -> ImportReportRecord {
    ImportReportRecord {
        id: uuid::Uuid::now_v7().to_string(),
        org_id,
        instance_id,
        dry_run: report.dry_run,
        status: if report.refused_count == 0 {
            "success".into()
        } else {
            "completed_with_refusals".into()
        },
        dashboard_count: report.dashboards.len() as i32,
        refused_count: report.refused_count as i32,
        mapped_count: report.mapped_count as i32,
        summary: report.to_json(),
        created_at: TimestampTz::now(),
    }
}

/// Exports a stored dashboard as byte-stable canonical Grafana classic JSON at `schemaVersion: 42`,
/// with `id` removed (R2, §7.2).
pub async fn export_dashboard(store: &dyn DaiyaStore, org_id: OrgId, uid: &str) -> CrResult<Value> {
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
    /// What `Apply` takes: the server keeps this plan's bundle under its digest until
    /// `expires_at` (SHA-256 hex of the canonical `{ baseEtag, desired }`).
    pub digest: String,
    pub expires_at: TimestampTz,
    pub base_etag: String,
    pub has_changes: bool,
    pub diffs: Vec<PlanDiffItem>,
    pub dry_run_report: ImportReport,
    pub desired: Value,
}

/// Computes a deterministic SHA-256 digest of the organization's live folders, datasources, and dashboards.
pub async fn compute_org_state_etag(store: &dyn DaiyaStore, org_id: OrgId) -> CrResult<String> {
    Ok(org_state_etag(
        &store.list_folders(org_id).await?,
        &store.list_datasources(org_id).await?,
        &store.list_dashboards(org_id).await?,
    ))
}

/// How long the server keeps a plan for `Apply` (R2, §7.3).
pub const PLAN_TTL_MILLIS: i64 = 60 * 60 * 1000;

/// SHA-256 hex of the canonical `{ baseEtag, desired }`: the name `Apply` takes for a plan.
#[must_use]
pub fn plan_digest(base_etag: &str, desired: &Value) -> String {
    let canonical =
        CanonicalJson::canonicalize(&json!({ "baseEtag": base_etag, "desired": desired }));
    sha256_hex(
        serde_json::to_string(&canonical)
            .unwrap_or_default()
            .as_bytes(),
    )
}

/// Computes a [`PlanBundle`] diffing `desired` against the live state of `org_id` (R2, §7.3).
pub async fn compute_plan(
    store: &dyn DaiyaStore,
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
            Some(ex)
                if ex.url != ds.url
                    || ex.name != ds.name
                    || ex.kind != ds.kind
                    || ex.access != ds.access
                    || ex.is_default != ds.is_default
                    || ex.read_only != ds.read_only
                    || ex.json_data != ds.json_data
                    || ex.secret_refs != ds.secret_refs
                    || ex.allow_hosts != ds.allow_hosts
                    || ex.wasm_digest != ds.wasm_digest =>
            {
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

    let ds_index = DatasourceIndex::new(
        live_ds
            .values()
            .map(|d| DataSourceInfo {
                uid: d.uid.clone(),
                name: d.name.clone(),
                ds_type: d.kind.clone(),
                api_version: String::new(),
                is_default: d.is_default,
            })
            .collect(),
    );
    let known_uids: BTreeSet<String> = live_ds.keys().cloned().collect();
    let lib_map: BTreeMap<String, LibraryPanelRecord> = store
        .list_library_panels(org_id)
        .await?
        .into_iter()
        .map(|l| (l.uid.clone(), l))
        .collect();
    for (d_rep, input) in dry_run_report.dashboards.iter().zip(&desired.dashboards) {
        // "unchanged" only when the stored document and folder are exactly what apply would
        // write; a plan of the live state then has no changes.
        let action = match live_dash.get(&d_rep.uid) {
            None => "create",
            Some(live) => {
                let (_, canonical, _) =
                    prepare_dashboard(&input.raw_json, &ds_index, &known_uids, &lib_map, limits)?;
                let folder = input
                    .folder_uid
                    .clone()
                    .or_else(|| d_rep.folder_uid.clone());
                if live.data == canonical && live.folder_uid == folder {
                    "unchanged"
                } else {
                    "update"
                }
            }
        };
        diffs.push(PlanDiffItem {
            kind: "Dashboard".into(),
            uid: d_rep.uid.clone(),
            action: action.into(),
        });
    }

    let has_changes = diffs.iter().any(|d| d.action != "unchanged");
    let desired_val = serde_json::to_value(desired)
        .map_err(|e| CrError::Internal(format!("plan bundle did not serialize: {e}")))?;
    let digest = plan_digest(&base_etag, &desired_val);
    let created_at = TimestampTz::now();
    let expires_at = TimestampTz::from_millis(created_at.as_millis() + PLAN_TTL_MILLIS)
        .ok_or_else(|| CrError::Internal("plan expiry out of range".into()))?;
    store
        .insert_plan(PlanRecord {
            installation_id,
            org_id,
            instance_id,
            digest: digest.clone(),
            base_etag: base_etag.clone(),
            desired: desired_val.clone(),
            created_by: actor.to_owned(),
            created_at,
            expires_at,
        })
        .await?;

    Ok(PlanBundle {
        org_id,
        digest,
        expires_at,
        base_etag,
        has_changes,
        diffs,
        dry_run_report,
        desired: desired_val,
    })
}

/// A plan the server holds, loaded by [`load_plan`]: the only input [`apply_plan`] takes, so
/// an Apply writes exactly a bundle `Plan` computed and kept, never one a caller supplies.
#[derive(Debug, Clone)]
pub struct LoadedPlan {
    record: PlanRecord,
    desired: DesiredBundle,
}

impl LoadedPlan {
    /// The bundle the plan writes, for the caller's own checks before [`apply_plan`].
    #[must_use]
    pub const fn desired(&self) -> &DesiredBundle {
        &self.desired
    }

    #[must_use]
    pub const fn record(&self) -> &PlanRecord {
        &self.record
    }
}

/// Loads the plan `digest` of `org_id`. An ill-formed digest is a 400; an unknown or expired
/// one, or one of another organization, is the same 404, so a caller learns nothing about plans
/// it cannot apply.
pub async fn load_plan(
    store: &dyn DaiyaStore,
    org_id: OrgId,
    digest: &str,
) -> CrResult<LoadedPlan> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(CrError::BadRequest(
            "digest: give the 64-character lowercase hex digest a Plan answered".into(),
        ));
    }
    let not_found = || {
        CrError::NotFound("plan not found or expired: run Plan again and apply its digest".into())
    };
    let record = store
        .get_plan(org_id, digest)
        .await?
        .filter(|p| p.org_id == org_id && p.expires_at > TimestampTz::now())
        .ok_or_else(not_found)?;
    if plan_digest(&record.base_etag, &record.desired) != record.digest {
        return Err(CrError::Internal(
            "stored plan does not match its digest".into(),
        ));
    }
    let desired = serde_json::from_value(record.desired.clone())
        .map_err(|e| CrError::Internal(format!("stored plan bundle did not parse: {e}")))?;
    Ok(LoadedPlan { record, desired })
}

/// Applies a plan the server holds in one store transaction ([`DaiyaStore::apply_bundle`]):
/// folders, datasources and dashboards are all written or none, and a plan whose base etag no
/// longer matches the organization's state is refused (R2, §7.3: exit code 3 /
/// `CrError::StalePlan`).
pub async fn apply_plan(
    store: &dyn DaiyaStore,
    installation_id: InstallationId,
    org_id: OrgId,
    instance_id: InstanceId,
    actor: &str,
    plan: &LoadedPlan,
    limits: &InputLimits,
) -> CrResult<ImportReport> {
    if plan.record.org_id != org_id {
        return Err(CrError::NotFound(
            "plan not found or expired: run Plan again and apply its digest".into(),
        ));
    }
    let desired = &plan.desired;
    let folders = desired
        .folders
        .iter()
        .cloned()
        .map(|mut f| {
            f.installation_id = installation_id;
            f.org_id = org_id;
            f.instance_id = instance_id;
            f
        })
        .collect();
    let datasources: Vec<DatasourceRecord> = desired
        .datasources
        .iter()
        .cloned()
        .map(|mut d| {
            d.installation_id = installation_id;
            d.org_id = org_id;
            d.instance_id = instance_id;
            d
        })
        .collect();
    // Dashboards resolve their datasources against the state after this bundle's datasources.
    let mut after: BTreeMap<String, DatasourceRecord> = store
        .list_datasources(org_id)
        .await?
        .into_iter()
        .map(|d| (d.uid.clone(), d))
        .collect();
    for d in &datasources {
        after.insert(d.uid.clone(), d.clone());
    }
    let mut after: Vec<DatasourceRecord> = after.into_values().collect();
    after.sort_by(|a, b| a.name.cmp(&b.name));
    let lib_map = library_panels(store, org_id).await?;
    let ids = (installation_id, org_id, instance_id);
    let (mut report, dashboards) = prepare_import(
        &after,
        &lib_map,
        ids,
        actor,
        &desired.dashboards,
        false,
        limits,
    )?;

    let saved = store
        .apply_bundle(ApplyBundle {
            installation_id,
            org_id,
            instance_id,
            base_etag: plan.record.base_etag.clone(),
            folders,
            datasources,
            dashboards,
            message: "imported".into(),
        })
        .await?;
    for (rep, record) in report.dashboards.iter_mut().zip(saved) {
        rep.uid = record.uid;
    }
    store
        .insert_import_report(audit_record(org_id, instance_id, &report))
        .await?;
    Ok(report)
}
