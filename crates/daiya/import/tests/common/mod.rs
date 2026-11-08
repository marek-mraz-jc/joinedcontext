//! A store that behaves like [`MemoryStore`] except for the methods a test names, which fail
//! the way a lost database does (500), so the import paths' error handling runs for real.

#![allow(dead_code)]

use daiya_model::store::StoreFuture;
use daiya_model::{
    CrError, DaiyaStore, DashboardRecord, DashboardVersionRecord, DatasourceRecord, FolderRecord,
    ImportReportRecord, LibraryPanelRecord, OrgId, OrgQuotas, PublicDashboardRecord,
    QueryCacheStatRecord,
};
use daiya_store_memory::MemoryStore;
use std::collections::BTreeSet;
use std::sync::Mutex;

pub struct FailingStore {
    pub inner: MemoryStore,
    failing: Mutex<BTreeSet<&'static str>>,
}

impl FailingStore {
    pub fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            failing: Mutex::new(BTreeSet::new()),
        }
    }

    /// From now on `method` fails with a 500.
    pub fn fail(&self, method: &'static str) {
        self.failing.lock().unwrap().insert(method);
    }

    pub fn heal(&self) {
        self.failing.lock().unwrap().clear();
    }

    fn fails(&self, method: &str) -> bool {
        self.failing.lock().unwrap().contains(method)
    }
}

macro_rules! delegate {
    ($name:ident ( $($arg:ident : $ty:ty),* $(,)? ) -> $out:ty) => {
        fn $name(&self, $($arg: $ty),*) -> StoreFuture<'_, $out> {
            if self.fails(stringify!($name)) {
                return Box::pin(async {
                    Err(CrError::Internal(concat!("injected failure in ", stringify!($name)).into()))
                });
            }
            self.inner.$name($($arg),*)
        }
    };
}

impl DaiyaStore for FailingStore {
    delegate!(verify_rls_enforced() -> ());
    delegate!(get_quotas(org_id: OrgId) -> OrgQuotas);
    delegate!(set_quotas(org_id: OrgId, quotas: OrgQuotas) -> ());
    delegate!(list_folders(org_id: OrgId) -> Vec<FolderRecord>);
    delegate!(get_folder(org_id: OrgId, uid: &str) -> Option<FolderRecord>);
    delegate!(upsert_folder(folder: FolderRecord) -> FolderRecord);
    delegate!(delete_folder(org_id: OrgId, uid: &str) -> bool);
    delegate!(list_dashboards(org_id: OrgId) -> Vec<DashboardRecord>);
    delegate!(get_dashboard(org_id: OrgId, uid: &str) -> Option<DashboardRecord>);
    delegate!(
        upsert_dashboard(
            dashboard: DashboardRecord,
            message: &str,
            overwrite: bool,
        ) -> DashboardRecord
    );
    delegate!(delete_dashboard(org_id: OrgId, uid: &str) -> bool);
    delegate!(
        list_dashboard_versions(org_id: OrgId, dashboard_uid: &str) -> Vec<DashboardVersionRecord>
    );
    delegate!(list_datasources(org_id: OrgId) -> Vec<DatasourceRecord>);
    delegate!(get_datasource(org_id: OrgId, uid: &str) -> Option<DatasourceRecord>);
    delegate!(get_datasource_by_name(org_id: OrgId, name: &str) -> Option<DatasourceRecord>);
    delegate!(upsert_datasource(ds: DatasourceRecord) -> DatasourceRecord);
    delegate!(delete_datasource(org_id: OrgId, uid: &str) -> bool);
    delegate!(list_library_panels(org_id: OrgId) -> Vec<LibraryPanelRecord>);
    delegate!(get_library_panel(org_id: OrgId, uid: &str) -> Option<LibraryPanelRecord>);
    delegate!(upsert_library_panel(panel: LibraryPanelRecord) -> LibraryPanelRecord);
    delegate!(delete_library_panel(org_id: OrgId, uid: &str) -> bool);
    delegate!(list_public_dashboards(org_id: OrgId) -> Vec<PublicDashboardRecord>);
    delegate!(get_public_dashboard(org_id: OrgId, uid: &str) -> Option<PublicDashboardRecord>);
    delegate!(
        get_public_dashboard_by_dashboard_uid(
            org_id: OrgId,
            dashboard_uid: &str,
        ) -> Option<PublicDashboardRecord>
    );
    delegate!(
        get_public_dashboard_by_token_hash(
            token_hash: &str,
        )
            -> Option<(PublicDashboardRecord, DashboardRecord)>
    );
    delegate!(upsert_public_dashboard(pubdash: PublicDashboardRecord) -> PublicDashboardRecord);
    delegate!(delete_public_dashboard(org_id: OrgId, uid: &str) -> bool);
    delegate!(list_query_cache_stats(org_id: OrgId) -> Vec<QueryCacheStatRecord>);
    delegate!(upsert_query_cache_stat(stat: QueryCacheStatRecord) -> ());
    delegate!(list_import_reports(org_id: OrgId) -> Vec<ImportReportRecord>);
    delegate!(insert_import_report(report: ImportReportRecord) -> ());
}
