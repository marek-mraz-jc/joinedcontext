//! wardkeep (identity provider) inside `jc`: the same state the standalone `wardkeep serve`
//! builds, on the in-memory store, with the KEK read from its file.

use crate::app::StartError;
use crate::config::WardkeepSection;
use crate::module::Module;
use axum::Router;
use jc_router::Mount;
use std::sync::Arc;
use wardkeep_core::{LocalAuthorizer, Store};
use wardkeep_crypto::Kek;
use wardkeep_import::{ApplyOptions, apply_realm_files};
use wardkeep_oidc::OutboundGuard;
use wardkeep_server::{WardkeepState, load_realm_files_from_path, router};
use wardkeep_store_memory::MemoryStore;

pub async fn build(cfg: &WardkeepSection, mount: Mount) -> Result<Router, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Wardkeep, e);
    let kek_file = cfg.kek_file.as_deref().ok_or_else(|| err(&"no kek_file"))?;
    let kek = Kek::from_file(kek_file).map_err(|e| err(&e))?;
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek.clone()));
    if let Some(dir) = &cfg.import_dir {
        let files = load_realm_files_from_path(dir).map_err(|e| err(&e))?;
        apply_realm_files(&store, &files, &ApplyOptions::default())
            .await
            .map_err(|e| err(&e))?;
    }
    let state = WardkeepState::new(
        store,
        kek,
        OutboundGuard::default(),
        Arc::new(LocalAuthorizer),
    );
    Ok(router(state, mount))
}
