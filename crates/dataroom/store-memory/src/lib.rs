#![forbid(unsafe_code)]
//! In-memory implementation of [`dataroom_core::Store`] (C1, C14, C16).

use chrono::Utc;
use dataroom_core::odata_defs::{
    activity_to_row, catalogue_to_row, dataroom_service, dataset_to_row, distribution_to_row,
    harvest_run_to_row, harvest_source_to_row, publisher_to_row, shacl_report_to_row,
};
use dataroom_core::store::BoxFuture;
use dataroom_core::{
    abandon_run, ActivityRow, Catalogue, CatalogueId, CataloguePlugin, DataroomError, Dataset,
    Distribution, HarvestRun, HarvestSource, OrgId, ProfileRecord, Publisher, RecordState,
    RequestCtx, ResourceView, Result, ShaclValidationReport, StaticPage, Store,
};
use odata_table::{MemoryTable, Table};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

#[derive(Default)]
struct Inner {
    catalogues: BTreeMap<(OrgId, CatalogueId), Catalogue>,
    publishers: BTreeMap<(OrgId, CatalogueId, String), Publisher>,
    datasets: BTreeMap<(OrgId, CatalogueId, String), Dataset>,
    views: BTreeMap<(OrgId, CatalogueId, String), ResourceView>,
    harvest_sources: BTreeMap<(OrgId, CatalogueId, String), HarvestSource>,
    harvest_runs: BTreeMap<(OrgId, CatalogueId, String), HarvestRun>,
    activity: Vec<ActivityRow>,
    shacl_reports: BTreeMap<(OrgId, CatalogueId, String), ShaclValidationReport>,
    profiles: BTreeMap<(OrgId, CatalogueId, String, String), ProfileRecord>,
    pages: BTreeMap<(OrgId, CatalogueId, String, String), StaticPage>,
    plugins: BTreeMap<(OrgId, CatalogueId, String), CataloguePlugin>,
}

/// Thread-safe in-memory [`Store`] implementation.
#[derive(Clone, Default)]
pub struct MemoryStore {
    inner: Arc<RwLock<Inner>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn read_inner(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write_inner(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// A deleted catalogue's rows stay gone: a harvest run still going when its catalogue was
/// deleted cannot write them back (the tombstone outlives a re-create only until it clears).
fn refuse_deleted(inner: &Inner, ctx: &RequestCtx) -> Result<()> {
    match inner.catalogues.get(&(ctx.org_id, ctx.catalogue_id)) {
        Some(c) if c.deleted.is_some() => Err(DataroomError::Gone(format!(
            "catalogue {} was deleted",
            ctx.catalogue_id
        ))),
        _ => Ok(()),
    }
}

impl Store for MemoryStore {
    fn resolve_host<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<Option<Catalogue>>> {
        let clean = host
            .split(':')
            .next()
            .unwrap_or(host)
            .trim()
            .to_ascii_lowercase();
        let found = self
            .read_inner()
            .catalogues
            .values()
            .find(|c| {
                let ch = c
                    .host
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .split(':')
                    .next()
                    .unwrap_or(&c.host);
                ch.eq_ignore_ascii_case(&clean) || c.host.eq_ignore_ascii_case(host)
            })
            .cloned();
        Box::pin(std::future::ready(Ok(found)))
    }

    fn list_catalogues<'a>(&'a self, org_id: OrgId) -> BoxFuture<'a, Result<Vec<Catalogue>>> {
        let list = self
            .read_inner()
            .catalogues
            .iter()
            .filter(|((oid, _), _)| *oid == org_id)
            .map(|(_, c)| c.clone())
            .collect();
        Box::pin(std::future::ready(Ok(list)))
    }

    fn get_catalogue<'a>(
        &'a self,
        org_id: OrgId,
        catalogue_id: CatalogueId,
    ) -> BoxFuture<'a, Result<Option<Catalogue>>> {
        let found = self
            .read_inner()
            .catalogues
            .get(&(org_id, catalogue_id))
            .cloned();
        Box::pin(std::future::ready(Ok(found)))
    }

    fn delete_catalogue<'a>(
        &'a self,
        org_id: OrgId,
        catalogue_id: CatalogueId,
    ) -> BoxFuture<'a, Result<bool>> {
        let mut inner = self.write_inner();
        let Some(cat) = inner.catalogues.get_mut(&(org_id, catalogue_id)) else {
            return Box::pin(std::future::ready(Ok(false)));
        };
        cat.deleted = Some(chrono::Utc::now());
        cat.modified = chrono::Utc::now();
        let mine = |o: &OrgId, c: &CatalogueId| *o == org_id && *c == catalogue_id;
        inner.publishers.retain(|(o, c, _), _| !mine(o, c));
        inner.datasets.retain(|(o, c, _), _| !mine(o, c));
        inner.views.retain(|(o, c, _), _| !mine(o, c));
        inner.harvest_sources.retain(|(o, c, _), _| !mine(o, c));
        inner.harvest_runs.retain(|(o, c, _), _| !mine(o, c));
        inner.activity.retain(|a| !mine(&a.org_id, &a.catalogue_id));
        inner.shacl_reports.retain(|(o, c, _), _| !mine(o, c));
        inner.profiles.retain(|(o, c, _, _), _| !mine(o, c));
        inner.pages.retain(|(o, c, _, _), _| !mine(o, c));
        inner.plugins.retain(|(o, c, _), _| !mine(o, c));
        Box::pin(std::future::ready(Ok(true)))
    }

    fn upsert_catalogue<'a>(&'a self, catalogue: Catalogue) -> BoxFuture<'a, Result<Catalogue>> {
        let mut guard = self.write_inner();
        let host_conflict = guard.catalogues.values().any(|c| {
            c.host.eq_ignore_ascii_case(&catalogue.host)
                && (c.org_id != catalogue.org_id || c.id != catalogue.id)
        });
        if host_conflict {
            return Box::pin(std::future::ready(Err(DataroomError::Conflict(format!(
                "host '{}' is already claimed by another catalogue",
                catalogue.host
            )))));
        }
        if guard
            .catalogues
            .keys()
            .any(|&(o, c)| c == catalogue.id && o != catalogue.org_id)
        {
            return Box::pin(std::future::ready(Err(DataroomError::Conflict(format!(
                "catalogue {} belongs to another organisation",
                catalogue.id
            )))));
        }
        guard
            .catalogues
            .insert((catalogue.org_id, catalogue.id), catalogue.clone());
        Box::pin(std::future::ready(Ok(catalogue)))
    }

    fn list_publishers<'a>(&'a self, ctx: &'a RequestCtx) -> BoxFuture<'a, Result<Vec<Publisher>>> {
        let mut list: Vec<Publisher> = self
            .read_inner()
            .publishers
            .iter()
            .filter(|((oid, cid, _), _)| *oid == ctx.org_id && *cid == ctx.catalogue_id)
            .map(|(_, p)| p.clone())
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Box::pin(std::future::ready(Ok(list)))
    }

    fn get_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<Publisher>>> {
        let guard = self.read_inner();
        let mut current = guard
            .publishers
            .iter()
            .find(|((oid, cid, _), p)| {
                *oid == ctx.org_id
                    && *cid == ctx.catalogue_id
                    && (p.id == id_or_name || p.name == id_or_name || p.uri == id_or_name)
            })
            .map(|(_, p)| p.clone());

        // Follow publisher merge redirect (up to 4 hops, C16).
        for _ in 0..4 {
            let Some(ref pub_rec) = current else {
                break;
            };
            let Some(ref target_id) = pub_rec.merged_into else {
                break;
            };
            let next = guard
                .publishers
                .iter()
                .find(|((oid, cid, _), p)| {
                    *oid == ctx.org_id
                        && *cid == ctx.catalogue_id
                        && (p.id == *target_id || p.name == *target_id)
                })
                .map(|(_, p)| p.clone());
            if next.is_some() {
                current = next;
            } else {
                break;
            }
        }
        Box::pin(std::future::ready(Ok(current)))
    }

    fn upsert_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut publisher: Publisher,
    ) -> BoxFuture<'a, Result<Publisher>> {
        publisher.org_id = ctx.org_id;
        publisher.catalogue_id = ctx.catalogue_id;
        if publisher.id.is_empty() {
            publisher.id = format!("pub-{}", publisher.name);
        }
        let mut guard = self.write_inner();
        let name_conflict = guard.publishers.iter().any(|((oid, cid, _), p)| {
            *oid == ctx.org_id
                && *cid == ctx.catalogue_id
                && p.name == publisher.name
                && p.id != publisher.id
        });
        if name_conflict {
            return Box::pin(std::future::ready(Err(DataroomError::Conflict(format!(
                "publisher name '{}' already exists in this catalogue",
                publisher.name
            )))));
        }
        guard.publishers.insert(
            (ctx.org_id, ctx.catalogue_id, publisher.id.clone()),
            publisher.clone(),
        );
        Box::pin(std::future::ready(Ok(publisher)))
    }

    fn merge_publishers<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        source_id_or_name: &'a str,
        target_id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Publisher>> {
        let mut guard = self.write_inner();
        let source = guard
            .publishers
            .values()
            .find(|p| {
                p.org_id == ctx.org_id
                    && p.catalogue_id == ctx.catalogue_id
                    && (p.id == source_id_or_name || p.name == source_id_or_name)
            })
            .cloned();
        let target = guard
            .publishers
            .values()
            .find(|p| {
                p.org_id == ctx.org_id
                    && p.catalogue_id == ctx.catalogue_id
                    && (p.id == target_id_or_name || p.name == target_id_or_name)
            })
            .cloned();

        let (Some(mut source), Some(target)) = (source, target) else {
            return Box::pin(std::future::ready(Err(DataroomError::NotFound(
                "source or target publisher not found".into(),
            ))));
        };
        if source.id == target.id {
            return Box::pin(std::future::ready(Err(DataroomError::field_error(
                "target",
                "cannot merge a publisher into itself",
            ))));
        }

        // Move datasets from source to target publisher (C16).
        for ((oid, cid, _), ds) in guard.datasets.iter_mut() {
            if *oid == ctx.org_id
                && *cid == ctx.catalogue_id
                && (ds.publisher_id == source.id || ds.publisher_id == source.name)
            {
                ds.publisher_id = target.name.clone();
                ds.modified = Utc::now();
            }
        }

        source.merged_into = Some(target.name.clone());
        source.state = RecordState::Withdrawn;
        source.modified = Utc::now();
        guard
            .publishers
            .insert((ctx.org_id, ctx.catalogue_id, source.id.clone()), source);

        Box::pin(std::future::ready(Ok(target)))
    }

    fn withdraw_publisher<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Publisher>> {
        let mut guard = self.write_inner();
        let found = guard.publishers.values_mut().find(|p| {
            p.org_id == ctx.org_id
                && p.catalogue_id == ctx.catalogue_id
                && (p.id == id_or_name || p.name == id_or_name)
        });
        let Some(p) = found else {
            return Box::pin(std::future::ready(Err(DataroomError::NotFound(format!(
                "publisher '{id_or_name}' not found"
            )))));
        };
        p.state = RecordState::Withdrawn;
        p.modified = Utc::now();
        let out = p.clone();
        Box::pin(std::future::ready(Ok(out)))
    }

    fn list_datasets<'a>(&'a self, ctx: &'a RequestCtx) -> BoxFuture<'a, Result<Vec<Dataset>>> {
        let mut list: Vec<Dataset> = self
            .read_inner()
            .datasets
            .iter()
            .filter(|((oid, cid, _), _)| *oid == ctx.org_id && *cid == ctx.catalogue_id)
            .map(|(_, d)| d.clone())
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Box::pin(std::future::ready(Ok(list)))
    }

    fn get_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<Dataset>>> {
        let found = self
            .read_inner()
            .datasets
            .iter()
            .find(|((oid, cid, _), d)| {
                *oid == ctx.org_id
                    && *cid == ctx.catalogue_id
                    && (d.id == id_or_name || d.name == id_or_name)
            })
            .map(|(_, d)| d.clone());
        Box::pin(std::future::ready(Ok(found)))
    }

    fn upsert_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut dataset: Dataset,
    ) -> BoxFuture<'a, Result<Dataset>> {
        dataset.org_id = ctx.org_id;
        dataset.catalogue_id = ctx.catalogue_id;
        if dataset.id.is_empty() {
            dataset.id = format!("pkg-{}", dataset.name);
        }
        for (i, dist) in dataset.distributions.iter_mut().enumerate() {
            dist.dataset_id = dataset.id.clone();
            if dist.id.is_empty() {
                dist.id = format!("res-{}-{i}", dataset.name);
            }
        }
        let mut guard = self.write_inner();
        if let Err(e) = refuse_deleted(&guard, ctx) {
            return Box::pin(std::future::ready(Err(e)));
        }
        let name_conflict = guard.datasets.iter().any(|((oid, cid, _), d)| {
            *oid == ctx.org_id
                && *cid == ctx.catalogue_id
                && d.name == dataset.name
                && d.id != dataset.id
        });
        if name_conflict {
            return Box::pin(std::future::ready(Err(DataroomError::field_error(
                "name",
                "That URL is already in use.",
            ))));
        }
        guard.datasets.insert(
            (ctx.org_id, ctx.catalogue_id, dataset.id.clone()),
            dataset.clone(),
        );
        Box::pin(std::future::ready(Ok(dataset)))
    }

    fn withdraw_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Dataset>> {
        let mut guard = self.write_inner();
        let found = guard.datasets.values_mut().find(|d| {
            d.org_id == ctx.org_id
                && d.catalogue_id == ctx.catalogue_id
                && (d.id == id_or_name || d.name == id_or_name)
        });
        let Some(d) = found else {
            return Box::pin(std::future::ready(Err(DataroomError::NotFound(format!(
                "dataset '{id_or_name}' not found"
            )))));
        };
        d.state = RecordState::Withdrawn;
        d.modified = Utc::now();
        let out = d.clone();
        Box::pin(std::future::ready(Ok(out)))
    }

    fn purge_dataset<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        let mut guard = self.write_inner();
        let key = guard
            .datasets
            .iter()
            .find(|((oid, cid, _), d)| {
                *oid == ctx.org_id
                    && *cid == ctx.catalogue_id
                    && (d.id == id_or_name || d.name == id_or_name)
            })
            .map(|(k, _)| k.clone());
        let Some(key) = key else {
            return Box::pin(std::future::ready(Err(DataroomError::NotFound(format!(
                "dataset '{id_or_name}' not found"
            )))));
        };
        if let Some(removed) = guard.datasets.remove(&key) {
            let res_ids: Vec<String> = removed.distributions.iter().map(|r| r.id.clone()).collect();
            guard.views.retain(|(oid, cid, _), v| {
                !(*oid == ctx.org_id
                    && *cid == ctx.catalogue_id
                    && res_ids.contains(&v.resource_id))
            });
        }
        Box::pin(std::future::ready(Ok(())))
    }

    fn get_distribution<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        distribution_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<(Dataset, Distribution)>>> {
        let guard = self.read_inner();
        let found = guard
            .datasets
            .iter()
            .filter(|((oid, cid, _), _)| *oid == ctx.org_id && *cid == ctx.catalogue_id)
            .find_map(|(_, ds)| {
                let dist = ds.distributions.iter().find(|r| r.id == distribution_id)?;
                Some((ds.clone(), dist.clone()))
            });
        Box::pin(std::future::ready(Ok(found)))
    }

    fn list_resource_views<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        resource_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<ResourceView>>> {
        let mut list: Vec<ResourceView> = self
            .read_inner()
            .views
            .iter()
            .filter(|((oid, cid, _), v)| {
                *oid == ctx.org_id && *cid == ctx.catalogue_id && v.resource_id == resource_id
            })
            .map(|(_, v)| v.clone())
            .collect();
        list.sort_by_key(|v| v.order);
        Box::pin(std::future::ready(Ok(list)))
    }

    fn get_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        view_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<ResourceView>>> {
        let found = self
            .read_inner()
            .views
            .get(&(ctx.org_id, ctx.catalogue_id, view_id.to_owned()))
            .cloned();
        Box::pin(std::future::ready(Ok(found)))
    }

    fn upsert_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut view: ResourceView,
    ) -> BoxFuture<'a, Result<ResourceView>> {
        if view.id.is_empty() {
            view.id = format!("view-{}", uuid::Uuid::now_v7());
        }
        self.write_inner().views.insert(
            (ctx.org_id, ctx.catalogue_id, view.id.clone()),
            view.clone(),
        );
        Box::pin(std::future::ready(Ok(view)))
    }

    fn delete_resource_view<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        view_id: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        let removed =
            self.write_inner()
                .views
                .remove(&(ctx.org_id, ctx.catalogue_id, view_id.to_owned()));
        if removed.is_none() {
            return Box::pin(std::future::ready(Err(DataroomError::NotFound(format!(
                "resource view '{view_id}' not found"
            )))));
        }
        Box::pin(std::future::ready(Ok(())))
    }

    fn list_harvest_sources<'a>(
        &'a self,
        ctx: &'a RequestCtx,
    ) -> BoxFuture<'a, Result<Vec<HarvestSource>>> {
        let list = self
            .read_inner()
            .harvest_sources
            .iter()
            .filter(|((oid, cid, _), _)| *oid == ctx.org_id && *cid == ctx.catalogue_id)
            .map(|(_, s)| s.clone())
            .collect();
        Box::pin(std::future::ready(Ok(list)))
    }

    fn get_harvest_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<HarvestSource>>> {
        let found = self
            .read_inner()
            .harvest_sources
            .iter()
            .find(|((oid, cid, _), s)| {
                *oid == ctx.org_id
                    && *cid == ctx.catalogue_id
                    && (s.id == id_or_name || s.name == id_or_name)
            })
            .map(|(_, s)| s.clone());
        Box::pin(std::future::ready(Ok(found)))
    }

    fn upsert_harvest_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut source: HarvestSource,
    ) -> BoxFuture<'a, Result<HarvestSource>> {
        source.org_id = ctx.org_id;
        source.catalogue_id = ctx.catalogue_id;
        if source.id.is_empty() {
            source.id = format!("hs-{}", source.name);
        }
        let mut inner = self.write_inner();
        if let Err(e) = refuse_deleted(&inner, ctx) {
            return Box::pin(std::future::ready(Err(e)));
        }
        inner.harvest_sources.insert(
            (ctx.org_id, ctx.catalogue_id, source.id.clone()),
            source.clone(),
        );
        Box::pin(std::future::ready(Ok(source)))
    }

    fn delete_harvest_source<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        id_or_name: &'a str,
    ) -> BoxFuture<'a, Result<bool>> {
        let mut inner = self.write_inner();
        let (org, cat) = (ctx.org_id, ctx.catalogue_id);
        let found = inner
            .harvest_sources
            .iter()
            .find(|((o, c, _), s)| {
                *o == org && *c == cat && (s.id == id_or_name || s.name == id_or_name)
            })
            .map(|(k, s)| (k.clone(), s.id.clone()));
        let Some((key, id)) = found else {
            return Box::pin(std::future::ready(Ok(false)));
        };
        inner.harvest_sources.remove(&key);
        inner
            .harvest_runs
            .retain(|(o, c, _), r| !(*o == org && *c == cat && r.source_id == id));
        Box::pin(std::future::ready(Ok(true)))
    }

    fn list_harvest_runs<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        source_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<HarvestRun>>> {
        let mut list: Vec<HarvestRun> = self
            .read_inner()
            .harvest_runs
            .iter()
            .filter(|((oid, cid, _), r)| {
                *oid == ctx.org_id
                    && *cid == ctx.catalogue_id
                    && source_id.is_none_or(|sid| r.source_id == sid)
            })
            .map(|(_, r)| r.clone())
            .collect();
        // Newest first, as the Postgres store orders them.
        list.sort_by_key(|r| std::cmp::Reverse(r.started_at));
        Box::pin(std::future::ready(Ok(list)))
    }

    fn get_harvest_run<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        run_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<HarvestRun>>> {
        let found = self
            .read_inner()
            .harvest_runs
            .get(&(ctx.org_id, ctx.catalogue_id, run_id.to_owned()))
            .cloned();
        Box::pin(std::future::ready(Ok(found)))
    }

    fn upsert_harvest_run<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut run: HarvestRun,
    ) -> BoxFuture<'a, Result<HarvestRun>> {
        run.org_id = ctx.org_id;
        run.catalogue_id = ctx.catalogue_id;
        let mut inner = self.write_inner();
        if let Err(e) = refuse_deleted(&inner, ctx) {
            return Box::pin(std::future::ready(Err(e)));
        }
        inner
            .harvest_runs
            .insert((ctx.org_id, ctx.catalogue_id, run.id.clone()), run.clone());
        Box::pin(std::future::ready(Ok(run)))
    }

    fn claim_harvest_run<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut run: HarvestRun,
        stale_before: chrono::DateTime<Utc>,
    ) -> BoxFuture<'a, Result<bool>> {
        run.org_id = ctx.org_id;
        run.catalogue_id = ctx.catalogue_id;
        let mut inner = self.write_inner();
        if let Err(e) = refuse_deleted(&inner, ctx) {
            return Box::pin(std::future::ready(Err(e)));
        }
        let mut held = false;
        for ((oid, cid, _), r) in inner.harvest_runs.iter_mut() {
            if *oid != ctx.org_id
                || *cid != ctx.catalogue_id
                || r.source_id != run.source_id
                || r.state != dataroom_core::HarvestRunState::Running
            {
                continue;
            }
            if r.started_at < stale_before {
                abandon_run(r);
            } else {
                held = true;
            }
        }
        if !held {
            inner
                .harvest_runs
                .insert((ctx.org_id, ctx.catalogue_id, run.id.clone()), run);
        }
        Box::pin(std::future::ready(Ok(!held)))
    }

    fn record_activity<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut row: ActivityRow,
    ) -> BoxFuture<'a, Result<()>> {
        row.org_id = ctx.org_id;
        row.catalogue_id = ctx.catalogue_id;
        self.write_inner().activity.push(row);
        Box::pin(std::future::ready(Ok(())))
    }

    fn list_activity<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        object_id: Option<&'a str>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ActivityRow>>> {
        let list: Vec<ActivityRow> = self
            .read_inner()
            .activity
            .iter()
            .rev()
            .filter(|a| {
                a.org_id == ctx.org_id
                    && a.catalogue_id == ctx.catalogue_id
                    && object_id.is_none_or(|oid| a.object_id == oid)
            })
            .take(limit)
            .cloned()
            .collect();
        Box::pin(std::future::ready(Ok(list)))
    }

    fn upsert_shacl_report<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut report: ShaclValidationReport,
    ) -> BoxFuture<'a, Result<ShaclValidationReport>> {
        report.org_id = ctx.org_id;
        report.catalogue_id = ctx.catalogue_id;
        if report.id.is_empty() {
            report.id = format!(
                "{}:{}:{}",
                report.dataset_id, report.profile_id, report.profile_version
            );
        }
        self.write_inner().shacl_reports.insert(
            (ctx.org_id, ctx.catalogue_id, report.id.clone()),
            report.clone(),
        );
        Box::pin(std::future::ready(Ok(report)))
    }

    fn list_shacl_reports<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        dataset_id: Option<&'a str>,
        profile_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<ShaclValidationReport>>> {
        let list = self
            .read_inner()
            .shacl_reports
            .iter()
            .filter(|((oid, cid, _), r)| {
                *oid == ctx.org_id
                    && *cid == ctx.catalogue_id
                    && dataset_id.is_none_or(|d| r.dataset_id == d)
                    && profile_id.is_none_or(|p| r.profile_id == p)
            })
            .map(|(_, r)| r.clone())
            .collect();
        Box::pin(std::future::ready(Ok(list)))
    }

    fn list_profiles<'a>(
        &'a self,
        ctx: &'a RequestCtx,
    ) -> BoxFuture<'a, Result<Vec<ProfileRecord>>> {
        let list = self
            .read_inner()
            .profiles
            .iter()
            .filter(|((oid, cid, _, _), _)| *oid == ctx.org_id && *cid == ctx.catalogue_id)
            .map(|(_, p)| p.clone())
            .collect();
        Box::pin(std::future::ready(Ok(list)))
    }

    fn upsert_profile<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        profile: ProfileRecord,
    ) -> BoxFuture<'a, Result<ProfileRecord>> {
        self.write_inner().profiles.insert(
            (
                ctx.org_id,
                ctx.catalogue_id,
                profile.id.clone(),
                profile.version.clone(),
            ),
            profile.clone(),
        );
        Box::pin(std::future::ready(Ok(profile)))
    }

    fn list_pages<'a>(&'a self, ctx: &'a RequestCtx) -> BoxFuture<'a, Result<Vec<StaticPage>>> {
        let list = self
            .read_inner()
            .pages
            .iter()
            .filter(|((oid, cid, _, _), _)| *oid == ctx.org_id && *cid == ctx.catalogue_id)
            .map(|(_, p)| p.clone())
            .collect();
        Box::pin(std::future::ready(Ok(list)))
    }

    fn upsert_page<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut page: StaticPage,
    ) -> BoxFuture<'a, Result<StaticPage>> {
        page.org_id = ctx.org_id;
        page.catalogue_id = ctx.catalogue_id;
        self.write_inner().pages.insert(
            (
                ctx.org_id,
                ctx.catalogue_id,
                page.slug.clone(),
                page.lang.clone(),
            ),
            page.clone(),
        );
        Box::pin(std::future::ready(Ok(page)))
    }

    fn list_plugins<'a>(
        &'a self,
        ctx: &'a RequestCtx,
    ) -> BoxFuture<'a, Result<Vec<CataloguePlugin>>> {
        let list = self
            .read_inner()
            .plugins
            .iter()
            .filter(|((oid, cid, _), _)| *oid == ctx.org_id && *cid == ctx.catalogue_id)
            .map(|(_, p)| p.clone())
            .collect();
        Box::pin(std::future::ready(Ok(list)))
    }

    fn upsert_plugin<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        mut plugin: CataloguePlugin,
    ) -> BoxFuture<'a, Result<CataloguePlugin>> {
        plugin.org_id = ctx.org_id;
        plugin.catalogue_id = ctx.catalogue_id;
        self.write_inner().plugins.insert(
            (ctx.org_id, ctx.catalogue_id, plugin.name.clone()),
            plugin.clone(),
        );
        Box::pin(std::future::ready(Ok(plugin)))
    }

    fn odata_table<'a>(
        &'a self,
        ctx: &'a RequestCtx,
        set_name: &'a str,
    ) -> BoxFuture<'a, Result<Arc<dyn Table>>> {
        let service = dataroom_service();
        let model = Arc::new(service.model());
        let Some(info) = service.info(model, set_name) else {
            return Box::pin(std::future::ready(Err(DataroomError::NotFound(format!(
                "entity set '{set_name}' not found"
            )))));
        };
        let guard = self.read_inner();
        let default_lang = guard
            .catalogues
            .get(&(ctx.org_id, ctx.catalogue_id))
            .map(|c| c.default_language.clone());
        let lang_ref = default_lang.as_deref();

        // Only the request's organization (and catalogue, where the row has one) reaches the
        // table, as forced RLS does in the Postgres store; `Scope` narrows further.
        let org = ctx.org_id.to_string();
        let cat = ctx.catalogue_id.to_string();
        let own = |row: &serde_json::Value| {
            row["org_id"].as_str() == Some(org.as_str())
                && (set_name == "Catalogues"
                    || row.get("catalogue_id").is_none()
                    || row["catalogue_id"].as_str() == Some(cat.as_str()))
        };
        let rows: Vec<serde_json::Value> = match set_name {
            "Catalogues" => guard.catalogues.values().map(catalogue_to_row).collect(),
            "Datasets" => guard
                .datasets
                .values()
                .map(|d| dataset_to_row(d, lang_ref))
                .collect(),
            "Distributions" => guard
                .datasets
                .values()
                .flat_map(|ds| {
                    ds.distributions.iter().map(move |dist| {
                        distribution_to_row(ds.org_id, ds.catalogue_id, dist, lang_ref)
                    })
                })
                .collect(),
            "Publishers" => guard
                .publishers
                .values()
                .map(|p| publisher_to_row(p, lang_ref))
                .collect(),
            "HarvestSources" => guard
                .harvest_sources
                .values()
                .map(harvest_source_to_row)
                .collect(),
            "HarvestRuns" => guard
                .harvest_runs
                .values()
                .map(harvest_run_to_row)
                .collect(),
            "ActivityRows" => guard.activity.iter().map(activity_to_row).collect(),
            "ShaclResults" => guard
                .shacl_reports
                .values()
                .map(shacl_report_to_row)
                .collect(),
            _ => Vec::new(),
        };
        let rows = rows.into_iter().filter(own).collect();

        let res = MemoryTable::new(info, rows)
            .map(|t| Arc::new(t) as Arc<dyn Table>)
            .map_err(DataroomError::from);
        Box::pin(std::future::ready(res))
    }
}
