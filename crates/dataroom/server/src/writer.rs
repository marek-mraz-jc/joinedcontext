//! The in-process write path for catalogues and harvest sources (ODP-11): the reconciler creates,
//! updates and deletes a Portal's catalogue while the dataroom serves, with no restart and no
//! network surface. The handle shares the serving state's tables, so a change is served from
//! the next request on; each catalogue gets a publish principal of its own.

use dataroom_api::{portal_publisher_on, DataroomState, PortalAddress};
use dataroom_core::auth::constant_time_eq;
use dataroom_core::files::{FileEntry, FileStore, FileUsage};
use dataroom_core::{
    ActivityRow, Catalogue, CatalogueId, DataroomError, Dataset, HarvestSource, LiveAuthorizer,
    OrgId, Publisher, RecordState, RequestCtx, Result, Subject,
};
use dataroom_harvest::harvest_principal;
use std::sync::{Arc, PoisonError, RwLock};

/// One catalogue as the reconciler wants it.
#[derive(Clone)]
pub struct CatalogueSpec {
    pub org_id: OrgId,
    pub id: CatalogueId,
    pub name: String,
    /// The catalogue's host; a path-mode Portal's synthetic `{name}.{org}.portal.invalid`.
    pub host: String,
    /// The canonical public URL every id and link of the catalogue names.
    pub base: Option<String>,
    /// The Organization whose address decides this catalogue (its `default` Portal).
    pub organization: Option<String>,
    /// The CKAN organisation the publish principal writes into, made when missing.
    pub publisher: Publisher,
    /// The publish principal's token; `None` leaves the catalogue without a publisher grant.
    pub publish_token: Option<String>,
    /// A further portal's routes (ODP-08); `None` for an Organization's `default`.
    pub portal: Option<PortalRoute>,
}

/// Where a further portal answers besides its catalogue's base (ODP-08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalRoute {
    /// The Organization's name, the `{org}` of the slug address.
    pub organization: String,
    /// The portal's `spec.path`, the `{path}` of `/{org}/opendata/-/{path}/`.
    pub path: String,
    /// Its own host, when it has one.
    pub host: Option<String>,
    /// With a host: the slug address answers `308` to the base (`slug: redirect`).
    pub redirect: bool,
}

impl std::fmt::Debug for CatalogueSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogueSpec")
            .field("org_id", &self.org_id)
            .field("id", &self.id)
            .field("name", &self.name)
            .field("host", &self.host)
            .field("portal", &self.portal)
            .field(
                "publish_token",
                &self.publish_token.as_ref().map(|_| "<set>"),
            )
            .finish_non_exhaustive()
    }
}

/// Writes catalogues into a serving dataroom (see the module docs).
#[derive(Clone)]
pub struct CatalogueWriter {
    state: DataroomState,
    authorizer: Arc<LiveAuthorizer>,
    harvested: Arc<RwLock<Vec<(OrgId, CatalogueId)>>>,
    /// One write at a time: a write reads the tables, awaits the store, then changes the
    /// tables, and two interleaved writes would act on what the other just replaced.
    serial: Arc<tokio::sync::Mutex<()>>,
}

impl CatalogueWriter {
    /// `state` is the serving state (its tables are shared, not copied), `authorizer` the one
    /// that state and the harvest scheduler check with, `harvested` the scheduler's list.
    pub fn new(
        state: DataroomState,
        authorizer: Arc<LiveAuthorizer>,
        harvested: Arc<RwLock<Vec<(OrgId, CatalogueId)>>>,
    ) -> Self {
        Self {
            state,
            authorizer,
            harvested,
            serial: Arc::default(),
        }
    }

    /// Creates or updates the catalogue of `spec`, its publisher, its grants and its routes.
    /// Refused: a catalogue id another Organization holds in the store (a deleted one's
    /// tombstone too), a token another catalogue holds.
    pub async fn upsert(&self, spec: CatalogueSpec) -> Result<Catalogue> {
        let _serial = self.serial.lock().await;
        let (org, id) = (spec.org_id, spec.id);
        let publish = spec
            .publish_token
            .as_deref()
            .map(|t| portal_publisher_on(t, org, id, &spec.publisher.name))
            .transpose()?;
        if let Some((binding, _)) = &publish {
            let taken = read(&self.state.tokens).iter().any(|b| {
                constant_time_eq(&b.token, &binding.token)
                    && b.subject.actor_id != binding.subject.actor_id
            });
            if taken {
                return Err(DataroomError::Conflict(format!(
                    "the publish token of catalogue {id} is another catalogue's"
                )));
            }
        }
        let store = self.state.store.as_ref();
        let held = store.get_catalogue(org, id).await?;
        let mut cat = crate::ensure_catalogue(store, org, id, &spec.name, &spec.host).await?;
        // A further portal's last base and the own hosts it left live in its row, so a restart
        // keeps them (ODP-08).
        let mut row = cat.clone();
        row.deleted = None;
        if spec.portal.is_some() {
            if let Some(left) = held
                .as_ref()
                .map(|h| h.host.as_str())
                .filter(|h| *h != spec.host && !h.ends_with(".portal.invalid"))
            {
                if !row.left_hosts.iter().any(|l| l == left) {
                    row.left_hosts.push(left.to_owned());
                }
            }
            row.left_hosts.retain(|l| *l != spec.host);
            row.portal_base = spec.base.clone();
        }
        let before = held
            .and_then(|h| h.portal_base)
            .or_else(|| read(&self.state.catalogue_bases).get(&id).cloned());
        if row.deleted != cat.deleted
            || row.left_hosts != cat.left_hosts
            || row.portal_base != cat.portal_base
        {
            row.modified = jc_time::TimestampTz::now();
            cat = store.upsert_catalogue(row).await?;
        }
        let ctx = RequestCtx::new(org, id, Subject::anonymous());
        if store
            .get_publisher(&ctx, &spec.publisher.name)
            .await?
            .is_none()
        {
            store.upsert_publisher(&ctx, spec.publisher.clone()).await?;
        }

        if let Some(old) = before
            .as_deref()
            .filter(|old| spec.base.as_deref() != Some(*old))
        {
            keep_dataset_iris(store, &ctx, old).await?;
        }

        let mut grants = vec![harvest_principal(org, id).1];
        let principal = principal_of(id);
        let mut tokens = write(&self.state.tokens);
        tokens.retain(|b| b.subject.actor_id.as_deref() != Some(principal.as_str()));
        if let Some((binding, grant)) = publish {
            tokens.push(binding);
            grants.push(grant);
        }
        drop(tokens);
        self.authorizer.replace_catalogue(org, id, grants);
        let mut bases = write(&self.state.catalogue_bases);
        match spec.base.clone() {
            Some(base) => bases.insert(id, base),
            None => bases.remove(&id),
        };
        drop(bases);
        match &spec.portal {
            Some(route) => self.state.portals.route(
                id,
                &route.organization,
                &route.path,
                route.host.as_deref(),
                &cat.left_hosts,
                PortalAddress {
                    org_id: org,
                    id,
                    redirect: spec
                        .base
                        .clone()
                        .filter(|_| route.host.is_some() && route.redirect),
                },
            ),
            None => self.state.portals.unroute(id),
        }
        if let Some(name) = spec.organization {
            write(&self.state.organization_catalogues).insert(name, (org, id));
        }
        let mut harvested = write(&self.harvested);
        if !harvested.contains(&(org, id)) {
            harvested.push((org, id));
        }
        Ok(cat)
    }

    /// Deletes the catalogue's datasets, publishers and sources (its addresses answer 410
    /// from now on) and takes its principal, grants and routes away. `false`: no such catalogue.
    pub async fn delete(&self, org: OrgId, id: CatalogueId) -> Result<bool> {
        let _serial = self.serial.lock().await;
        let deleted = self.state.store.delete_catalogue(org, id).await?;
        self.forget(org, id);
        Ok(deleted)
    }

    /// Stops serving the catalogue (its Organization was removed): its principal, grants,
    /// routes, base, Organization entry and harvest go, its datasets and publishers stay in the
    /// store, so nothing is lost and an import can bring them back.
    pub async fn withdraw(&self, org: OrgId, id: CatalogueId) {
        let _serial = self.serial.lock().await;
        self.forget(org, id);
    }

    fn forget(&self, org: OrgId, id: CatalogueId) {
        let principal = principal_of(id);
        write(&self.state.tokens)
            .retain(|b| b.subject.actor_id.as_deref() != Some(principal.as_str()));
        self.authorizer.replace_catalogue(org, id, Vec::new());
        write(&self.state.catalogue_bases).remove(&id);
        self.state.portals.unroute(id);
        write(&self.state.organization_catalogues).retain(|_, v| *v != (org, id));
        write(&self.harvested).retain(|v| *v != (org, id));
    }

    /// Creates or updates a harvest source of the catalogue, and `publisher`, the CKAN
    /// organisation its records land in, when it is missing; the scheduler runs the source on
    /// its schedule from the next minute on.
    pub async fn upsert_harvest_source(
        &self,
        org: OrgId,
        id: CatalogueId,
        source: HarvestSource,
        publisher: Option<Publisher>,
    ) -> Result<HarvestSource> {
        let _serial = self.serial.lock().await;
        let ctx = self.live(org, id)?;
        let store = self.state.store.as_ref();
        if let Some(publisher) = publisher {
            if store.get_publisher(&ctx, &publisher.name).await?.is_none() {
                store.upsert_publisher(&ctx, publisher).await?;
            }
        }
        store.upsert_harvest_source(&ctx, source).await
    }

    /// Runs operation `op` in catalogue `id` as that catalogue's own harvest principal (ODP-15):
    /// the Portal's sources routes test, start and list a source's runs with no token of
    /// their own, and no other catalogue's rows are in reach.
    pub async fn run_op(
        &self,
        org: OrgId,
        id: CatalogueId,
        op: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.live(org, id)?;
        let ctx = RequestCtx::new(org, id, harvest_principal(org, id).0);
        dataroom_core::ops::execute_operation(&self.state.op_context(), &ctx, op, input).await
    }

    /// The datasets of catalogue `id`, any state, by name: what a portal lists (ODP-15).
    pub async fn datasets(&self, org: OrgId, id: CatalogueId) -> Result<Vec<Dataset>> {
        let ctx = self.live(org, id)?;
        let mut all = self.state.store.list_datasets(&ctx).await?;
        all.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(all)
    }

    /// Deletes a harvest source and its runs and withdraws the active records it harvested
    /// (T-7416: a removed source leaves no records behind); `false` when the catalogue has no
    /// source of that name.
    pub async fn delete_harvest_source(
        &self,
        org: OrgId,
        id: CatalogueId,
        name: &str,
    ) -> Result<bool> {
        let _serial = self.serial.lock().await;
        let ctx = self.live(org, id)?;
        let store = self.state.store.as_ref();
        for ds in store.list_datasets(&ctx).await? {
            if ds.state == RecordState::Active && ds.harvest_source_id.as_deref() == Some(name) {
                store.withdraw_dataset(&ctx, &ds.id).await?;
            }
        }
        store.delete_harvest_source(&ctx, name).await
    }

    fn file_store(&self) -> Result<Arc<FileStore>> {
        self.state
            .files
            .clone()
            .ok_or_else(|| DataroomError::NotSupported("this installation stores no files".into()))
    }

    /// The files of catalogue `id` and what its Organization holds over all its portals (T-7417).
    pub async fn files(&self, org: OrgId, id: CatalogueId) -> Result<(Vec<FileEntry>, FileUsage)> {
        self.live(org, id)?;
        let store = self.file_store()?;
        blocking(move || Ok((store.list(org, id)?, store.usage(org)?))).await
    }

    /// One file write of catalogue `id` by person `by`, with its audit row (T-7417).
    pub async fn write_file(
        &self,
        org: OrgId,
        id: CatalogueId,
        by: &str,
        write: FileWrite,
    ) -> Result<FileEntry> {
        let _serial = self.serial.lock().await;
        let ctx = self.live(org, id)?;
        let store = self.file_store()?;
        let (operation, summary, entry) = blocking({
            let by = by.to_owned();
            move || {
                Ok(match write {
                    FileWrite::Upload { name, bytes } => {
                        let e = store.put((org, id), &name, &bytes, &by)?;
                        ("file.upload", format!("Uploaded file {}", e.name), e)
                    }
                    FileWrite::Rename { id: file, name } => {
                        let (was, e) = store.rename((org, id), &file, &name)?;
                        (
                            "file.rename",
                            format!("Renamed file {} to {}", was.name, e.name),
                            e,
                        )
                    }
                    FileWrite::Replace { id: file, bytes } => {
                        let (_, e) = store.replace((org, id), &file, &bytes, &by)?;
                        (
                            "file.replace",
                            format!("Replaced the bytes of file {}", e.name),
                            e,
                        )
                    }
                    FileWrite::Delete { id: file } => {
                        let e = store.delete((org, id), &file)?;
                        ("file.delete", format!("Deleted file {}", e.name), e)
                    }
                })
            }
        })
        .await?;
        self.state
            .store
            .record_activity(
                &ctx,
                ActivityRow {
                    org_id: org,
                    catalogue_id: id,
                    id: format!("act-{}", uuid::Uuid::now_v7()),
                    at: jc_time::TimestampTz::now(),
                    actor: by.to_owned(),
                    operation: operation.into(),
                    object_type: "File".into(),
                    object_id: entry.id.clone(),
                    summary,
                    details: serde_json::json!({
                        "name": entry.name, "sha256": entry.sha256,
                        "size": entry.size, "contentType": entry.content_type,
                    }),
                },
            )
            .await?;
        Ok(entry)
    }

    /// Lists in catalogue `to` the active datasets of catalogue `from` (whose public base is
    /// `from_base`) that `keep` selects, an `endpoints` source of a Portal (ODP-10): each a copy
    /// under its own name whose IRI stays `from`'s (ODP-14), marked `generated_by = marker`, its
    /// publisher copied when `to` lacks it. A name `to` holds for anything else (a harvested
    /// record) is left alone; a copy `keep` no longer selects is withdrawn. Answers the names
    /// `to` lists.
    pub async fn list_from(
        &self,
        org: OrgId,
        (from, from_base): (CatalogueId, &str),
        to: CatalogueId,
        marker: &str,
        keep: impl Fn(&Dataset) -> bool,
    ) -> Result<Vec<String>> {
        let _serial = self.serial.lock().await;
        let (source, target) = (self.live(org, from)?, self.live(org, to)?);
        let store = self.state.store.as_ref();
        let mut listed = Vec::new();
        for ds in store.list_datasets(&source).await? {
            if ds.state != RecordState::Active || !keep(&ds) {
                continue;
            }
            let held = store.get_dataset(&target, &ds.name).await?;
            if held
                .as_ref()
                .is_some_and(|h| h.generated_by.as_deref() != Some(marker))
            {
                continue;
            }
            listed.push(ds.name.clone());
            let copy = Dataset {
                org_id: org,
                catalogue_id: to,
                uri: Some(ds.canonical_uri(from_base)),
                generated_by: Some(marker.to_owned()),
                ..ds
            };
            if held.as_ref() == Some(&copy) {
                continue;
            }
            if store
                .get_publisher(&target, &copy.publisher_id)
                .await?
                .is_none()
            {
                if let Some(publisher) = store.get_publisher(&source, &copy.publisher_id).await? {
                    let publisher = Publisher {
                        org_id: org,
                        catalogue_id: to,
                        id: String::new(),
                        ..publisher
                    };
                    store.upsert_publisher(&target, publisher).await?;
                }
            }
            store.upsert_dataset(&target, copy).await?;
        }
        for ds in store.list_datasets(&target).await? {
            if ds.generated_by.as_deref() == Some(marker)
                && ds.state == RecordState::Active
                && !listed.contains(&ds.name)
            {
                store.withdraw_dataset(&target, &ds.id).await?;
            }
        }
        Ok(listed)
    }

    /// The context of a catalogue this writer serves; another one is not found.
    fn live(&self, org: OrgId, id: CatalogueId) -> Result<RequestCtx> {
        if !read(&self.harvested).contains(&(org, id)) {
            return Err(DataroomError::NotFound(format!("no catalogue {id}")));
        }
        Ok(RequestCtx::new(org, id, Subject::anonymous()))
    }
}

/// Pins every dataset of `ctx` that names no IRI of its own to the one it has under `old`, the
/// address the catalogue leaves: an address change never moves a dataset IRI (ODP-08), and the
/// old address keeps answering (a slug address redirects).
async fn keep_dataset_iris(
    store: &dyn dataroom_core::Store,
    ctx: &RequestCtx,
    old: &str,
) -> Result<()> {
    for mut ds in store.list_datasets(ctx).await? {
        let own = ds.uri.as_deref().is_some_and(|u| !u.trim().is_empty())
            || ds.extras.get("uri").is_some_and(|u| !u.trim().is_empty());
        if !own {
            ds.uri = Some(ds.canonical_uri(old));
            store.upsert_dataset(ctx, ds).await?;
        }
    }
    Ok(())
}

/// The publish principal of one catalogue (as [`portal_publisher_on`] names it).
fn principal_of(id: CatalogueId) -> String {
    format!("{}:{id}", dataroom_api::PORTAL_PUBLISHER)
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// A change to one portal file (T-7417).
#[derive(Debug, Clone)]
pub enum FileWrite {
    Upload { name: String, bytes: Vec<u8> },
    Rename { id: String, name: String },
    Replace { id: String, bytes: Vec<u8> },
    Delete { id: String },
}

/// Runs a file store call off the async workers.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    match tokio::task::spawn_blocking(f).await {
        Ok(answer) => answer,
        // Only a file store call that panicked ends here.
        Err(e) => Err(DataroomError::Storage(format!(
            "file store call stopped: {e}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dataroom_core::Store;
    use dataroom_store_memory::MemoryStore;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn id<T>(parse: fn(&str) -> std::result::Result<T, DataroomError>, n: u128) -> T {
        parse(&format!("00000000-0000-7000-8000-{n:012x}")).expect("an id")
    }

    /// jc's writer on `store` as it starts: the tables empty, nothing routed yet.
    fn started(store: Arc<dyn Store>) -> (DataroomState, CatalogueWriter) {
        let (org, cat) = (
            id(OrgId::parse_verified, 0xA1),
            id(CatalogueId::parse_verified, 0xC0),
        );
        let authorizer = Arc::new(LiveAuthorizer::new(Vec::new()));
        let state = DataroomState::new(store, authorizer.clone(), org, cat);
        let writer = CatalogueWriter::new(state.clone(), authorizer, Arc::default());
        (state, writer)
    }

    /// Portal `mobility` of Organization `city` on `host`, or at its slug address without one.
    fn mobility(host: Option<&str>) -> CatalogueSpec {
        let (org_id, id) = (
            id(OrgId::parse_verified, 0xA1),
            id(CatalogueId::parse_verified, 0xC1),
        );
        CatalogueSpec {
            org_id,
            id,
            name: "mobility".into(),
            host: host.map_or_else(
                || dataroom_api::synthetic_host("city", "mobility"),
                str::to_owned,
            ),
            base: Some(host.map_or_else(
                || "https://data.city.test/-/mobility".into(),
                |h| format!("https://{h}"),
            )),
            organization: None,
            publisher: serde_json::from_value(json!({
                "orgId": org_id, "catalogueId": id, "id": "", "name": "city",
                "title": {"sk": "Mesto"}, "description": {}, "uri": "https://data.city.test/publisher/city",
                "state": "active", "editorGrants": [],
                "created": "2026-11-14T10:00:00Z", "modified": "2026-11-14T10:00:00Z"
            }))
            .expect("a publisher"),
            publish_token: None,
            portal: Some(PortalRoute {
                organization: "city".into(),
                path: "mobility".into(),
                host: host.map(str::to_owned),
                redirect: false,
            }),
        }
    }

    #[tokio::test]
    async fn each_file_write_leaves_an_audit_row_and_another_portal_reads_none() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let root = std::env::temp_dir().join(format!("jc-writer-files-{}", uuid::Uuid::now_v7()));
        let (org, cat) = (
            id(OrgId::parse_verified, 0xA1),
            id(CatalogueId::parse_verified, 0xC0),
        );
        let authorizer = Arc::new(LiveAuthorizer::new(Vec::new()));
        let state = DataroomState::new(store.clone(), authorizer.clone(), org, cat)
            .with_files(Arc::new(FileStore::new(&root)));
        let writer = CatalogueWriter::new(state, authorizer, Arc::default());
        let spec = mobility(None);
        writer.upsert(spec.clone()).await.expect("served");
        let upload = FileWrite::Upload {
            name: "linky.csv".into(),
            bytes: b"linka;smer\n1;centrum\n".to_vec(),
        };
        let file = writer
            .write_file(spec.org_id, spec.id, "ana", upload)
            .await
            .expect("uploaded");
        let (listed, usage) = writer.files(spec.org_id, spec.id).await.expect("listed");
        assert_eq!((listed.len(), usage.files), (1, 1));
        let refused = writer
            .write_file(
                spec.org_id,
                spec.id,
                "ana",
                FileWrite::Upload {
                    name: "page.csv".into(),
                    bytes: b"<html><script>x</script></html>".to_vec(),
                },
            )
            .await
            .unwrap_err();
        assert!(
            matches!(refused, DataroomError::Validation { .. }),
            "{refused}"
        );
        writer
            .write_file(
                spec.org_id,
                spec.id,
                "bo",
                FileWrite::Delete {
                    id: file.id.clone(),
                },
            )
            .await
            .expect("deleted");
        let ctx = RequestCtx::new(spec.org_id, spec.id, Subject::anonymous());
        let rows: Vec<(String, String)> = store
            .list_activity(&ctx, None, 50)
            .await
            .expect("activity")
            .into_iter()
            .filter(|r| r.object_type == "File")
            .map(|r| (r.operation, r.actor))
            .collect();
        assert!(
            rows.contains(&("file.upload".into(), "ana".into())),
            "{rows:?}"
        );
        assert!(
            rows.contains(&("file.delete".into(), "bo".into())),
            "{rows:?}"
        );
        assert_eq!(rows.len(), 2, "a refused upload writes no row: {rows:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_deleted_source_withdraws_only_its_own_records() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let spec = mobility(None);
        let (_, writer) = started(store.clone());
        writer.upsert(spec.clone()).await.expect("served");
        let ctx = RequestCtx::new(spec.org_id, spec.id, Subject::anonymous());
        for (name, source) in [
            ("air", Some("kosice")),
            ("bus", Some("praha")),
            ("own", None),
        ] {
            let ds: dataroom_core::Dataset = serde_json::from_value(json!({
                "orgId": spec.org_id, "catalogueId": spec.id, "id": "", "name": name,
                "title": {"sk": name}, "description": {}, "publisherId": "city",
                "visibility": "public", "state": "active", "tags": [], "distributions": [],
                "harvestSourceId": source,
                "issued": "2026-11-14T10:00:00Z", "modified": "2026-11-14T10:00:00Z"
            }))
            .expect("a dataset");
            store.upsert_dataset(&ctx, ds).await.expect("stored");
        }
        writer
            .delete_harvest_source(spec.org_id, spec.id, "kosice")
            .await
            .expect("deleted");
        let states: Vec<(String, RecordState)> = store
            .list_datasets(&ctx)
            .await
            .expect("listed")
            .into_iter()
            .map(|d| (d.name, d.state))
            .collect();
        assert!(
            states.contains(&("air".into(), RecordState::Withdrawn)),
            "{states:?}"
        );
        assert!(
            states.contains(&("bus".into(), RecordState::Active)),
            "{states:?}"
        );
        assert!(
            states.contains(&("own".into(), RecordState::Active)),
            "{states:?}"
        );
    }

    const TOKEN_A: &str = "portal-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn dataset(org: OrgId, cat: CatalogueId, name: &str, extra: serde_json::Value) -> Dataset {
        let mut v = json!({
            "orgId": org, "catalogueId": cat, "id": "", "name": name,
            "title": {"sk": name}, "description": {}, "publisherId": "city",
            "visibility": "public", "state": "active", "tags": [], "distributions": [],
            "issued": "2026-11-14T10:00:00Z", "modified": "2026-11-14T10:00:00Z"
        });
        for (key, value) in extra.as_object().into_iter().flatten() {
            v[key] = value.clone();
        }
        serde_json::from_value(v).expect("a dataset")
    }

    /// Portal `transit` of the same Organization at its slug address.
    fn transit() -> CatalogueSpec {
        let mut spec = mobility(None);
        spec.id = id(CatalogueId::parse_verified, 0xC2);
        spec.name = "transit".into();
        spec.host = dataroom_api::synthetic_host("city", "transit");
        spec.base = Some("https://data.city.test/-/transit".into());
        spec.publisher.catalogue_id = spec.id;
        spec.portal = Some(PortalRoute {
            organization: "city".into(),
            path: "transit".into(),
            host: None,
            redirect: false,
        });
        spec
    }

    fn principals(state: &DataroomState) -> Vec<String> {
        read(&state.tokens)
            .iter()
            .filter_map(|b| b.subject.actor_id.clone())
            .collect()
    }

    #[tokio::test]
    async fn a_publish_token_binds_one_catalogue_and_another_catalogue_cannot_take_it() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let (state, writer) = started(store);
        let mut a = mobility(None);
        a.publish_token = Some(TOKEN_A.into());
        let shown = format!("{a:?}");
        assert!(
            shown.contains("<set>") && !shown.contains(TOKEN_A),
            "{shown}"
        );
        writer.upsert(a.clone()).await.expect("bound");
        writer
            .upsert(a.clone())
            .await
            .expect("the same catalogue again");
        assert_eq!(
            principals(&state),
            [principal_of(a.id)],
            "one binding per catalogue"
        );

        let mut b = transit();
        b.publish_token = Some(format!(" {TOKEN_A} "));
        let err = writer.upsert(b.clone()).await.unwrap_err();
        assert!(
            matches!(&err, DataroomError::Conflict(m) if m.contains("another catalogue's")),
            "{err}"
        );
        b.publish_token = Some("short".into());
        assert!(
            writer.upsert(b.clone()).await.is_err(),
            "a weak token is refused"
        );
        b.publish_token = None;
        writer.upsert(b).await.expect("no token, no binding");
        assert_eq!(principals(&state), [principal_of(a.id)]);

        a.publish_token = None;
        writer.upsert(a).await.expect("the token taken away");
        assert!(principals(&state).is_empty());
    }

    #[tokio::test]
    async fn withdraw_and_delete_stop_serving_and_only_delete_empties_the_store() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let (state, writer) = started(store.clone());
        let mut spec = mobility(Some("mobility.city.test"));
        spec.organization = Some("city".into());
        spec.publish_token = Some(TOKEN_A.into());
        writer.upsert(spec.clone()).await.expect("served");
        let ctx = RequestCtx::new(spec.org_id, spec.id, Subject::anonymous());
        store
            .upsert_dataset(&ctx, dataset(spec.org_id, spec.id, "air", json!({})))
            .await
            .expect("stored");
        assert_eq!(
            state.portals.host("mobility.city.test"),
            Some((spec.org_id, spec.id))
        );
        assert!(read(&state.organization_catalogues).contains_key("city"));

        writer.withdraw(spec.org_id, spec.id).await;
        assert_eq!(state.portals.host("mobility.city.test"), None);
        assert!(principals(&state).is_empty());
        assert!(!read(&state.organization_catalogues).contains_key("city"));
        assert!(!read(&state.catalogue_bases).contains_key(&spec.id));
        let err = writer.datasets(spec.org_id, spec.id).await.unwrap_err();
        assert!(
            matches!(&err, DataroomError::NotFound(m) if m.contains("no catalogue")),
            "{err}"
        );
        assert_eq!(
            store.list_datasets(&ctx).await.expect("kept").len(),
            1,
            "withdraw keeps data"
        );

        writer.upsert(spec.clone()).await.expect("served again");
        assert_eq!(
            writer
                .datasets(spec.org_id, spec.id)
                .await
                .expect("listed")
                .len(),
            1
        );
        assert!(writer.delete(spec.org_id, spec.id).await.expect("deleted"));
        assert!(writer.datasets(spec.org_id, spec.id).await.is_err());
        assert_eq!(state.portals.host("mobility.city.test"), None);
        let other = id(CatalogueId::parse_verified, 0xC9);
        assert!(
            !writer.delete(spec.org_id, other).await.expect("answered"),
            "no such catalogue"
        );
    }

    #[tokio::test]
    async fn sources_operations_and_listings_answer_only_for_a_served_catalogue() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let (_, writer) = started(store.clone());
        let spec = mobility(None);
        let (org, cat) = (spec.org_id, spec.id);
        let source: HarvestSource = serde_json::from_value(json!({
            "orgId": org, "catalogueId": cat, "name": "kosice", "kind": "dcat-ap",
            "url": "https://data.kosice.test/feed.ttl", "publisherId": "kraj",
            "allowedHosts": ["data.kosice.test"], "modified": "2026-11-14T10:00:00Z"
        }))
        .expect("a source");
        let mut kraj = spec.publisher.clone();
        kraj.name = "kraj".into();
        let not_found =
            |r: Result<()>| assert!(matches!(r, Err(DataroomError::NotFound(_))), "{r:?}");
        not_found(
            writer
                .upsert_harvest_source(org, cat, source.clone(), None)
                .await
                .map(|_| ()),
        );
        not_found(
            writer
                .run_op(org, cat, "harvest_source.list", json!({}))
                .await
                .map(|_| ()),
        );
        not_found(writer.datasets(org, cat).await.map(|_| ()));
        not_found(writer.files(org, cat).await.map(|_| ()));
        not_found(
            writer
                .delete_harvest_source(org, cat, "kosice")
                .await
                .map(|_| ()),
        );

        writer.upsert(spec).await.expect("served");
        let saved = writer
            .upsert_harvest_source(org, cat, source.clone(), Some(kraj.clone()))
            .await
            .expect("saved");
        assert_eq!(saved.name, "kosice");
        let ctx = RequestCtx::new(org, cat, Subject::anonymous());
        let made = store
            .get_publisher(&ctx, "kraj")
            .await
            .expect("read")
            .expect("made");
        let mut renamed = kraj.clone();
        renamed.title = BTreeMap::from([("sk".into(), "Iný".into())]);
        writer
            .upsert_harvest_source(org, cat, source.clone(), Some(renamed))
            .await
            .expect("saved again");
        writer
            .upsert_harvest_source(org, cat, source, None)
            .await
            .expect("saved without a publisher to make");
        let kept = store
            .get_publisher(&ctx, "kraj")
            .await
            .expect("read")
            .expect("kept");
        assert_eq!(
            kept.title, made.title,
            "an existing publisher is left as it is"
        );

        let listed = writer
            .run_op(org, cat, "harvest_source.list", json!({}))
            .await
            .expect("listed as the harvest principal");
        assert!(listed.to_string().contains("kosice"), "{listed}");
        for name in ["bus", "air"] {
            store
                .upsert_dataset(&ctx, dataset(org, cat, name, json!({})))
                .await
                .expect("stored");
        }
        let names: Vec<String> = writer
            .datasets(org, cat)
            .await
            .expect("listed")
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, ["air", "bus"]);
        assert!(!writer
            .delete_harvest_source(org, cat, "none")
            .await
            .expect("answered"));
        let err = writer.files(org, cat).await.unwrap_err();
        assert!(
            matches!(&err, DataroomError::NotSupported(m) if m.contains("stores no files")),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_rename_and_a_replace_leave_their_own_audit_rows() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let root = std::env::temp_dir().join(format!("jc-writer-files-{}", uuid::Uuid::now_v7()));
        let spec = mobility(None);
        let authorizer = Arc::new(LiveAuthorizer::new(Vec::new()));
        let state = DataroomState::new(store.clone(), authorizer.clone(), spec.org_id, spec.id)
            .with_files(Arc::new(FileStore::new(&root)));
        let writer = CatalogueWriter::new(state, authorizer, Arc::default());
        writer.upsert(spec.clone()).await.expect("served");
        let (org, cat) = (spec.org_id, spec.id);
        let upload = FileWrite::Upload {
            name: "a.csv".into(),
            bytes: b"x;y\n1;2\n".to_vec(),
        };
        let file = writer
            .write_file(org, cat, "ana", upload)
            .await
            .expect("uploaded");
        let rename = FileWrite::Rename {
            id: file.id.clone(),
            name: "b.csv".into(),
        };
        let renamed = writer
            .write_file(org, cat, "ana", rename)
            .await
            .expect("renamed");
        assert_eq!(renamed.name, "b.csv");
        let replace = FileWrite::Replace {
            id: file.id.clone(),
            bytes: b"x;y\n3;4\n".to_vec(),
        };
        let replaced = writer
            .write_file(org, cat, "bo", replace)
            .await
            .expect("replaced");
        assert_ne!(replaced.sha256, file.sha256);
        let ctx = RequestCtx::new(org, cat, Subject::anonymous());
        let rows: Vec<(String, String, String)> = store
            .list_activity(&ctx, None, 50)
            .await
            .expect("activity")
            .into_iter()
            .filter(|r| r.object_type == "File")
            .map(|r| (r.operation, r.actor, r.summary))
            .collect();
        assert!(
            rows.contains(&(
                "file.rename".into(),
                "ana".into(),
                "Renamed file a.csv to b.csv".into()
            )),
            "{rows:?}"
        );
        assert!(
            rows.contains(&(
                "file.replace".into(),
                "bo".into(),
                "Replaced the bytes of file b.csv".into()
            )),
            "{rows:?}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn list_from_copies_selected_records_keeps_foreign_names_and_withdraws_dropped_copies() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let (_, writer) = started(store.clone());
        let (from, to) = (mobility(None), transit());
        let org = from.org_id;
        writer.upsert(from.clone()).await.expect("from served");
        let source = RequestCtx::new(org, from.id, Subject::anonymous());
        let target = RequestCtx::new(org, to.id, Subject::anonymous());
        let base = "https://data.city.test/-/mobility";
        let marker = "endpoints:air";
        let err = writer
            .list_from(org, (from.id, base), to.id, marker, |_| true)
            .await
            .unwrap_err();
        assert!(
            matches!(err, DataroomError::NotFound(_)),
            "the target is not served: {err}"
        );
        writer.upsert(to.clone()).await.expect("to served");
        let mut kraj = from.publisher.clone();
        kraj.name = "kraj".into();
        store
            .upsert_publisher(&source, kraj)
            .await
            .expect("a publisher only the source holds");
        for (name, extra) in [
            ("air", json!({"publisherId": "kraj"})),
            ("solo", json!({"publisherId": "nikto"})),
            ("bus", json!({})),
            ("taken", json!({})),
            ("old", json!({"state": "withdrawn"})),
        ] {
            store
                .upsert_dataset(&source, dataset(org, from.id, name, extra))
                .await
                .expect("stored");
        }
        store
            .upsert_dataset(
                &target,
                dataset(org, to.id, "taken", json!({"publisherId": "city"})),
            )
            .await
            .expect("a harvested record of the target");

        let keep_air = |d: &Dataset| d.name != "bus";
        let listed = writer
            .list_from(org, (from.id, base), to.id, marker, keep_air)
            .await
            .expect("listed");
        assert_eq!(listed, ["air", "solo"]);
        assert!(
            store
                .get_publisher(&target, "nikto")
                .await
                .expect("read")
                .is_none(),
            "a publisher the source lacks is not made up"
        );
        let air = store
            .get_dataset(&target, "air")
            .await
            .expect("read")
            .expect("copied");
        assert_eq!(
            air.uri.as_deref(),
            Some("https://data.city.test/-/mobility/dataset/air")
        );
        assert_eq!(air.generated_by.as_deref(), Some(marker));
        let copied = store
            .get_publisher(&target, "kraj")
            .await
            .expect("read")
            .expect("copied");
        assert_eq!(copied.catalogue_id, to.id);
        let taken = store
            .get_dataset(&target, "taken")
            .await
            .expect("read")
            .expect("kept");
        assert_eq!(taken.generated_by, None, "a foreign record is left alone");
        assert_eq!(store.get_dataset(&target, "bus").await.expect("read"), None);
        assert_eq!(store.get_dataset(&target, "old").await.expect("read"), None);

        let again = writer
            .list_from(org, (from.id, base), to.id, marker, keep_air)
            .await
            .expect("listed again");
        assert_eq!(again, ["air", "solo"]);
        let none = writer
            .list_from(org, (from.id, base), to.id, marker, |_| false)
            .await
            .expect("nothing selected");
        assert!(none.is_empty());
        let air = store
            .get_dataset(&target, "air")
            .await
            .expect("read")
            .expect("kept");
        assert_eq!(
            air.state,
            RecordState::Withdrawn,
            "a copy no longer selected is withdrawn"
        );
        let taken = store
            .get_dataset(&target, "taken")
            .await
            .expect("read")
            .expect("kept");
        assert_eq!(
            taken.state,
            RecordState::Active,
            "another marker's record stays"
        );
    }

    #[tokio::test]
    async fn a_catalogue_without_a_portal_route_or_base_is_unrouted_and_its_base_forgotten() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let (state, writer) = started(store);
        let routed = mobility(Some("mobility.city.test"));
        writer.upsert(routed.clone()).await.expect("routed");
        assert_eq!(
            state.portals.host("mobility.city.test"),
            Some((routed.org_id, routed.id))
        );
        assert!(read(&state.catalogue_bases).contains_key(&routed.id));
        // Back on its own host, then away again: each host it left keeps answering.
        writer
            .upsert(mobility(Some("data.city.test")))
            .await
            .expect("moved");
        writer.upsert(routed.clone()).await.expect("moved back");
        assert_eq!(
            state.portals.host("data.city.test"),
            Some((routed.org_id, routed.id))
        );

        let plain = CatalogueSpec {
            base: None,
            portal: None,
            ..routed.clone()
        };
        writer.upsert(plain).await.expect("served without a route");
        assert_eq!(state.portals.host("mobility.city.test"), None);
        assert!(!read(&state.catalogue_bases).contains_key(&routed.id));
    }

    #[tokio::test]
    async fn the_slug_address_redirects_only_to_a_portals_own_host() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let (state, writer) = started(store);
        let redirect = |host: Option<&str>, redirect: bool| {
            let mut spec = mobility(host);
            if let Some(route) = spec.portal.as_mut() {
                route.redirect = redirect;
            }
            spec
        };
        let slug = || {
            state
                .portals
                .slug("city", "mobility")
                .expect("routed")
                .redirect
        };
        writer
            .upsert(redirect(None, true))
            .await
            .expect("slug only");
        assert_eq!(
            slug(),
            None,
            "without a host the slug address serves itself"
        );
        writer
            .upsert(redirect(Some("mobility.city.test"), false))
            .await
            .expect("own host");
        assert_eq!(slug(), None);
        writer
            .upsert(redirect(Some("mobility.city.test"), true))
            .await
            .expect("redirect");
        assert_eq!(slug().as_deref(), Some("https://mobility.city.test"));
    }

    #[tokio::test]
    async fn the_catalogue_row_is_written_only_when_its_portal_fields_change() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let (_, writer) = started(store.clone());
        let spec = mobility(None);
        let row = || async {
            store
                .get_catalogue(spec.org_id, spec.id)
                .await
                .expect("read")
                .expect("held")
        };
        writer.upsert(spec.clone()).await.expect("served");
        let first = row().await;
        assert!(
            first.left_hosts.is_empty(),
            "a slug address is no host a portal leaves"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        writer.upsert(spec.clone()).await.expect("unchanged");
        assert_eq!(
            row().await.modified,
            first.modified,
            "nothing changed, nothing written"
        );

        let mut based = spec.clone();
        based.base = Some("https://data.city.test/-/mobility-2".into());
        writer.upsert(based).await.expect("a new base");
        let moved = row().await;
        assert_eq!(
            moved.portal_base.as_deref(),
            Some("https://data.city.test/-/mobility-2")
        );
        assert_ne!(moved.modified, first.modified);

        writer
            .upsert(mobility(Some("mobility.city.test")))
            .await
            .expect("own host");
        assert!(
            row().await.left_hosts.is_empty(),
            "the synthetic host of the slug address is not kept as a left host"
        );
        writer
            .upsert(mobility(Some("data.city.test")))
            .await
            .expect("another host");
        assert_eq!(row().await.left_hosts, ["mobility.city.test"]);
    }

    #[tokio::test]
    async fn iri_pins_and_left_hosts_survive_a_restart() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
        let spec = mobility(Some("mobility.city.test"));
        let (_, writer) = started(store.clone());
        writer.upsert(spec.clone()).await.expect("on its own host");
        let ctx = RequestCtx::new(spec.org_id, spec.id, Subject::anonymous());
        let air: dataroom_core::Dataset = serde_json::from_value(json!({
            "orgId": spec.org_id, "catalogueId": spec.id, "id": "", "name": "air",
            "title": {"sk": "Ovzdušie"}, "description": {}, "publisherId": "city",
            "visibility": "public", "state": "active", "tags": [], "distributions": [],
            "issued": "2026-11-14T10:00:00Z", "modified": "2026-11-14T10:00:00Z"
        }))
        .expect("a dataset");
        store.upsert_dataset(&ctx, air).await.expect("stored");
        let own = dataset(
            spec.org_id,
            spec.id,
            "own",
            json!({"uri": "https://elsewhere.test/own"}),
        );
        store.upsert_dataset(&ctx, own).await.expect("stored");
        let extra = dataset(
            spec.org_id,
            spec.id,
            "extra",
            json!({"extras": {"uri": "https://elsewhere.test/extra"}}),
        );
        store.upsert_dataset(&ctx, extra).await.expect("stored");

        // jc restarts and the manifest now names the slug address: the first write after the
        // restart still pins the IRI to the host the portal leaves, and that host still routes.
        let (state, writer) = started(store.clone());
        writer
            .upsert(mobility(None))
            .await
            .expect("to its slug address");
        let mut uris = BTreeMap::new();
        for ds in store.list_datasets(&ctx).await.expect("listed") {
            uris.insert(ds.name, ds.uri);
        }
        assert_eq!(
            uris["air"].as_deref(),
            Some("https://mobility.city.test/dataset/air"),
            "the dataset keeps its IRI"
        );
        assert_eq!(uris["own"].as_deref(), Some("https://elsewhere.test/own"));
        assert_eq!(uris["extra"], None, "an IRI in the extras is its own");
        assert_eq!(
            state.portals.host("mobility.city.test"),
            Some((spec.org_id, spec.id))
        );

        // Another restart: the left host is read back from the catalogue row.
        let (state, writer) = started(store);
        writer.upsert(mobility(None)).await.expect("unchanged");
        assert_eq!(
            state.portals.host("mobility.city.test"),
            Some((spec.org_id, spec.id)),
            "the left host still answers after a restart"
        );
    }
}
