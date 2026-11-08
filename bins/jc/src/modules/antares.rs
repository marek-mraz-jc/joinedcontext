//! Antares (NGSI-LD broker) inside `jc`: the broker library's own steps (configuration from its
//! `ANTARES_*` environment, state, background work, router), served through the mount router
//! instead of its own listener. The host owns the tracing subscriber and the metrics recorder
//! (`telemetry::init_hosted`), so `/q/metrics` is not served by the broker; `ANTARES_HTTP_PORT`
//! and `ANTARES_METRICS_PORT` open no listener here.
//!
//! With `[antares.database]` the broker serves from Postgres (`ANTARES_STORE=postgres` unless
//! the environment asks for timescale), the URL built from the section and its password file
//! instead of `ANTARES_DATABASE_URL`, and a role that bypasses row-level security refuses the
//! start (`require_rls`, the standalone `ANTARES_REQUIRE_RLS=1`). The broker migrates itself at
//! start under sqlx's advisory lock, as standalone.
//!
//! The broker keeps process-wide statics (memory budget, WebSocket hub, caches): one Antares per
//! process.

use crate::app::StartError;
use crate::config::AntaresSection;
use crate::module::Module;
use axum::Router;

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::module(Module::Antares, e)
}

pub async fn build(
    cfg: &AntaresSection,
    setup: &crate::setup::Setup,
) -> Result<Router, StartError> {
    let render = antares_broker::telemetry::init_hosted();
    let mut broker_cfg = antares_broker::config_from_env().map_err(err)?;
    // The `/q` admin credential is mandatory in a shared process.
    let token_file = cfg
        .admin_token_file
        .as_deref()
        .or_else(|| setup.path(crate::setup::Secret::AntaresAdminToken))
        .ok_or_else(|| err("no admin_token_file"))?;
    broker_cfg.admin_token_file = Some(token_file.display().to_string());
    if let Some(db) = &cfg.database {
        match broker_cfg.store_name.as_str() {
            "memory" => broker_cfg.store_name = "postgres".into(),
            "postgres" | "timescale" => {}
            other => {
                return Err(err(format!(
                    "[antares.database] is set but ANTARES_STORE={other}; unset it or use postgres"
                )));
            }
        }
        // As standalone (`config_from_env`): bus=local over a shared database double-fires
        // notifications when a second process serves it, unless the deployment says it runs one.
        let one_process = std::env::var("ANTARES_ALLOW_SHARED_LOCAL")
            .is_ok_and(|v| matches!(v.as_str(), "1" | "true"));
        if broker_cfg.bus_mode == "local" && !one_process {
            return Err(err(
                "[antares.database] with ANTARES_BUS=local double-fires notifications when more \
                 than one jc shares the database; use ANTARES_BUS=nats, or set \
                 ANTARES_ALLOW_SHARED_LOCAL=1 for a single jc process",
            ));
        }
        let url = db.app_url_with_password().map_err(err)?;
        broker_cfg.database_url = Some(antares_broker::Secret::new(url));
    }
    if broker_cfg.database_url.is_some() || broker_cfg.store_name != "memory" {
        broker_cfg.require_rls = true;
    }
    if broker_cfg.metrics_port.is_some() {
        tracing::warn!(
            "ANTARES_METRICS_PORT opens no listener inside jc; use the jc admin listener"
        );
        broker_cfg.metrics_port = None;
    }
    let mut broker = antares_broker::build_state(&broker_cfg, render)
        .await
        .map_err(err)?;
    antares_broker::spawn_background(&mut broker)
        .await
        .map_err(err)?;
    let served = antares_broker::router(broker).map_err(err)?;
    Ok(Router::new().fallback_service(served.app))
}
