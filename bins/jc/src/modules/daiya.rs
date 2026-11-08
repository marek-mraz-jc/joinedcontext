//! daiya (dashboards) inside `jc`: the state of the standalone `daiya serve`, on Postgres when
//! `[daiya.database]` is set (daiya's own `open_store`: migration as the owning role when one is
//! configured, then the request pool and the boot check that refuses a role or schema without
//! forced RLS), else on its in-memory store. When the Portal runs beside it, first-start setup
//! holds a password for the Portal's Grafana client, which daiya accepts as its server
//! administrator (HTTP Basic, `DaiyaState::with_server_admin`), the role Grafana's admin user
//! holds for the Portal.

use crate::config::DaiyaSection;
use crate::setup::{DASHBOARDS_USER, Secret, Setup};
use axum::Router;
use daiya_model::OrgId;
use daiya_server::config::{
    MigrateConfig, StoreConfig, open_store, provision, provisioning_vars_from,
};
use daiya_server::{DaiyaState, router};
use jc_router::Mount;
use std::net::SocketAddr;

fn store_config(cfg: &DaiyaSection) -> StoreConfig {
    match &cfg.database {
        None => StoreConfig::Memory,
        Some(db) => StoreConfig::Postgres {
            url: db.url.clone(),
            password_file: db.password_file.clone(),
            max_connections: db.max_connections(),
            migrate: db
                .migrate_url
                .clone()
                .zip(db.migrate_password_file.clone())
                .map(|(url, password_file)| MigrateConfig { url, password_file }),
        },
    }
}

pub(crate) async fn state(cfg: &DaiyaSection, setup: &Setup) -> Result<DaiyaState, String> {
    let org = OrgId::from_u128(1);
    let store = open_store(&store_config(cfg))
        .await
        .map_err(|e| e.to_string())?;
    let Some(password) = setup.value(Secret::DashboardsPassword) else {
        return Ok(DaiyaState::for_standalone(store, org));
    };
    DaiyaState::for_standalone(store, org)
        .with_server_admin(DASHBOARDS_USER, password)
        .map_err(|e| e.to_string())
}

/// daiya's router. With `[daiya] provisioning_dir` the files are loaded, and only the
/// datasources they wrote may reach `own`, `jc`'s listener; the API's datasources keep the
/// default outbound policy (no loopback, no private address, no http).
pub async fn build(
    cfg: &DaiyaSection,
    setup: &Setup,
    own: SocketAddr,
    mount: Mount,
) -> Result<Router, String> {
    let mut state = state(cfg, setup).await?;
    if let Some(dir) = &cfg.provisioning_dir {
        state.own_addresses = vec![own];
        let vars = provisioning_vars_from(std::env::vars());
        let report = provision(&state, dir, &vars)
            .await
            .map_err(|e| e.to_string())?;
        tracing::info!(
            datasources = report.datasources_upserted,
            dashboards = report.dashboards,
            "daiya: provisioning loaded"
        );
    }
    Ok(router(state, mount))
}
