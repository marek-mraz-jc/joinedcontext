//! controlroom (dashboards) inside `jc`: the state of the standalone `controlroom serve` on its
//! in-memory store.

use axum::Router;
use controlroom_model::OrgId;
use controlroom_server::{ControlroomState, router};
use controlroom_store_memory::MemoryStore;
use jc_router::Mount;
use std::sync::Arc;

pub fn build(mount: Mount) -> Router {
    let store = Arc::new(MemoryStore::new());
    let state = ControlroomState::for_standalone(store, OrgId::from_u128(1));
    router(state, mount)
}
