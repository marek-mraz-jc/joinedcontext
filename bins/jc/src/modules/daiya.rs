//! daiya (dashboards) inside `jc`: the state of the standalone `daiya serve`, on Postgres when
//! `[daiya.database]` is set (daiya's own `open_store`: migration as the owning role when one is
//! configured, then the request pool and the boot check that refuses a role or schema without
//! forced RLS), else on its in-memory store. When the Portal runs beside it, first-start setup
//! holds a password for the Portal's Grafana client, which daiya accepts as its server
//! administrator (HTTP Basic, `DaiyaState::with_server_admin`), the role Grafana's admin user
//! holds for the Portal.

use crate::config::DaiyaSection;
use crate::module::Module;
use crate::setup::{DASHBOARDS_USER, Secret, Setup};
use crate::siblings::Siblings;
use axum::Router;
use daiya_model::OrgId;
use daiya_server::config::{
    MigrateConfig, StoreConfig, declare_organizations, open_store, provision,
    provisioning_vars_from,
};
use daiya_server::{DaiyaState, OperatorReach, OwnReach, router};

use jc_router::Mount;

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
    // The Portal reads members-only Endpoints with the person's token, exchanged for the one
    // Endpoint a panel reads and handed over beside its credential.
    DaiyaState::for_standalone(store, org)
        .with_server_admin(DASHBOARDS_USER, password)
        .map(DaiyaState::with_person_tokens)
        .map_err(|e| e.to_string())
}

/// The gateway's Endpoints on `jc`'s own listener, as the Portal names them in the datasources
/// it makes (`{JC_PORTAL_GATEWAY_URL}/api/endpoint/{slug}`, `crate::siblings`): `None` when the
/// gateway is not in this container or answers on a host of its own.
#[must_use]
pub fn gateway_reach(siblings: &Siblings) -> Option<OwnReach> {
    let gateway = siblings.get(Module::Gateway)?;
    let own = format!("http://{}", crate::siblings::reach(siblings.listener));
    gateway
        .internal
        .strip_prefix(&own)
        .filter(|path| path.is_empty() || path.starts_with('/'))?;
    OwnReach::endpoints(&format!("{}/api/endpoint/", gateway.internal)).ok()
}

/// daiya's router. With `[daiya] provisioning_dir` the files are loaded. Every NGSI-LD and OData
/// datasource, file-made or not, may read the gateway's Endpoints on `jc`'s own listener
/// ([`gateway_reach`]) and nothing else there; organization 1's Prometheus datasources may reach
/// `[daiya] prometheus_reach` alone; the rest keeps the default outbound policy (no loopback, no
/// private address, no http).
pub async fn build(
    cfg: &DaiyaSection,
    setup: &Setup,
    siblings: &Siblings,
    mount: Mount,
) -> Result<Router, String> {
    let mut state = state(cfg, setup).await?;
    declare_organizations(&state, &cfg.organizations).map_err(|e| e.to_string())?;
    state.own_reach = gateway_reach(siblings).into_iter().collect();
    if let Some(raw) = &cfg.prometheus_reach {
        let reach = OperatorReach::parse(raw).map_err(|e| e.to_string())?;
        state.outbound_policy.prometheus_reach = Some((OrgId::from_u128(1), reach));
    }
    if let Some(dir) = &cfg.provisioning_dir {
        let vars = provisioning_vars_from(crate::config::process_env());
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
