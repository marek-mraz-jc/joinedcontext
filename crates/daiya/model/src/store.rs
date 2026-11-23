#![forbid(unsafe_code)]
//! Async `DaiyaStore` trait implemented by `daiya-store-pg` and `daiya-store-memory` (R1, R8).

use crate::entities::{
    ApplyBundle, AuditRecord, DashboardRecord, DashboardVersionRecord, DatasourceRecord,
    FolderRecord, ImportReportRecord, LibraryPanelRecord, PlanRecord, PublicDashboardRecord,
};
use crate::error::{CrError, CrResult};
use crate::ids::{InstallationId, InstanceId, OrgId};
use crate::limits::OrgQuotas;
use crate::rows::{RowPage, RowQuery};
use std::future::Future;
use std::pin::Pin;

pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = CrResult<T>> + Send + 'a>>;

/// An organization of the installation (T-7509). Its number and id are the store's, so every
/// replica and restart answers alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgRow {
    /// Grafana's `orgId`, from 2: 1 is the main organization, which has no row.
    pub number: i64,
    pub name: String,
    pub org_id: OrgId,
    /// A retired organization keeps its number and id; its name is free.
    pub retired: bool,
}

/// What an organization is looked up by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgKey {
    /// Live or retired.
    Number(i64),
    /// Live or retired.
    Id(OrgId),
    /// The live one of that name, else the last retired one.
    Name(String),
}

/// Storage interface for all `daiya` entities, scoped per organization (`OrgId`).
pub trait DaiyaStore: Send + Sync {
    /// Verifies that forced RLS is active on every table and the app role does not have `BYPASSRLS` (R8).
    fn verify_rls_enforced(&self) -> StoreFuture<'_, ()>;

    // The installation's organizations
    fn find_org(
        &self,
        installation: InstallationId,
        key: OrgKey,
    ) -> StoreFuture<'_, Option<OrgRow>>;
    /// The live ones, by number.
    fn list_orgs(&self, installation: InstallationId) -> StoreFuture<'_, Vec<OrgRow>>;
    /// Holds `name`, under `number` when one is declared, else the next one. The id is `first_id`
    /// the first time the installation holds the name, a new random one once an organization of
    /// that name was retired. A declared organization already held answers its row; any other
    /// name or number held is a conflict.
    fn create_org(
        &self,
        installation: InstallationId,
        name: &str,
        number: Option<i64>,
        first_id: OrgId,
    ) -> StoreFuture<'_, OrgRow>;
    /// Whether a live organization had that number; it is retired now.
    fn retire_org(&self, installation: InstallationId, number: i64) -> StoreFuture<'_, bool>;

    // Quotas
    fn get_quotas(&self, org_id: OrgId) -> StoreFuture<'_, OrgQuotas>;
    fn set_quotas(
        &self,
        installation: InstallationId,
        org_id: OrgId,
        instance: InstanceId,
        quotas: OrgQuotas,
    ) -> StoreFuture<'_, ()>;

    // Folders
    fn list_folders(&self, org_id: OrgId) -> StoreFuture<'_, Vec<FolderRecord>>;
    fn get_folder(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, Option<FolderRecord>>;
    fn upsert_folder(&self, folder: FolderRecord) -> StoreFuture<'_, FolderRecord>;
    /// Deletes an empty folder; whether it existed. A folder that still holds dashboards,
    /// subfolders or library panels is refused with [`folder_content_refusal`]'s conflict and
    /// nothing is deleted (S9-36).
    fn delete_folder(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool>;

    // Dashboards & Versions
    fn list_dashboards(&self, org_id: OrgId) -> StoreFuture<'_, Vec<DashboardRecord>>;
    fn get_dashboard(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, Option<DashboardRecord>>;
    fn upsert_dashboard(
        &self,
        dashboard: DashboardRecord,
        message: &str,
        overwrite: bool,
    ) -> StoreFuture<'_, DashboardRecord>;
    fn delete_dashboard(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool>;
    fn list_dashboard_versions(
        &self,
        org_id: OrgId,
        dashboard_uid: &str,
    ) -> StoreFuture<'_, Vec<DashboardVersionRecord>>;

    // Datasources
    fn list_datasources(&self, org_id: OrgId) -> StoreFuture<'_, Vec<DatasourceRecord>>;
    fn get_datasource(&self, org_id: OrgId, uid: &str)
    -> StoreFuture<'_, Option<DatasourceRecord>>;
    fn get_datasource_by_name(
        &self,
        org_id: OrgId,
        name: &str,
    ) -> StoreFuture<'_, Option<DatasourceRecord>>;
    fn upsert_datasource(&self, ds: DatasourceRecord) -> StoreFuture<'_, DatasourceRecord>;
    fn delete_datasource(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool>;

    // Library Panels
    fn list_library_panels(&self, org_id: OrgId) -> StoreFuture<'_, Vec<LibraryPanelRecord>>;
    fn get_library_panel(
        &self,
        org_id: OrgId,
        uid: &str,
    ) -> StoreFuture<'_, Option<LibraryPanelRecord>>;
    fn upsert_library_panel(
        &self,
        panel: LibraryPanelRecord,
    ) -> StoreFuture<'_, LibraryPanelRecord>;
    fn delete_library_panel(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool>;

    // Public Dashboards
    fn list_public_dashboards(&self, org_id: OrgId) -> StoreFuture<'_, Vec<PublicDashboardRecord>>;
    fn get_public_dashboard(
        &self,
        org_id: OrgId,
        uid: &str,
    ) -> StoreFuture<'_, Option<PublicDashboardRecord>>;
    fn get_public_dashboard_by_dashboard_uid(
        &self,
        org_id: OrgId,
        dashboard_uid: &str,
    ) -> StoreFuture<'_, Option<PublicDashboardRecord>>;
    /// Looks up a public dashboard by the SHA-256 hash of its access token (R7).
    fn get_public_dashboard_by_token_hash(
        &self,
        token_hash: &str,
    ) -> StoreFuture<'_, Option<(PublicDashboardRecord, DashboardRecord)>>;
    fn upsert_public_dashboard(
        &self,
        pubdash: PublicDashboardRecord,
    ) -> StoreFuture<'_, PublicDashboardRecord>;
    fn delete_public_dashboard(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, bool>;

    // Query Cache Stats & Import Reports
    /// Index rows of one set in `org_id`, never a model: see [`RowQuery`].
    fn read_rows(&self, org_id: OrgId, query: RowQuery) -> StoreFuture<'_, RowPage>;

    fn list_import_reports(&self, org_id: OrgId) -> StoreFuture<'_, Vec<ImportReportRecord>>;
    fn insert_import_report(
        &self,
        installation: InstallationId,
        report: ImportReportRecord,
    ) -> StoreFuture<'_, ()>;

    // Audit (S9-28): append only, written in the transaction of the change it records.
    fn append_audit(&self, row: AuditRecord) -> StoreFuture<'_, ()>;
    fn list_audit(&self, org_id: OrgId) -> StoreFuture<'_, Vec<AuditRecord>>;

    // Plans and Apply (R2, §7.3)
    /// Keeps `plan` (a repeated `(org_id, digest)` renews its creator and expiry) and drops the
    /// organization's expired plans.
    fn insert_plan(&self, plan: PlanRecord) -> StoreFuture<'_, ()>;
    /// The organization's plan of `digest`, expired or not (the caller checks `expires_at`).
    fn get_plan(&self, org_id: OrgId, digest: &str) -> StoreFuture<'_, Option<PlanRecord>>;
    /// Writes the whole bundle or nothing: one transaction (one write lock) that re-checks the
    /// organization's state etag against `bundle.base_etag` under an organization lock
    /// (`CrError::StalePlan` when it moved) and runs every save check of the single upserts.
    /// Answers the dashboards as saved.
    fn apply_bundle(&self, bundle: ApplyBundle) -> StoreFuture<'_, Vec<DashboardRecord>>;
}

/// How many uids of each kind a folder-delete refusal names before it counts the rest.
const REFUSAL_NAMES: usize = 10;

/// The 409 for deleting folder `uid` while it holds content: names up to ten uids of each
/// kind and counts the rest, so the caller can move or delete them first. `Ok` when empty.
pub fn folder_content_refusal(
    uid: &str,
    dashboards: Vec<String>,
    subfolders: Vec<String>,
    library_panels: Vec<String>,
) -> CrResult<()> {
    let parts: Vec<String> = [
        ("dashboards", dashboards),
        ("subfolders", subfolders),
        ("library panels", library_panels),
    ]
    .into_iter()
    .filter(|(_, uids)| !uids.is_empty())
    .map(|(kind, mut uids)| {
        let total = uids.len();
        uids.sort();
        uids.truncate(REFUSAL_NAMES);
        let more = total - uids.len();
        let tail = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        format!("{total} {kind} ({}{tail})", uids.join(", "))
    })
    .collect();
    if parts.is_empty() {
        return Ok(());
    }
    Err(CrError::Conflict(format!(
        "Folder '{uid}' is not empty: it holds {}. Move or delete them first.",
        parts.join("; ")
    )))
}

#[cfg(test)]
mod tests {
    use super::folder_content_refusal;

    #[test]
    fn a_refusal_names_ten_uids_per_kind_and_counts_the_rest() {
        assert!(folder_content_refusal("f", vec![], vec![], vec![]).is_ok());
        let many: Vec<String> = (0..12).map(|i| format!("d{i:02}")).collect();
        let err = folder_content_refusal("f", many, vec!["sub".into()], vec![]).unwrap_err();
        assert_eq!(err.status_code(), 409);
        let text = err.to_string();
        assert!(text.contains("12 dashboards (d00, d01"), "{text}");
        assert!(text.contains("d09 and 2 more)"), "{text}");
        assert!(!text.contains("d10"), "{text}");
        assert!(text.contains("1 subfolders (sub)"), "{text}");
        assert!(!text.contains("library"), "{text}");
    }
}
