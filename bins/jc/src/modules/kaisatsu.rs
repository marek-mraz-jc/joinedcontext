//! kaisatsu (identity provider) inside `jc`: the same state the standalone `kaisatsu serve`
//! builds, on the in-memory store, with the KEK read from its file.

use crate::app::StartError;
use crate::config::KaisatsuSection;
use crate::module::Module;
use axum::Router;
use jc_router::Mount;
use kaisatsu_core::{LocalAuthorizer, Store};
use kaisatsu_crypto::Kek;
use kaisatsu_import::{ApplyOptions, apply_realm_files};
use kaisatsu_oidc::OutboundGuard;
use kaisatsu_server::{KaisatsuState, load_realm_files_from_path, router};
use kaisatsu_store_memory::MemoryStore;
use std::sync::Arc;

pub async fn build(cfg: &KaisatsuSection, mount: Mount) -> Result<Router, StartError> {
    let err = |e: &dyn std::fmt::Display| StartError::module(Module::Kaisatsu, e);
    let kek_file = cfg.kek_file.as_deref().ok_or_else(|| err(&"no kek_file"))?;
    let kek = Kek::from_file(kek_file).map_err(|e| err(&e))?;
    let store: Arc<dyn Store> = Arc::new(MemoryStore::new(kek.clone()));
    if let Some(dir) = &cfg.import_dir {
        let files = load_realm_files_from_path(dir).map_err(|e| err(&e))?;
        apply_realm_files(&store, &files, &ApplyOptions::default())
            .await
            .map_err(|e| err(&e))?;
    }
    let state = KaisatsuState::new(
        store,
        kek,
        OutboundGuard::default(),
        Arc::new(LocalAuthorizer),
    );
    Ok(router(state, mount))
}
