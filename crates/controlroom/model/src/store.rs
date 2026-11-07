#![forbid(unsafe_code)]
//! Async `ControlroomStore` trait implemented by `controlroom-store-pg` and `controlroom-store-memory` (R1, R8).

use crate::entities::{
    DashboardRecord, DashboardVersionRecord, DatasourceRecord, FolderRecord, ImportReportRecord,
    LibraryPanelRecord, PublicDashboardRecord, QueryCacheStatRecord,
};
use crate::error::CrResult;
use crate::ids::OrgId;
use crate::limits::OrgQuotas;
use std::future::Future;
use std::pin::Pin;

pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = CrResult<T>> + Send + 'a>>;

/// Storage interface for all `controlroom` entities, scoped per organization (`OrgId`).
pub trait ControlroomStore: Send + Sync {
    /// Verifies that forced RLS is active on every table and the app role does not have `BYPASSRLS` (R8).
    fn verify_rls_enforced(&self) -> StoreFuture<'_, ()>;

    // Quotas
    fn get_quotas(&self, org_id: OrgId) -> StoreFuture<'_, OrgQuotas>;
    fn set_quotas(&self, org_id: OrgId, quotas: OrgQuotas) -> StoreFuture<'_, ()>;

    // Folders
    fn list_folders(&self, org_id: OrgId) -> StoreFuture<'_, Vec<FolderRecord>>;
    fn get_folder(&self, org_id: OrgId, uid: &str) -> StoreFuture<'_, Option<FolderRecord>>;
    fn upsert_folder(&self, folder: FolderRecord) -> StoreFuture<'_, FolderRecord>;
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
    fn get_datasource(
        &self,
        org_id: OrgId,
        uid: &str,
    ) -> StoreFuture<'_, Option<DatasourceRecord>>;
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
    fn list_query_cache_stats(&self, org_id: OrgId) -> StoreFuture<'_, Vec<QueryCacheStatRecord>>;
    fn upsert_query_cache_stat(&self, stat: QueryCacheStatRecord) -> StoreFuture<'_, ()>;
    fn list_import_reports(&self, org_id: OrgId) -> StoreFuture<'_, Vec<ImportReportRecord>>;
    fn insert_import_report(&self, report: ImportReportRecord) -> StoreFuture<'_, ()>;
}
