//! daiya (dashboards) inside `jc`: the state of the standalone `daiya serve` on its
//! in-memory store. When the Portal runs beside it, first-start setup holds a password for the
//! Portal's Grafana client, which daiya accepts as HTTP Basic for the subject `portal`, an
//! administrator of the default organization only.

use crate::setup::{DASHBOARDS_USER, Secret, Setup};
use axum::Router;
use base64::Engine;
use daiya_api::TokenBinding;
use daiya_model::{
    GrafanaRole, InstallationId, InstanceId, OrgId, PolicyAuthorizer, RoleBinding, Subject,
};
use daiya_server::{DaiyaState, router};
use daiya_store_memory::MemoryStore;
use jc_router::Mount;
use std::sync::Arc;

/// The credential string daiya hashes for HTTP Basic: base64 of `user:password`.
pub(crate) fn basic(user: &str, password: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
}

pub(crate) fn state(setup: &Setup) -> DaiyaState {
    let org = OrgId::from_u128(1);
    let store = Arc::new(MemoryStore::new());
    let Some(password) = setup.value(Secret::DashboardsPassword) else {
        return DaiyaState::for_standalone(store, org);
    };
    let authz = PolicyAuthorizer::new(vec![
        RoleBinding::from_grafana_role(org, "admin", GrafanaRole::Admin, None),
        RoleBinding::from_grafana_role(org, DASHBOARDS_USER, GrafanaRole::Admin, None),
    ]);
    DaiyaState::new(
        InstallationId::from_u128(1),
        org,
        InstanceId::from_u128(1),
        store,
        Arc::new(authz),
    )
    .with_tokens(vec![TokenBinding::new(
        &basic(DASHBOARDS_USER, password),
        Subject::authenticated(DASHBOARDS_USER, vec![org], Vec::new()),
    )])
}

pub fn build(setup: &Setup, mount: Mount) -> Router {
    router(state(setup), mount)
}
