//! The Portal inside `jc`: its own configuration (`JC_*` environment, as the standalone Portal
//! reads it), its own state and auth, its background work, and its internal listener
//! (`JC_INTERNAL_BIND`, default `0.0.0.0:9090`; it opens only when a realm is configured). Antares
//! opens no listener inside `jc`, so its 9090 no longer meets this one; a clash with `jc`'s own
//! listeners shows as a bind error at start.
//!
//! `JC_PORTAL_PUBLIC_URL` defaults to the mount's public base when unset, so the Portal's absolute
//! URLs carry the slug or host it is mounted at.

use crate::app::StartError;
use crate::module::Module;
use axum::Router;
use jc_router::Mount;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use tokio_util::sync::CancellationToken;

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::module(Module::Portal, e)
}

/// The Portal's configuration from `lookup`, with the public URL defaulting to the mount.
pub(crate) fn config(
    mount: &Mount,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Config, StartError> {
    let public = format!("{}/", mount.public_base());
    Config::from_vars(|key| match lookup(key) {
        None if key == "JC_PORTAL_PUBLIC_URL" => Some(public.clone()),
        other => other,
    })
    .map_err(err)
}

pub async fn build(mount: Mount, shutdown: &CancellationToken) -> Result<Router, StartError> {
    let config = config(&mount, |key| std::env::var(key).ok())?;
    let state = AppState::from_config(config).await.map_err(err)?;
    server::spawn_background(&state);
    let token = shutdown.clone();
    server::spawn_internal_listener(&state, async move { token.cancelled().await })
        .await
        .map_err(|e| err(format!("internal listener: {e}")))?;
    Ok(server::app(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_public_url_defaults_to_the_mount_and_the_environment_wins() {
        let mount = Mount::new("portal", "https", "city.example", "/portal").unwrap();
        let cfg = config(&mount, |_| None).unwrap();
        assert_eq!(cfg.public_base_url.as_str(), "https://city.example/portal/");
        let cfg = config(&mount, |k| {
            (k == "JC_PORTAL_PUBLIC_URL").then(|| "https://portal.example/".to_owned())
        })
        .unwrap();
        assert_eq!(cfg.public_base_url.as_str(), "https://portal.example/");
        let bad = config(&mount, |k| {
            (k == "JC_PORTAL_BIND").then(|| "nope".to_owned())
        });
        assert!(bad.is_err());
    }
}
