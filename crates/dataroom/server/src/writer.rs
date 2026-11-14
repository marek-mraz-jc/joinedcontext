//! The in-process write path for catalogues and harvest sources (ODP-11): the reconciler creates,
//! updates and deletes a Portal's catalogue while the dataroom serves, with no restart and no
//! network surface. The handle shares the serving state's tables, so a change is served from
//! the next request on; each catalogue gets a publish principal of its own.

use dataroom_api::{portal_publisher_on, DataroomState};
use dataroom_core::auth::constant_time_eq;
use dataroom_core::{
    Catalogue, CatalogueId, DataroomError, HarvestSource, LiveAuthorizer, OrgId, Publisher,
    RequestCtx, Result, Subject,
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
}

impl std::fmt::Debug for CatalogueSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogueSpec")
            .field("org_id", &self.org_id)
            .field("id", &self.id)
            .field("name", &self.name)
            .field("host", &self.host)
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
        }
    }

    /// Creates or updates the catalogue of `spec`, its publisher, its grants and its routes.
    /// Refused: a catalogue id another Organization holds, a token another catalogue holds.
    pub async fn upsert(&self, spec: CatalogueSpec) -> Result<Catalogue> {
        let (org, id) = (spec.org_id, spec.id);
        if read(&self.harvested)
            .iter()
            .any(|&(o, c)| c == id && o != org)
        {
            return Err(DataroomError::Conflict(format!(
                "catalogue {id} belongs to another Organization"
            )));
        }
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
        let mut cat = crate::ensure_catalogue(store, org, id, &spec.name, &spec.host).await?;
        if cat.deleted.is_some() {
            cat.deleted = None;
            cat.modified = chrono::Utc::now();
            cat = store.upsert_catalogue(cat).await?;
        }
        let ctx = RequestCtx::new(org, id, Subject::anonymous());
        if store
            .get_publisher(&ctx, &spec.publisher.name)
            .await?
            .is_none()
        {
            store.upsert_publisher(&ctx, spec.publisher.clone()).await?;
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
        match spec.base {
            Some(base) => bases.insert(id, base),
            None => bases.remove(&id),
        };
        drop(bases);
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
        let deleted = self.state.store.delete_catalogue(org, id).await?;
        let principal = principal_of(id);
        write(&self.state.tokens)
            .retain(|b| b.subject.actor_id.as_deref() != Some(principal.as_str()));
        self.authorizer.replace_catalogue(org, id, Vec::new());
        write(&self.state.catalogue_bases).remove(&id);
        write(&self.state.organization_catalogues).retain(|_, v| *v != (org, id));
        write(&self.harvested).retain(|v| *v != (org, id));
        Ok(deleted)
    }

    /// Creates or updates a harvest source of the catalogue; the scheduler runs it on its
    /// schedule from the next minute on.
    pub async fn upsert_harvest_source(
        &self,
        org: OrgId,
        id: CatalogueId,
        source: HarvestSource,
    ) -> Result<HarvestSource> {
        let ctx = self.live(org, id)?;
        self.state.store.upsert_harvest_source(&ctx, source).await
    }

    /// Deletes a harvest source and its runs; `false` when the catalogue has none of that name.
    pub async fn delete_harvest_source(
        &self,
        org: OrgId,
        id: CatalogueId,
        id_or_name: &str,
    ) -> Result<bool> {
        let ctx = self.live(org, id)?;
        self.state
            .store
            .delete_harvest_source(&ctx, id_or_name)
            .await
    }

    /// The context of a catalogue this writer serves; another one is not found.
    fn live(&self, org: OrgId, id: CatalogueId) -> Result<RequestCtx> {
        if !read(&self.harvested).contains(&(org, id)) {
            return Err(DataroomError::NotFound(format!("no catalogue {id}")));
        }
        Ok(RequestCtx::new(org, id, Subject::anonymous()))
    }
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
