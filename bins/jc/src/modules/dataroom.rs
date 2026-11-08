//! dataroom (dataset catalogue) inside `jc`: in-memory store with the default catalogue, the
//! policy authorizer of the standalone `dataroom serve`, URLs under the mount prefix. When the
//! Portal runs beside it, first-start setup holds a CKAN API token for the Portal: the subject
//! `portal-ckan`, Read and Propose on the default catalogue only
//! (`dataroom_api::portal_publisher`).

use crate::app::StartError;
use crate::config::DataroomSection;
use crate::module::Module;
use crate::setup::{Secret, Setup};
use axum::Router;
use dataroom_api::DataroomState;
use dataroom_core::{CatalogueId, OrgId, PolicyAuthorizer, Store};
use dataroom_store_memory::MemoryStore;
use jc_router::Mount;
use std::sync::Arc;

pub async fn build(
    cfg: &DataroomSection,
    setup: &Setup,
    mount: Mount,
) -> Result<Router, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Dataroom, e);
    let org_id = OrgId::parse_verified(&cfg.org_id).map_err(|e| err(&e))?;
    let catalogue_id = CatalogueId::parse_verified(&cfg.catalogue_id).map_err(|e| err(&e))?;
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new());
    dataroom_server::ensure_default_catalogue(store.as_ref(), org_id, catalogue_id, &mount.host)
        .await
        .map_err(|e| err(&e))?;
    let (authorizer, tokens) = match setup.value(Secret::CatalogueToken) {
        None => (PolicyAuthorizer::default(), Vec::new()),
        Some(token) => {
            let (binding, grant) =
                dataroom_api::portal_publisher(token, org_id, catalogue_id).map_err(|e| err(&e))?;
            (PolicyAuthorizer::new(vec![grant]), vec![binding])
        }
    };
    let state = DataroomState::new(store, Arc::new(authorizer), org_id, catalogue_id)
        .with_tokens(tokens)
        .with_public_base(mount.public_base())
        .with_mount_prefix(mount.path_prefix);
    Ok(dataroom_api::router(state))
}
