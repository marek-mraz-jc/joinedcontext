//! daiya (dashboards) inside `jc`: the state of the standalone `daiya serve` on its
//! in-memory store.

use axum::Router;
use daiya_model::OrgId;
use daiya_server::{DaiyaState, router};
use daiya_store_memory::MemoryStore;
use jc_router::Mount;
use std::sync::Arc;

pub fn build(mount: Mount) -> Router {
    let store = Arc::new(MemoryStore::new());
    let state = DaiyaState::for_standalone(store, OrgId::from_u128(1));
    router(state, mount)
}
