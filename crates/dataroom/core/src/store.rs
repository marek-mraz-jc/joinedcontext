#![forbid(unsafe_code)]
//! Storage trait implemented by both `dataroom-store-memory` and `dataroom-store-pg` (C1, C14).

use crate::auth::RequestCtx;
use crate::error::Result;
use crate::model::{
    ActivityRow, Catalogue, CatalogueDeclaration, CatalogueId, CataloguePlugin, Dataset,
    Distribution, HarvestRun, HarvestSource, OrgId, ProfileRecord, Publisher, ResourceView,
    ShaclValidationReport, StaticPage, Visibility,
};
use jc_time::TimestampTz;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The `modified` a guarded write stores: the proposed one, moved one microsecond past
/// `expected` when it is not later, so every guarded write changes the version the next writer
/// compares (Postgres keeps microseconds).
#[must_use]
pub fn next_modified(proposed: TimestampTz, expected: Option<TimestampTz>) -> TimestampTz {
    match expected {
        Some(prev) if proposed <= prev => prev
            .checked_add(jiff::SignedDuration::from_micros(1))
            .unwrap_or(proposed),
        _ => proposed,
    }
}

/// The conflict a guarded write answers when another write changed the record since it was read.
#[must_use]
pub fn changed_since_read(kind: &str, id: &str) -> crate::error::DataroomError {
    crate::error::DataroomError::Conflict(format!(
        "{kind} '{id}' was changed by another write since it was read: read it again and retry"
    ))
}

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

impl SearchQuery {
    /// Whether the state and public-only part of the query keeps `ds` (the terms aside).
    #[must_use]
    pub fn keeps(&self, ds: &Dataset) -> bool {
        let state_kept = match ds.state {
            crate::model::RecordState::Active => true,
            crate::model::RecordState::Draft => self.include_drafts,
            crate::model::RecordState::Withdrawn => self.include_withdrawn,
        };
        state_kept && (!self.public_only || ds.visibility == Visibility::Public)
    }
}

impl DatasetFilter {
    pub fn public_default() -> Self {
        Self {
            limit: 20,
            ..Self::default()
        }
    }
}

/// The text and state part of a catalogue search a store answers (S11-38): the store narrows
/// and ranks, the search crate authorizes, facets and pages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// Per query word, the folded prefixes any of which matches it (`word_stem_candidates`);
    /// a dataset matches when every word does. Empty: every dataset matches.
    pub terms: Vec<Vec<String>>,
    pub include_drafts: bool,
    pub include_withdrawn: bool,
    /// Only `public` datasets: an anonymous search without `include_private`.
    pub public_only: bool,
}

/// One dataset a store found for a [`SearchQuery`].
#[derive(Debug, Clone, PartialEq)]
pub struct SearchCandidate {
    /// With `rank` set, a slim record: no description, distributions holding only id and
    /// format. With `rank` unset, the full record, which the caller still has to match.
    pub dataset: Dataset,
    pub rank: Option<usize>,
}

/// The answer of [`Store::search_page`]: the catalogue's publishers and the full records of
/// the page `pick` chose, in its order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchPage {
    pub publishers: Vec<Publisher>,
    pub datasets: Vec<Dataset>,
}

/// Chooses a page from the candidates of a [`Store::search_page`] and returns its dataset ids.
pub type PickPage<'a> =
    dyn FnMut(&[Publisher], Vec<SearchCandidate>) -> Result<Vec<String>> + Send + 'a;

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

    /// [`Store::save_dataset`] for a catalogue row: the write and its activity row in one
    /// transaction, refused with a conflict when the stored `modified` is no longer `expected`.
    fn save_catalogue<'a>(
        &'a self,
        catalogue: Catalogue,
        expected: Option<TimestampTz>,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<Catalogue>>;

    /// Writes only the declared members of a catalogue, so a concurrent edit of another one
    /// (hidden datasets, branding) is never lost; `None` when there is no such catalogue.
    fn declare_catalogue<'a>(
        &'a self,
        org_id: OrgId,
        id: CatalogueId,
        declared: &'a CatalogueDeclaration,
    ) -> BoxFuture<'a, Result<Option<Catalogue>>> {
        Box::pin(async move {
            let Some(mut held) = self.get_catalogue(org_id, id).await? else {
                return Ok(None);
            };
            if !declared.apply(&mut held) {
                return Ok(Some(held));
            }
            held.modified = jc_time::TimestampTz::now();
            self.upsert_catalogue(held).await.map(Some)
        })
    }

    /// Deletes every row of the catalogue (datasets, publishers, sources, runs, pages, ...)
    /// and keeps the catalogue row as a tombstone with [`Catalogue::deleted`] set (ODP-09).
    /// `false` when there is no such catalogue.
    fn delete_catalogue<'a>(
        &'a self,
        org_id: OrgId,
        catalogue_id: CatalogueId,
    ) -> BoxFuture<'a, Result<bool>>;

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

    /// [`Store::save_dataset`] for a publisher: the write and its activity row in one
    /// transaction, guarded on the stored `modified` when `expected` is given (S11-13, S11-25).
    fn save_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        publisher: Publisher,
        expected: Option<TimestampTz>,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<Publisher>>;

    /// Merges publisher `source_id` into `target_id` (moves datasets, sets `merged_into`, C16),
    /// with `activity` in the same transaction; an activity row with no `object_id` names the
    /// target.
    fn merge_publishers<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        source_id_or_name: &'a str,
        target_id_or_name: &'a str,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<Publisher>>;

    /// Withdraws a publisher (its datasets leave public feeds, C16), read and written in one
    /// transaction with `activity`.
    fn withdraw_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<Publisher>>;

    /// Lists all datasets in `ctx`'s catalogue (including drafts/withdrawn for internal callers).
    fn list_datasets<'a>(&'a self, ctx: &'a RequestCtx) -> BoxFuture<'a, Result<Vec<Dataset>>>;

    /// One search page in one read (S11-38): the publishers, the datasets `query` keeps
    /// (state, public-only, every term matched), `pick` choosing the page from them, and the
    /// page's full records. This default reads whole records and leaves the match to the
    /// caller (every rank unset); the Postgres store matches and ranks in SQL.
    fn search_page<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        query: &'a SearchQuery,
        pick: &'a mut PickPage<'_>,
    ) -> BoxFuture<'a, Result<SearchPage>> {
        Box::pin(async move {
            let publishers = self.list_publishers(ctx).await?;
            let candidates: Vec<SearchCandidate> = self
                .list_datasets(ctx)
                .await?
                .into_iter()
                .filter(|ds| query.keeps(ds))
                .map(|dataset| SearchCandidate {
                    dataset,
                    rank: None,
                })
                .collect();
            let mut by_id: std::collections::HashMap<String, Dataset> = candidates
                .iter()
                .map(|c| (c.dataset.id.clone(), c.dataset.clone()))
                .collect();
            let ids = pick(&publishers, candidates)?;
            let datasets = ids.iter().filter_map(|id| by_id.remove(id)).collect();
            Ok(SearchPage {
                publishers,
                datasets,
            })
        })
    }

    /// The datasets of one `dataset_type` (`dashboard`), in name order.
    fn list_datasets_of_type<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        dataset_type: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Dataset>>> {
        Box::pin(async move {
            let mut all = self.list_datasets(ctx).await?;
            all.retain(|d| d.dataset_type == dataset_type);
            Ok(all)
        })
    }

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

    /// Writes `dataset` and its activity row in one transaction (AUD-02): the row commits with
    /// the change or neither does. With `expected`, the stored record's `modified` must still be
    /// the one the caller read, else nothing is written and the answer is a conflict (S11-25).
    /// The saved `modified` always moves past `expected` ([`next_modified`]), and an activity
    /// row with no `object_id` names the saved dataset's id.
    fn save_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        dataset: Dataset,
        expected: Option<TimestampTz>,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<Dataset>>;

    /// Marks a dataset as withdrawn (`state = Withdrawn`).
    fn withdraw_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Dataset>>;

    /// Permanently purges a dataset (`dataset_purge`), with `activity` in the same transaction.
    fn purge_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
        activity: ActivityRow,
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

    /// The views of several distributions (one dataset's) in one read: grouped in the order of
    /// `distribution_ids`, each group as [`Store::list_resource_views`] orders it.
    fn list_views_of<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        distribution_ids: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<ResourceView>>> {
        Box::pin(async move {
            let mut out = Vec::new();
            for id in distribution_ids {
                out.extend(self.list_resource_views(ctx, id).await?);
            }
            Ok(out)
        })
    }

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

    /// Writes a resource view and its activity row in one transaction (S11-13); an activity row
    /// with no `object_id` names the saved view.
    fn save_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        view: ResourceView,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<ResourceView>>;

    /// Deletes a resource view by `id`, with `activity` in the same transaction.
    fn delete_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        view_id: &'a str,
        activity: ActivityRow,
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

    /// Writes a harvest source and its activity row in one transaction (S11-13).
    fn save_harvest_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        source: HarvestSource,
        activity: ActivityRow,
    ) -> BoxFuture<'a, Result<HarvestSource>>;

    /// Deletes the harvest source `id_or_name` of `ctx`'s catalogue and its runs; `false` when
    /// there is none. The datasets it harvested stay until the catalogue's next run decides.
    fn delete_harvest_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<bool>>;

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
        _stale_before: jc_time::TimestampTz,
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

    /// Stores a new profile version and its activity row in one transaction (S11-13). A
    /// version is never rewritten: a stored `(id, version)` is a conflict and nothing is written.
    fn save_profile<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        profile: ProfileRecord,
        activity: ActivityRow,
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

    /// Writes a plugin and its activity row in one transaction (S11-13).
    fn save_plugin<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        plugin: CataloguePlugin,
        activity: ActivityRow,
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
