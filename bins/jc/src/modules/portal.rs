//! The Portal inside `jc`: its own configuration (`JC_*` environment, as the standalone Portal
//! reads it), its own state and auth, its background work, and its internal listener
//! (`JC_INTERNAL_BIND`; it opens only when a realm is configured). Inside `jc` it defaults to a
//! free loopback port (`127.0.0.1:0`) instead of the standalone `0.0.0.0:9090`, so it neither
//! clashes with another listener nor opens on every interface; an explicit value wins.
//!
//! `JC_PORTAL_PUBLIC_URL` defaults to the mount's public base when unset, so the Portal's absolute
//! URLs carry the slug or host it is mounted at. The settings that name a sibling module
//! (kaisatsu, shako, ekiben, dataroom, daiya, antares) default to that sibling's mount when it
//! runs in this `jc` (`siblings::portal_vars`); an environment value always wins.

use crate::app::StartError;
use crate::module::Module;
use crate::modules::BuildCtx;
use axum::Router;
use jc_router::Mount;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::module(Module::Portal, e)
}

/// The Portal's configuration from `lookup`, with the public URL defaulting to the mount and the
/// keys `lookup` leaves unset taken from `siblings` (also handed to the Portal's forge lookup).
/// The Portal's internal listener inside `jc` unless `JC_INTERNAL_BIND` says otherwise.
const INTERNAL_BIND: &str = "127.0.0.1:0";

pub(crate) fn config(
    mount: &Mount,
    lookup: impl Fn(&str) -> Option<String>,
    siblings: BTreeMap<String, String>,
) -> Result<Config, StartError> {
    let public = format!("{}/", mount.public_base());
    let mut config = Config::from_vars(|key| match lookup(key) {
        None if key == "JC_PORTAL_PUBLIC_URL" => Some(public.clone()),
        None if key == "JC_INTERNAL_BIND" => Some(INTERNAL_BIND.to_owned()),
        None => siblings.get(key).cloned(),
        other => other,
    })
    .map_err(err)?;
    config.host_vars = siblings;
    Ok(config)
}

/// Builds the Portal and starts its internal listener; answers the router and the address the
/// listener took (a free loopback port unless `JC_INTERNAL_BIND` says otherwise), which the
/// gateway reads revocations, connectors and usage from.
pub async fn build(
    ctx: BuildCtx<'_>,
    wire_siblings: bool,
    mount: Mount,
    shutdown: &CancellationToken,
) -> Result<(Router, Option<std::net::SocketAddr>), StartError> {
    let env = crate::config::portal_env;
    let siblings = if wire_siblings {
        crate::siblings::portal_vars(ctx.siblings, ctx.setup, env)
    } else {
        BTreeMap::new()
    };
    tracing::info!(
        keys = ?siblings.keys().collect::<Vec<_>>(),
        "portal: settings filled from sibling modules"
    );
    let config = config(&mount, env, siblings)?;
    let state = AppState::from_config(config).await.map_err(err)?;
    server::spawn_background(&state);
    let token = shutdown.clone();
    let internal = server::spawn_internal_listener(&state, async move { token.cancelled().await })
        .await
        .map_err(|e| err(format!("internal listener: {e}")))?;
    Ok((server::app(state), internal))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_public_url_defaults_to_the_mount_and_the_environment_wins() {
        let mount = Mount::new("portal", "https", "city.example", "/portal").unwrap();
        let cfg = config(&mount, |_| None, BTreeMap::new()).unwrap();
        assert_eq!(cfg.public_base_url.as_str(), "https://city.example/portal/");
        let cfg = config(
            &mount,
            |k| (k == "JC_PORTAL_PUBLIC_URL").then(|| "https://portal.example/".to_owned()),
            BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(cfg.public_base_url.as_str(), "https://portal.example/");
        let bad = config(
            &mount,
            |k| (k == "JC_PORTAL_BIND").then(|| "nope".to_owned()),
            BTreeMap::new(),
        );
        assert!(bad.is_err());
    }

    #[test]
    fn the_internal_listener_takes_a_free_loopback_port_unless_set() {
        let mount = Mount::new("portal", "https", "city.example", "/portal").unwrap();
        let cfg = config(&mount, |_| None, BTreeMap::new()).unwrap();
        assert!(cfg.internal_bind.ip().is_loopback());
        assert_eq!(cfg.internal_bind.port(), 0);
        let cfg = config(
            &mount,
            |k| (k == "JC_INTERNAL_BIND").then(|| "0.0.0.0:9191".to_owned()),
            BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(cfg.internal_bind.to_string(), "0.0.0.0:9191");
    }

    #[test]
    fn sibling_values_fill_unset_keys_and_reach_the_forge_lookup() {
        let mount = Mount::new("portal", "https", "city.example", "/portal").unwrap();
        let mut siblings = BTreeMap::new();
        siblings.insert(
            "JC_PORTAL_BROKER_URL".to_owned(),
            "http://127.0.0.1:9/broker".to_owned(),
        );
        siblings.insert(
            "JC_SHAKO_URL_TEST_ONLY".to_owned(),
            "http://127.0.0.1:9/git".to_owned(),
        );
        let cfg = config(&mount, |_| None, siblings.clone()).unwrap();
        assert_eq!(cfg.broker_url.as_deref(), Some("http://127.0.0.1:9/broker"));
        assert_eq!(
            cfg.forge_var("JC_SHAKO_URL_TEST_ONLY").as_deref(),
            Some("http://127.0.0.1:9/git")
        );
        // The environment wins over the sibling.
        let cfg = config(
            &mount,
            |k| (k == "JC_PORTAL_BROKER_URL").then(|| "https://broker.example".to_owned()),
            siblings,
        )
        .unwrap();
        assert_eq!(cfg.broker_url.as_deref(), Some("https://broker.example"));
    }
}
