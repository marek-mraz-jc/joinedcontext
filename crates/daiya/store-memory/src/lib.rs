#![forbid(unsafe_code)]
//! `daiya-store-memory`: In-memory implementation of [`DaiyaStore`] with strict
//! per-organization (`OrgId`) row isolation, quota enforcement, version history, and boot RLS
//! verification (R8, §5.3).

use daiya_model::{
    ApplyBundle, AuditRecord, CrError, CrResult, DaiyaStore, DashboardRecord, DashboardVersionRecord,
    DatasourceRecord, FolderRecord, ImportReportRecord, InstallationId, InstanceId,
    LibraryPanelRecord, MAX_LIVE_PLANS, OrgId, OrgKey, OrgQuotas, OrgRow, PlanRecord,
    PublicDashboardRecord, RowPage, RowQuery, RowSet, constant_time_eq, org_state_etag,
    store::StoreFuture,
};
use jc_time::TimestampTz;
use parking_lot::RwLock;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default, Clone)]
struct OrgState {
    quotas: OrgQuotas,
    folders: BTreeMap<String, FolderRecord>,
    dashboards: BTreeMap<String, DashboardRecord>,
    versions: BTreeMap<String, Vec<DashboardVersionRecord>>,
    datasources: BTreeMap<String, DatasourceRecord>,
    library_panels: BTreeMap<String, LibraryPanelRecord>,
    public_dashboards: BTreeMap<String, PublicDashboardRecord>,
    import_reports: Vec<ImportReportRecord>,
    plans: BTreeMap<String, PlanRecord>,
    audit: Vec<AuditRecord>,
}

#[derive(Debug, Default)]
struct InnerStore {
    directory: BTreeMap<InstallationId, Vec<OrgRow>>,
    orgs: BTreeMap<OrgId, OrgState>,
    unguarded_tables: BTreeSet<String>,
    role_has_bypassrls: bool,
}

/// Thread-safe in-memory [`DaiyaStore`] implementation.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: RwLock<InnerStore>,
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Simulates creating a table without `FORCE ROW LEVEL SECURITY` to test the boot check (R8).
    pub fn simulate_table_without_forced_rls(&self, table_name: impl Into<String>) {
        self.inner
            .write()
            .unguarded_tables
            .insert(table_name.into());
    }

    /// Simulates an app role with `BYPASSRLS` to test the boot check refusal (R8).
    pub fn simulate_bypassrls_role(&self, bypass: bool) {
        self.inner.write().role_has_bypassrls = bypass;
    }
}

fn check_folder_depth<'a>(
    folders: &'a BTreeMap<String, FolderRecord>,
    folder_uid: &str,
    mut parent_uid: Option<&'a str>,
) -> CrResult<()> {
    let mut depth = 0usize;
    let mut visited = BTreeSet::new();
    visited.insert(folder_uid.to_owned());
    while let Some(p_uid) = parent_uid {
        if p_uid.is_empty() {
            break;
        }
        if !visited.insert(p_uid.to_owned()) {
            return Err(CrError::BadRequest("cyclic folder parentUid".into()));
        }
        depth += 1;
        // MaxNestedFolderDepth = 4 (GF pkg/services/folder/model.go:30)
        // Grafana counts the new folder too (`len(parents)+1 >= MaxNestedFolderDepth`).
        if depth >= 4 {
            return Err(CrError::BadRequest(
                "folder nesting depth exceeds maximum allowed depth of 4".into(),
            ));
        }
        parent_uid = folders.get(p_uid).and_then(|f| f.parent_uid.as_deref());
    }
    Ok(())
}

fn upsert_folder_in(org: &mut OrgState, mut folder: FolderRecord) -> CrResult<FolderRecord> {
    if folder.uid == "general" {
        return Err(CrError::BadRequest(
            "cannot create or overwrite reserved folder uid 'general'".into(),
        ));
    }
    check_folder_depth(&org.folders, &folder.uid, folder.parent_uid.as_deref())?;

    let now = TimestampTz::now();
    if let Some(existing) = org.folders.get(&folder.uid) {
        folder.version = existing.version + 1;
        folder.created_at = existing.created_at;
        folder.updated_at = now;
    } else {
        folder.version = 1;
        folder.created_at = now;
        folder.updated_at = now;
    }
    org.folders.insert(folder.uid.clone(), folder.clone());
    Ok(folder)
}

fn upsert_dashboard_in(
    org: &mut OrgState,
    mut dashboard: DashboardRecord,
    msg: String,
    overwrite: bool,
) -> CrResult<DashboardRecord> {
    if dashboard.panel_count as u64 > org.quotas.max_panels_per_dashboard {
        return Err(CrError::QuotaExceeded {
            resource: "panels_per_dashboard".into(),
            limit: org.quotas.max_panels_per_dashboard,
        });
    }

    let now = TimestampTz::now();
    let parent_version;
    if let Some(existing) = org.dashboards.get(&dashboard.uid) {
        if !overwrite && dashboard.version != existing.version {
            return Err(CrError::PreconditionFailed(format!(
                "dashboard '{}' has been modified (stored version {}, given {})",
                dashboard.uid, existing.version, dashboard.version
            )));
        }
        if existing.is_provisioned && !existing.allow_ui_updates && !dashboard.is_provisioned {
            return Err(CrError::Forbidden(
                "cannot update provisioned dashboard when allowUiUpdates is false".into(),
            ));
        }
        parent_version = existing.version;
        dashboard.version = existing.version + 1;
        dashboard.created_at = existing.created_at;
        dashboard.updated_at = now;
    } else {
        if org.dashboards.len() as u64 >= org.quotas.max_dashboards {
            return Err(CrError::QuotaExceeded {
                resource: "dashboards".into(),
                limit: org.quotas.max_dashboards,
            });
        }
        parent_version = 0;
        dashboard.version = 1;
        dashboard.created_at = now;
        dashboard.updated_at = now;
    }

    if let Some(obj) = dashboard.data.as_object_mut() {
        obj.insert(
            "version".to_owned(),
            serde_json::Value::from(dashboard.version),
        );
        obj.insert(
            "uid".to_owned(),
            serde_json::Value::String(dashboard.uid.clone()),
        );
    }

    let ver_record = DashboardVersionRecord {
        installation_id: dashboard.installation_id,
        org_id: dashboard.org_id,
        instance_id: dashboard.instance_id,
        dashboard_uid: dashboard.uid.clone(),
        version: dashboard.version,
        parent_version,
        restored_from: 0,
        message: if msg.is_empty() {
            if parent_version == 0 {
                "Initial save".into()
            } else {
                "Updated dashboard".into()
            }
        } else {
            msg
        },
        data: dashboard.data.clone(),
        created_by: dashboard.updated_by.clone(),
        created_at: now,
    };

    org.versions
        .entry(dashboard.uid.clone())
        .or_default()
        .push(ver_record);
    org.dashboards
        .insert(dashboard.uid.clone(), dashboard.clone());
    Ok(dashboard)
}

fn upsert_datasource_in(
    org: &mut OrgState,
    mut ds: DatasourceRecord,
) -> CrResult<DatasourceRecord> {
    // One name per organization, as Grafana (`ErrDataSourceNameExists`) and the
    // Postgres store's UNIQUE (org_id, name).
    if org
        .datasources
        .values()
        .any(|other| other.uid != ds.uid && other.name == ds.name)
    {
        return Err(CrError::Conflict(format!(
            "already exists (data source name '{}')",
            ds.name
        )));
    }
    let is_new = !org.datasources.contains_key(&ds.uid);
    if is_new {
        if org.datasources.len() as u64 >= org.quotas.max_datasources {
            return Err(CrError::QuotaExceeded {
                resource: "datasources".into(),
                limit: org.quotas.max_datasources,
            });
        }
        if ds.kind.starts_with("wasm:") {
            let wasm_count = org
                .datasources
                .values()
                .filter(|d| d.kind.starts_with("wasm:"))
                .count() as u64;
            if wasm_count >= org.quotas.max_wasm_plugins {
                return Err(CrError::QuotaExceeded {
                    resource: "wasm_plugins".into(),
                    limit: org.quotas.max_wasm_plugins,
                });
            }
        }
    }

    if ds.is_default {
        for other in org.datasources.values_mut() {
            if other.uid != ds.uid {
                other.is_default = false;
            }
        }
    }

    let now = TimestampTz::now();
    if let Some(existing) = org.datasources.get(&ds.uid) {
        ds.version = existing.version + 1;
        ds.created_at = existing.created_at;
        ds.updated_at = now;
    } else {
        ds.version = 1;
        ds.created_at = now;
        ds.updated_at = now;
    }
    org.datasources.insert(ds.uid.clone(), ds.clone());
    Ok(ds)
}

/// The organization's state etag over the same orders the list methods answer.
fn etag_of(org: &OrgState) -> String {
    let folders: Vec<FolderRecord> = org.folders.values().cloned().collect();
    let dashboards: Vec<DashboardRecord> = org.dashboards.values().cloned().collect();
    let mut datasources: Vec<DatasourceRecord> = org.datasources.values().cloned().collect();
    datasources.sort_by(|a, b| a.name.cmp(&b.name));
    org_state_etag(&folders, &datasources, &dashboards)
}

impl DaiyaStore for MemoryStore {
    fn find_org(
        &self,
        installation: InstallationId,
        key: OrgKey,
    ) -> StoreFuture<'_, Option<OrgRow>> {
        Box::pin(async move {
            let guard = self.inner.read();
            let rows = guard.directory.get(&installation).into_iter().flatten();
            Ok(rows
                .filter(|row| match &key {
                    OrgKey::Number(number) => row.number == *number,
                    OrgKey::Id(id) => row.org_id == *id,
                    OrgKey::Name(name) => row.name == *name,
                })
                .max_by_key(|row| (!row.retired, row.number))
                .cloned())
        })
    }

    fn list_orgs(&self, installation: InstallationId) -> StoreFuture<'_, Vec<OrgRow>> {
        Box::pin(async move {
            let guard = self.inner.read();
            let mut live: Vec<OrgRow> = guard
                .directory
                .get(&installation)
                .into_iter()
                .flatten()
                .filter(|row| !row.retired)
                .cloned()
                .collect();
            live.sort_by_key(|row| row.number);
            Ok(live)
        })
    }

    fn create_org(
        &self,
        installation: InstallationId,
        name: &str,
        number: Option<i64>,
        first_id: OrgId,
    ) -> StoreFuture<'_, OrgRow> {
        let name = name.to_owned();
        Box::pin(async move {
            let mut guard = self.inner.write();
            let rows = guard.directory.entry(installation).or_default();
            let taken = || CrError::Conflict("Organization name taken".into());
            if let Some(held) = rows.iter().find(|row| Some(row.number) == number) {
                return if held.name == name && !held.retired {
                    Ok(held.clone())
                } else {
                    Err(taken())
                };
            }
            if rows.iter().any(|row| !row.retired && row.name == name) {
                return Err(taken());
            }
            let last = rows.iter().map(|row| row.number).max().unwrap_or(1);
            let row = OrgRow {
                number: number.unwrap_or_else(|| last.max(1).saturating_add(1)),
                org_id: if rows.iter().any(|row| row.name == name) {
                    OrgId::generate()
                } else {
                    first_id
                },
                name,
                retired: false,
            };
            if rows.iter().any(|held| held.org_id == row.org_id) {
                return Err(taken());
            }
            rows.push(row.clone());
            Ok(row)
        })
    }

    fn retire_org(&self, installation: InstallationId, number: i64) -> StoreFuture<'_, bool> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let held = guard.directory.get_mut(&installation).and_then(|rows| {
                rows.iter_mut()
                    .find(|row| row.number == number && !row.retired)
            });
            Ok(held.map(|row| row.retired = true).is_some())
        })
    }

    fn verify_rls_enforced(&self) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            let guard = self.inner.read();
            if guard.role_has_bypassrls {
                return Err(CrError::RlsNotEnforced(
                    "database role has BYPASSRLS enabled".into(),
                ));
            }
            if let Some(table) = guard.unguarded_tables.iter().next() {
                return Err(CrError::RlsNotEnforced(format!(
                    "table '{table}' does not have FORCE ROW LEVEL SECURITY enabled"
                )));
            }
            Ok(())
        })
    }

    fn get_quotas(&self, org_id: OrgId) -> StoreFuture<'_, OrgQuotas> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .map_or_else(OrgQuotas::default, |o| o.quotas.clone()))
        })
    }

    fn set_quotas(
        &self,
        _installation: InstallationId,
        org_id: OrgId,
        _instance: InstanceId,
        quotas: OrgQuotas,
    ) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            self.inner.write().orgs.entry(org_id).or_default().quotas = quotas;
            Ok(())
        })
    }

    fn list_folders(&self, org_id: OrgId) -> StoreFuture<'_, Vec<FolderRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .map(|o| o.folders.values().cloned().collect())
                .unwrap_or_default())
        })
    }

    fn get_folder(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, Option<FolderRecord>> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .and_then(|o| o.folders.get(&uid).cloned()))
        })
    }

    fn upsert_folder(&self, folder: FolderRecord) -> StoreFuture<'_, FolderRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            upsert_folder_in(guard.orgs.entry(folder.org_id).or_default(), folder)
        })
    }

    fn delete_folder(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut guard = self.inner.write();
            let Some(org) = guard.orgs.get_mut(&org_id) else {
                return Ok(false);
            };
            let removed = org.folders.remove(&uid).is_some();
            if removed {
                org.dashboards
                    .retain(|_, d| d.folder_uid.as_deref() != Some(uid.as_str()));
                let kept = &org.dashboards;
                org.versions.retain(|d, _| kept.contains_key(d));
                org.public_dashboards
                    .retain(|_, p| kept.contains_key(&p.dashboard_uid));
            }
            Ok(removed)
        })
    }

    fn list_dashboards(&self, org_id: OrgId) -> StoreFuture<'_, Vec<DashboardRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .map(|o| o.dashboards.values().cloned().collect())
                .unwrap_or_default())
        })
    }

    fn get_dashboard(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, Option<DashboardRecord>> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .and_then(|o| o.dashboards.get(&uid).cloned()))
        })
    }

    fn upsert_dashboard(
        &self,
        dashboard: DashboardRecord,
        message: &str,
        overwrite: bool,
    ) -> StoreFuture<'_, DashboardRecord> {
        let msg = message.to_owned();
        Box::pin(async move {
            let mut guard = self.inner.write();
            let org = guard.orgs.entry(dashboard.org_id).or_default();
            upsert_dashboard_in(org, dashboard, msg, overwrite)
        })
    }

    fn delete_dashboard(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut guard = self.inner.write();
            let Some(org) = guard.orgs.get_mut(&org_id) else {
                return Ok(false);
            };
            let removed = org.dashboards.remove(&uid).is_some();
            if removed {
                org.versions.remove(&uid);
                org.public_dashboards.retain(|_, p| p.dashboard_uid != uid);
            }
            Ok(removed)
        })
    }

    fn list_dashboard_versions(
        &self,
        org_id: OrgId,
        dashboard_uid: &str,
    ) -> StoreFuture<'_, Vec<DashboardVersionRecord>> {
        let uid = dashboard_uid.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .and_then(|o| o.versions.get(&uid).cloned())
                .unwrap_or_default())
        })
    }

    fn list_datasources(&self, org_id: OrgId) -> StoreFuture<'_, Vec<DatasourceRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .map(|o| {
                    // Sorted by name, as Grafana lists them and the Postgres store does.
                    let mut all: Vec<DatasourceRecord> = o.datasources.values().cloned().collect();
                    all.sort_by(|a, b| a.name.cmp(&b.name));
                    all
                })
                .unwrap_or_default())
        })
    }

    fn get_datasource(
        &self,
        org_id: OrgId,
        uid: &str,
    ) -> StoreFuture<'_, Option<DatasourceRecord>> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .and_then(|o| o.datasources.get(&uid).cloned()))
        })
    }

    fn get_datasource_by_name(
        &self,
        org_id: OrgId,
        name: &str,
    ) -> StoreFuture<'_, Option<DatasourceRecord>> {
        let name = name.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .and_then(|o| o.datasources.values().find(|ds| ds.name == name).cloned()))
        })
    }

    fn upsert_datasource(&self, ds: DatasourceRecord) -> StoreFuture<'_, DatasourceRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            upsert_datasource_in(guard.orgs.entry(ds.org_id).or_default(), ds)
        })
    }

    fn delete_datasource(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut guard = self.inner.write();
            let Some(org) = guard.orgs.get_mut(&org_id) else {
                return Ok(false);
            };
            if let Some(ds) = org.datasources.get(&uid)
                && ds.read_only
            {
                return Err(CrError::Forbidden(
                    "Cannot delete read-only data source".into(),
                ));
            }
            Ok(org.datasources.remove(&uid).is_some())
        })
    }

    fn list_library_panels(&self, org_id: OrgId) -> StoreFuture<'_, Vec<LibraryPanelRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .map(|o| o.library_panels.values().cloned().collect())
                .unwrap_or_default())
        })
    }

    fn get_library_panel(
        &self,
        org_id: OrgId,
        uid: &str,
    ) -> StoreFuture<'_, Option<LibraryPanelRecord>> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .and_then(|o| o.library_panels.get(&uid).cloned()))
        })
    }

    fn upsert_library_panel(
        &self,
        mut panel: LibraryPanelRecord,
    ) -> StoreFuture<'_, LibraryPanelRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let org = guard.orgs.entry(panel.org_id).or_default();
            let now = TimestampTz::now();
            if let Some(existing) = org.library_panels.get(&panel.uid) {
                panel.version = existing.version + 1;
                panel.created_at = existing.created_at;
                panel.updated_at = now;
            } else {
                panel.version = 1;
                panel.created_at = now;
                panel.updated_at = now;
            }
            org.library_panels.insert(panel.uid.clone(), panel.clone());
            Ok(panel)
        })
    }

    fn delete_library_panel(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut guard = self.inner.write();
            Ok(guard
                .orgs
                .get_mut(&org_id)
                .is_some_and(|o| o.library_panels.remove(&uid).is_some()))
        })
    }

    fn list_public_dashboards(&self, org_id: OrgId) -> StoreFuture<'_, Vec<PublicDashboardRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .map(|o| o.public_dashboards.values().cloned().collect())
                .unwrap_or_default())
        })
    }

    fn get_public_dashboard(
        &self,
        org_id: OrgId,
        uid: &str,
    ) -> StoreFuture<'_, Option<PublicDashboardRecord>> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .and_then(|o| o.public_dashboards.get(&uid).cloned()))
        })
    }

    fn get_public_dashboard_by_dashboard_uid(
        &self,
        org_id: OrgId,
        dashboard_uid: &str,
    ) -> StoreFuture<'_, Option<PublicDashboardRecord>> {
        let duid = dashboard_uid.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard.orgs.get(&org_id).and_then(|o| {
                o.public_dashboards
                    .values()
                    .find(|p| p.dashboard_uid == duid)
                    .cloned()
            }))
        })
    }

    fn get_public_dashboard_by_token_hash(
        &self,
        token_hash: &str,
    ) -> StoreFuture<'_, Option<(PublicDashboardRecord, DashboardRecord)>> {
        let th = token_hash.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            for org in guard.orgs.values() {
                for pd in org.public_dashboards.values() {
                    if constant_time_eq(&pd.access_token_hash, &th)
                        && let Some(dash) = org.dashboards.get(&pd.dashboard_uid)
                    {
                        return Ok(Some((pd.clone(), dash.clone())));
                    }
                }
            }
            Ok(None)
        })
    }

    fn upsert_public_dashboard(
        &self,
        mut pubdash: PublicDashboardRecord,
    ) -> StoreFuture<'_, PublicDashboardRecord> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let org = guard.orgs.entry(pubdash.org_id).or_default();
            if !org.dashboards.contains_key(&pubdash.dashboard_uid) {
                return Err(CrError::NotFound(format!(
                    "dashboard '{}' not found",
                    pubdash.dashboard_uid
                )));
            }
            let now = TimestampTz::now();
            if let Some(existing) = org.public_dashboards.get(&pubdash.uid) {
                pubdash.created_at = existing.created_at;
                pubdash.updated_at = now;
            } else {
                pubdash.created_at = now;
                pubdash.updated_at = now;
            }
            let mut stored = pubdash.clone();
            stored.access_token = None; // Never keep raw token in persistent store!
            org.public_dashboards.insert(stored.uid.clone(), stored);
            Ok(pubdash)
        })
    }

    fn delete_public_dashboard(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool> {
        let uid = uid.to_owned();
        Box::pin(async move {
            let mut guard = self.inner.write();
            Ok(guard
                .orgs
                .get_mut(&org_id)
                .is_some_and(|o| o.public_dashboards.remove(&uid).is_some()))
        })
    }

    fn read_rows(&self, org_id: OrgId, query: RowQuery) -> StoreFuture<'_, RowPage> {
        Box::pin(async move {
            let guard = self.inner.read();
            let rows = guard
                .orgs
                .get(&org_id)
                .map(|org| index_rows(org, query.set))
                .unwrap_or_default();
            Ok(daiya_model::rows::answer_rows(rows, &query))
        })
    }

    fn list_import_reports(&self, org_id: OrgId) -> StoreFuture<'_, Vec<ImportReportRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .map(|o| {
                    // Newest first, as the Postgres store (`ORDER BY created_at DESC`).
                    let mut all = o.import_reports.clone();
                    all.sort_by_key(|r| std::cmp::Reverse(r.created_at));
                    all
                })
                .unwrap_or_default())
        })
    }

    fn insert_import_report(
        &self,
        _installation: InstallationId,
        report: ImportReportRecord,
    ) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            guard
                .orgs
                .entry(report.org_id)
                .or_default()
                .import_reports
                .push(report);
            Ok(())
        })
    }

    fn append_audit(&self, row: AuditRecord) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            guard.orgs.entry(row.org_id).or_default().audit.push(row);
            Ok(())
        })
    }

    fn list_audit(&self, org_id: OrgId) -> StoreFuture<'_, Vec<AuditRecord>> {
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .map(|o| o.audit.clone())
                .unwrap_or_default())
        })
    }

    fn insert_plan(&self, plan: PlanRecord) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let org = guard.orgs.entry(plan.org_id).or_default();
            let now = TimestampTz::now();
            org.plans.retain(|_, p| p.expires_at > now);
            if org.plans.keys().filter(|d| **d != plan.digest).count() as u64 >= MAX_LIVE_PLANS {
                return Err(CrError::QuotaExceeded {
                    resource: "plans".into(),
                    limit: MAX_LIVE_PLANS,
                });
            }
            org.plans.insert(plan.digest.clone(), plan);
            Ok(())
        })
    }

    fn get_plan(&self, org_id: OrgId, digest: &str) -> StoreFuture<'_, Option<PlanRecord>> {
        let digest = digest.to_owned();
        Box::pin(async move {
            let guard = self.inner.read();
            Ok(guard
                .orgs
                .get(&org_id)
                .and_then(|o| o.plans.get(&digest).cloned()))
        })
    }

    fn apply_bundle(&self, bundle: ApplyBundle) -> StoreFuture<'_, Vec<DashboardRecord>> {
        Box::pin(async move {
            let mut guard = self.inner.write();
            let live = guard.orgs.entry(bundle.org_id).or_default();
            let current = etag_of(live);
            if current != bundle.base_etag {
                return Err(CrError::StalePlan(format!(
                    "organization state changed since plan was created (expected etag {}, found {current})",
                    bundle.base_etag
                )));
            }
            // Written to a copy that replaces the organization only when every write passed.
            let mut org = live.clone();
            for folder in bundle.folders {
                upsert_folder_in(&mut org, folder)?;
            }
            for ds in bundle.datasources {
                upsert_datasource_in(&mut org, ds)?;
            }
            let mut saved = Vec::with_capacity(bundle.dashboards.len());
            for dashboard in bundle.dashboards {
                saved.push(upsert_dashboard_in(
                    &mut org,
                    dashboard,
                    bundle.message.clone(),
                    true,
                )?);
            }
            *live = org;
            Ok(saved)
        })
    }
}

/// The index rows of `set`, member for member what daiya-store-pg selects.
fn index_rows(org: &OrgState, set: RowSet) -> Vec<Value> {
    match set {
        RowSet::Dashboards => org
            .dashboards
            .values()
            .map(|d| {
                json!({
                    "uid": d.uid,
                    "title": d.title,
                    "slug": d.slug,
                    "folderUid": d.folder_uid,
                    "panelCount": d.panel_count,
                    "version": d.version,
                    "schemaVersion": d.schema_version,
                    "isProvisioned": d.is_provisioned,
                    "updatedBy": d.updated_by,
                    "updatedAt": d.updated_at.to_rfc3339(),
                    "tags": d.tags,
                    "provisionedExternalId": d.provisioned_external_id,
                })
            })
            .collect(),
        RowSet::DashboardVersions => org
            .dashboards
            .values()
            .flat_map(|d| {
                org.versions.get(&d.uid).into_iter().flatten().map(|v| {
                    json!({
                        "id": format!("{}:{}", v.dashboard_uid, v.version),
                        "dashboardUid": v.dashboard_uid,
                        "version": v.version,
                        "parentVersion": v.parent_version,
                        "restoredFrom": v.restored_from,
                        "message": v.message,
                        "createdBy": v.created_by,
                        "createdAt": v.created_at.to_rfc3339(),
                        "folderUid": d.folder_uid,
                    })
                })
            })
            .collect(),
        RowSet::Folders => org
            .folders
            .values()
            .map(|f| {
                json!({
                    "uid": f.uid,
                    "parentUid": f.parent_uid,
                    "title": f.title,
                    "version": f.version,
                    "managedBy": f.managed_by,
                    "updatedAt": f.updated_at.to_rfc3339(),
                })
            })
            .collect(),
        RowSet::Datasources => org
            .datasources
            .values()
            .map(|ds| {
                json!({
                    "uid": ds.uid,
                    "name": ds.name,
                    "type": ds.kind,
                    "access": ds.access,
                    "url": ds.url,
                    "isDefault": ds.is_default,
                    "readOnly": ds.read_only,
                    "wasmDigest": ds.wasm_digest,
                    "version": ds.version,
                })
            })
            .collect(),
        RowSet::LibraryPanels => org
            .library_panels
            .values()
            .map(|p| {
                json!({
                    "uid": p.uid,
                    "folderUid": p.folder_uid,
                    "name": p.name,
                    "type": p.panel_type,
                    "description": p.description,
                    "version": p.version,
                })
            })
            .collect(),
        RowSet::PublicDashboards => org
            .public_dashboards
            .values()
            .map(|p| {
                json!({
                    "uid": p.uid,
                    "dashboardUid": p.dashboard_uid,
                    "isEnabled": p.is_enabled,
                    "timeSelectionEnabled": p.time_selection_enabled,
                    "annotationsEnabled": p.annotations_enabled,
                    "showQueryText": p.show_query_text,
                    "share": p.share.as_str(),
                })
            })
            .collect(),
        RowSet::ImportReports => org
            .import_reports
            .iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "dryRun": r.dry_run,
                    "status": r.status,
                    "dashboardCount": r.dashboard_count,
                    "refusedCount": r.refused_count,
                    "mappedCount": r.mapped_count,
                    "createdAt": r.created_at.to_rfc3339(),
                })
            })
            .collect(),
    }
}
