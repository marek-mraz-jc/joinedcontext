#![forbid(unsafe_code)]
//! Storage trait implemented by both `dataroom-store-memory` and `dataroom-store-pg` (C1, C14).

use crate::auth::RequestCtx;
use crate::error::Result;
use crate::model::{
    ActivityRow, Catalogue, CatalogueId, CataloguePlugin, Dataset, Distribution, HarvestRun,
    HarvestSource, OrgId, ProfileRecord, Publisher, ResourceView, ShaclValidationReport,
    StaticPage, Visibility,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Structured filter for dataset queries (compatible with CKAN `fq` and search facets).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatasetFilter {
    pub q: String,
    pub publishers: Vec<String>,
    pub themes: Vec<String>,
    pub formats: Vec<String>,
    pub licences: Vec<String>,
    pub tags: Vec<String>,
    pub spatial: Vec<String>,
    pub years: Vec<i32>,
    pub visibilities: Vec<Visibility>,
    pub include_private: bool,
    pub include_drafts: bool,
    pub include_withdrawn: bool,
    pub harvest_source: Option<String>,
    pub sort: Option<String>,
    pub offset: usize,
    pub limit: usize,
}

impl DatasetFilter {
    pub fn public_default() -> Self {
        Self {
            limit: 20,
            ..Self::default()
        }
    }
}

/// The storage contract for `dataroom`.
///
/// Every method takes a [`RequestCtx`] carrying the verified `(org_id, catalogue_id)`,
/// ensuring tenant and catalogue isolation at the store boundary.
pub trait Store: Send + Sync {
    /// Resolves a catalogue by its unique host name (`cat_catalogue.host`).
    fn resolve_host<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<Option<Catalogue>>>;

    /// Lists all catalogues belonging to `org_id`.
    fn list_catalogues<'a>(&'a self, org_id: OrgId) -> BoxFuture<'a, Result<Vec<Catalogue>>>;

    /// Gets a catalogue by `(org_id, catalogue_id)`.
    fn get_catalogue<'a>(
        &'a self,
        org_id: OrgId,
        catalogue_id: CatalogueId,
    ) -> BoxFuture<'a, Result<Option<Catalogue>>>;

    /// Creates or updates a catalogue.
    fn upsert_catalogue<'a>(&'a self, catalogue: Catalogue) -> BoxFuture<'a, Result<Catalogue>>;

    /// Lists publishers in `ctx`'s catalogue.
    fn list_publishers<'a>(&'a self, ctx: &'a RequestCtx) -> BoxFuture<'a, Result<Vec<Publisher>>>;

    /// Gets a publisher by `id` or `name` slug, following `merged_into` redirects when `follow_merge` is true.
    fn get_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<Publisher>>>;

    /// Creates or updates a publisher.
    fn upsert_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        publisher: Publisher,
    ) -> BoxFuture<'a, Result<Publisher>>;

    /// Merges publisher `source_id` into `target_id` (moves datasets, sets `merged_into`, C16).
    fn merge_publishers<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        source_id_or_name: &'a str,
        target_id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Publisher>>;

    /// Withdraws a publisher (its datasets leave public feeds, C16).
    fn withdraw_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Publisher>>;

    /// Lists all datasets in `ctx`'s catalogue (including drafts/withdrawn for internal callers).
    fn list_datasets<'a>(&'a self, ctx: &'a RequestCtx) -> BoxFuture<'a, Result<Vec<Dataset>>>;

    /// Gets a dataset by `id` or `name` slug.
    fn get_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<Dataset>>>;

    /// Creates or updates a dataset and its distributions.
    fn upsert_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        dataset: Dataset,
    ) -> BoxFuture<'a, Result<Dataset>>;

    /// Marks a dataset as withdrawn (`state = Withdrawn`).
    fn withdraw_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Dataset>>;

    /// Permanently purges a dataset (`dataset_purge`).
    fn purge_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<()>>;

    /// Gets a distribution and its parent dataset by distribution `id`.
    fn get_distribution<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        distribution_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<(Dataset, Distribution)>>>;

    /// Lists resource views for a distribution.
    fn list_resource_views<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        resource_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<ResourceView>>>;

    /// Gets a resource view by `id`.
    fn get_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        view_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ResourceView>>>;

    /// Creates or updates a resource view.
    fn upsert_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        view: ResourceView,
    ) -> BoxFuture<'a, Result<ResourceView>>;

    /// Deletes a resource view by `id`.
    fn delete_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        view_id: &'a str,
    ) -> BoxFuture<'a, Result<()>>;

    /// Lists harvest sources in `ctx`'s catalogue.
    fn list_harvest_sources<'a>(
        &'a self,
        ctx: &'a RequestCtx,
    ) -> BoxFuture<'a, Result<Vec<HarvestSource>>>;

    /// Gets a harvest source by `id` or `name`.
    fn get_harvest_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<HarvestSource>>>;

    /// Creates or updates a harvest source.
    fn upsert_harvest_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        source: HarvestSource,
    ) -> BoxFuture<'a, Result<HarvestSource>>;

    /// Lists harvest runs in `ctx`'s catalogue.
    fn list_harvest_runs<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        source_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<HarvestRun>>>;

    /// Gets a harvest run by `id`.
    fn get_harvest_run<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        run_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<HarvestRun>>>;

    /// Creates or updates a harvest run.
    fn upsert_harvest_run<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        run: HarvestRun,
    ) -> BoxFuture<'a, Result<HarvestRun>>;

    /// Starts `run` (state Running) unless another run of its source is still running: the
    /// per-source lock of scheduled and manual harvests. A Running run started before
    /// `stale_before` is one a stopped process left behind: it is marked failed first.
    /// `false`: another run holds the source and nothing was written.
    fn claim_harvest_run<'a>(
        &'a self,
        _ctx: &'a RequestCtx,
        _run: HarvestRun,
        _stale_before: chrono::DateTime<chrono::Utc>,
    ) -> BoxFuture<'a, Result<bool>> {
        Box::pin(std::future::ready(Err(
            crate::error::DataroomError::NotSupported("this store holds no harvest lock".into()),
        )))
    }

    /// Appends an activity audit row (`C13`).
    fn record_activity<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        row: ActivityRow,
    ) -> BoxFuture<'a, Result<()>>;

    /// Lists activity audit rows for `ctx`'s catalogue.
    fn list_activity<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        object_id: Option<&'a str>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ActivityRow>>>;

    /// Stores a SHACL validation report for a `(dataset, profile, version)` (`C8`, `C17`).
    fn upsert_shacl_report<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        report: ShaclValidationReport,
    ) -> BoxFuture<'a, Result<ShaclValidationReport>>;

    /// Lists stored SHACL validation reports for a dataset or the whole catalogue.
    fn list_shacl_reports<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        dataset_id: Option<&'a str>,
        profile_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<ShaclValidationReport>>>;

    /// Lists stored custom/uploaded profiles (`C17`).
    fn list_profiles<'a>(
        &'a self,
        ctx: &'a RequestCtx,
    ) -> BoxFuture<'a, Result<Vec<ProfileRecord>>>;

    /// Saves a versioned profile (`C17`).
    fn upsert_profile<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        profile: ProfileRecord,
    ) -> BoxFuture<'a, Result<ProfileRecord>>;

    /// Lists static pages for `ctx`'s catalogue.
    fn list_pages<'a>(&'a self, ctx: &'a RequestCtx) -> BoxFuture<'a, Result<Vec<StaticPage>>>;

    /// Upserts a static page.
    fn upsert_page<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        page: StaticPage,
    ) -> BoxFuture<'a, Result<StaticPage>>;

    /// Lists plugins for `ctx`'s catalogue.
    fn list_plugins<'a>(
        &'a self,
        ctx: &'a RequestCtx,
    ) -> BoxFuture<'a, Result<Vec<CataloguePlugin>>>;

    /// Upserts a plugin for `ctx`'s catalogue.
    fn upsert_plugin<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        plugin: CataloguePlugin,
    ) -> BoxFuture<'a, Result<CataloguePlugin>>;

    /// Returns an `odata_table::Table` for the named entity set (`Catalogues`, `Datasets`,
    /// `Distributions`, `Publishers`, `HarvestSources`, `HarvestRuns`, `ActivityRows`,
    /// `ShaclResults`) scoped to `ctx` (C14).
    fn odata_table<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        set_name: &'a str,
    ) -> BoxFuture<'a, Result<Arc<dyn odata_table::Table>>>;
}
