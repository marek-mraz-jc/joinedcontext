//! A dataroom `Store` over another one that refuses the calls a test names, as
//! [`super::faulty_store`] does for kaisatsu: the failure arm of every dataroom store call a jc
//! module makes is forced with it, never by a broken database. Unit tests in `src/` include this
//! file by `#[path]`; it is test code, kept out of the coverage fold with the rest of `tests/`.

use dataroom_core::store::BoxFuture;
use dataroom_core::{
    ActivityRow, Catalogue, CatalogueId, CataloguePlugin, DataroomError, Dataset, Distribution,
    HarvestRun, HarvestSource, OrgId, ProfileRecord, Publisher, RequestCtx, ResourceView, Result,
    ShaclValidationReport, StaticPage, Store,
};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

/// The error every refused call answers.
pub const REFUSED: &str = "injected dataroom store failure";

pub struct FaultyDataroom {
    inner: Arc<dyn Store>,
    failing: Mutex<BTreeSet<&'static str>>,
}

impl FaultyDataroom {
    pub fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            failing: Mutex::new(BTreeSet::new()),
        }
    }

    /// From now on the store method `method` answers [`REFUSED`].
    pub fn fail(&self, method: &'static str) {
        self.failing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(method);
    }

    fn refusal(&self, method: &str) -> Option<DataroomError> {
        self.failing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(method)
            .then(|| DataroomError::Storage(REFUSED.to_owned()))
    }
}

macro_rules! forward {
    ($( $name:ident<$lt:lifetime>($($arg:ident : $ty:ty),*) -> $ret:ty; )*) => {
        $(
            fn $name<$lt>(&$lt self, $($arg: $ty),*) -> BoxFuture<$lt, Result<$ret>> {
                if let Some(e) = self.refusal(stringify!($name)) {
                    return Box::pin(async move { Err(e) });
                }
                self.inner.$name($($arg),*)
            }
        )*
    };
}

impl Store for FaultyDataroom {
    forward! {
        get_catalogue<'a>(org_id: OrgId, catalogue_id: CatalogueId) -> Option<Catalogue>;
        resolve_host<'a>(host: &'a str) -> Option<Catalogue>;
        list_catalogues<'a>(org_id: OrgId) -> Vec<Catalogue>;
        upsert_catalogue<'a>(catalogue: Catalogue) -> Catalogue;
        save_catalogue<'a>(catalogue: Catalogue, expected: Option<jc_time::TimestampTz>, activity: ActivityRow) -> Catalogue;
        delete_catalogue<'a>(org_id: OrgId, catalogue_id: CatalogueId) -> bool;
        list_publishers<'a>(ctx: &'a RequestCtx) -> Vec<Publisher>;
        get_publisher<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> Option<Publisher>;
        upsert_publisher<'a>(ctx: &'a RequestCtx, publisher: Publisher) -> Publisher;
        save_publisher<'a>(ctx: &'a RequestCtx, publisher: Publisher, expected: Option<jc_time::TimestampTz>, activity: ActivityRow) -> Publisher;
        merge_publishers<'a>(ctx: &'a RequestCtx, source: &'a str, target: &'a str, activity: ActivityRow) -> Publisher;
        withdraw_publisher<'a>(ctx: &'a RequestCtx, id_or_name: &'a str, activity: ActivityRow) -> Publisher;
        list_datasets<'a>(ctx: &'a RequestCtx) -> Vec<Dataset>;
        get_dataset<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> Option<Dataset>;
        upsert_dataset<'a>(ctx: &'a RequestCtx, dataset: Dataset) -> Dataset;
        save_dataset<'a>(ctx: &'a RequestCtx, dataset: Dataset, expected: Option<jc_time::TimestampTz>, activity: ActivityRow) -> Dataset;
        withdraw_dataset<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> Dataset;
        purge_dataset<'a>(ctx: &'a RequestCtx, id_or_name: &'a str, activity: ActivityRow) -> ();
        get_distribution<'a>(ctx: &'a RequestCtx, id: &'a str) -> Option<(Dataset, Distribution)>;
        list_resource_views<'a>(ctx: &'a RequestCtx, resource_id: &'a str) -> Vec<ResourceView>;
        get_resource_view<'a>(ctx: &'a RequestCtx, view_id: &'a str) -> Option<ResourceView>;
        upsert_resource_view<'a>(ctx: &'a RequestCtx, view: ResourceView) -> ResourceView;
        save_resource_view<'a>(ctx: &'a RequestCtx, view: ResourceView, activity: ActivityRow) -> ResourceView;
        delete_resource_view<'a>(ctx: &'a RequestCtx, view_id: &'a str, activity: ActivityRow) -> ();
        list_harvest_sources<'a>(ctx: &'a RequestCtx) -> Vec<HarvestSource>;
        get_harvest_source<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> Option<HarvestSource>;
        upsert_harvest_source<'a>(ctx: &'a RequestCtx, source: HarvestSource) -> HarvestSource;
        save_harvest_source<'a>(ctx: &'a RequestCtx, source: HarvestSource, activity: ActivityRow) -> HarvestSource;
        delete_harvest_source<'a>(ctx: &'a RequestCtx, id_or_name: &'a str) -> bool;
        list_harvest_runs<'a>(ctx: &'a RequestCtx, source_id: Option<&'a str>) -> Vec<HarvestRun>;
        get_harvest_run<'a>(ctx: &'a RequestCtx, run_id: &'a str) -> Option<HarvestRun>;
        upsert_harvest_run<'a>(ctx: &'a RequestCtx, run: HarvestRun) -> HarvestRun;
        claim_harvest_run<'a>(ctx: &'a RequestCtx, run: HarvestRun, stale_before: jc_time::TimestampTz) -> bool;
        record_activity<'a>(ctx: &'a RequestCtx, row: ActivityRow) -> ();
        list_activity<'a>(ctx: &'a RequestCtx, object_id: Option<&'a str>, limit: usize) -> Vec<ActivityRow>;
        upsert_shacl_report<'a>(ctx: &'a RequestCtx, report: ShaclValidationReport) -> ShaclValidationReport;
        list_shacl_reports<'a>(ctx: &'a RequestCtx, dataset_id: Option<&'a str>, profile_id: Option<&'a str>) -> Vec<ShaclValidationReport>;
        list_profiles<'a>(ctx: &'a RequestCtx) -> Vec<ProfileRecord>;
        upsert_profile<'a>(ctx: &'a RequestCtx, profile: ProfileRecord) -> ProfileRecord;
        save_profile<'a>(ctx: &'a RequestCtx, profile: ProfileRecord, activity: ActivityRow) -> ProfileRecord;
        list_pages<'a>(ctx: &'a RequestCtx) -> Vec<StaticPage>;
        upsert_page<'a>(ctx: &'a RequestCtx, page: StaticPage) -> StaticPage;
        list_plugins<'a>(ctx: &'a RequestCtx) -> Vec<CataloguePlugin>;
        upsert_plugin<'a>(ctx: &'a RequestCtx, plugin: CataloguePlugin) -> CataloguePlugin;
        save_plugin<'a>(ctx: &'a RequestCtx, plugin: CataloguePlugin, activity: ActivityRow) -> CataloguePlugin;
        odata_table<'a>(ctx: &'a RequestCtx, set_name: &'a str) -> Arc<dyn odata_table::Table>;
    }
}
