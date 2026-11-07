//! dataroom (dataset catalogue) inside `jc`: in-memory store with the default catalogue, the
//! policy authorizer of the standalone `dataroom serve`, URLs under the mount prefix.

use crate::app::StartError;
use crate::config::DataroomSection;
use crate::module::Module;
use axum::Router;
use dataroom_api::DataroomState;
use dataroom_core::{CatalogueId, OrgId, PolicyAuthorizer, Store};
use dataroom_store_memory::MemoryStore;
use jc_router::Mount;
use std::sync::Arc;

pub async fn build(cfg: &DataroomSection, mount: Mount) -> Result<Router, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Dataroom, e);
    let org_id = OrgId::parse_verified(&cfg.org_id).map_err(|e| err(&e))?;
    let catalogue_id = CatalogueId::parse_verified(&cfg.catalogue_id).map_err(|e| err(&e))?;
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    dataroom_server::ensure_default_catalogue(store.as_ref(), org_id, catalogue_id, &mount.host)
        .await
        .map_err(|e| err(&e))?;
    let state = DataroomState::new(
        store,
        Arc::new(PolicyAuthorizer::default()),
        org_id,
        catalogue_id,
    )
    .with_mount_prefix(mount.path_prefix);
    Ok(dataroom_api::router(state))
}
