//! daiya (dashboards) inside `jc`: the state of the standalone `daiya serve` on its
//! in-memory store. When the Portal runs beside it, first-start setup holds a password for the
//! Portal's Grafana client, which daiya accepts as its server administrator (HTTP Basic,
//! `DaiyaState::with_server_admin`), the role Grafana's admin user holds for the Portal.

use crate::setup::{DASHBOARDS_USER, Secret, Setup};
use axum::Router;
use daiya_model::OrgId;
use daiya_server::{DaiyaState, router};
use daiya_store_memory::MemoryStore;
use jc_router::Mount;
use std::sync::Arc;

pub(crate) fn state(setup: &Setup) -> Result<DaiyaState, String> {
    let org = OrgId::from_u128(1);
    let store = Arc::new(MemoryStore::new());
    let Some(password) = setup.value(Secret::DashboardsPassword) else {
        return Ok(DaiyaState::for_standalone(store, org));
    };
    DaiyaState::for_standalone(store, org)
        .with_server_admin(DASHBOARDS_USER, password)
        .map_err(|e| e.to_string())
}

pub fn build(setup: &Setup, mount: Mount) -> Result<Router, String> {
    Ok(router(state(setup)?, mount))
}
