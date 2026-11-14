//! The in-process write path for catalogues and harvest sources (ODP-11): the reconciler creates,
//! updates and deletes a Portal's catalogue while the dataroom serves, with no restart and no
//! network surface. The handle shares the serving state's tables, so a change is served from
//! the next request on; each catalogue gets a publish principal of its own.

use dataroom_api::{portal_publisher_on, DataroomState, PortalAddress};
use dataroom_core::auth::constant_time_eq;
use dataroom_core::{
    Catalogue, CatalogueId, DataroomError, Dataset, HarvestSource, LiveAuthorizer, OrgId,
    Publisher, RecordState, RequestCtx, Result, Subject,
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
            row.modified = chrono::Utc::now();
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
        let principal = principal_of(id);
        write(&self.state.tokens)
            .retain(|b| b.subject.actor_id.as_deref() != Some(principal.as_str()));
        self.authorizer.replace_catalogue(org, id, Vec::new());
        write(&self.state.catalogue_bases).remove(&id);
        self.state.portals.unroute(id);
        write(&self.state.organization_catalogues).retain(|_, v| *v != (org, id));
        write(&self.harvested).retain(|v| *v != (org, id));
        Ok(deleted)
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

    /// Deletes a harvest source and its runs; `false` when the catalogue has none of that name.
    pub async fn delete_harvest_source(
        &self,
        org: OrgId,
        id: CatalogueId,
        id_or_name: &str,
    ) -> Result<bool> {
        let _serial = self.serial.lock().await;
        let ctx = self.live(org, id)?;
        self.state
            .store
            .delete_harvest_source(&ctx, id_or_name)
            .await
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

#[cfg(test)]
mod tests {
    use super::*;
    use dataroom_core::Store;
    use dataroom_store_memory::MemoryStore;
    use serde_json::json;

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

        // jc restarts and the manifest now names the slug address: the first write after the
        // restart still pins the IRI to the host the portal leaves, and that host still routes.
        let (state, writer) = started(store.clone());
        writer
            .upsert(mobility(None))
            .await
            .expect("to its slug address");
        let pinned = store.list_datasets(&ctx).await.expect("listed");
        assert_eq!(
            pinned[0].uri.as_deref(),
            Some("https://mobility.city.test/dataset/air"),
            "the dataset keeps its IRI"
        );
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
