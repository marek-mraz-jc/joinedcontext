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
    // jc serves the scrape on its admin listener, behind the admin token (`app::admin_router`).
    config.public_metrics = false;
    Ok(config)
}

/// Builds the Portal and starts its internal listener; answers the router and the address the
/// listener took (a free loopback port unless `JC_INTERNAL_BIND` says otherwise), which the
/// gateway reads revocations, connectors and usage from.
pub async fn build(
    ctx: BuildCtx<'_>,
    wire_siblings: bool,
    organizations: BTreeMap<String, String>,
    mount: Mount,
    shutdown: &CancellationToken,
) -> Result<(Router, Option<std::net::SocketAddr>), StartError> {
    let env = crate::config::portal_env;
    let mut siblings = if wire_siblings {
        crate::siblings::portal_vars(ctx.siblings, ctx.setup, env)
    } else {
        BTreeMap::new()
    };
    siblings.extend(organizations);
    tracing::info!(
        keys = ?siblings.keys().collect::<Vec<_>>(),
        "portal: settings filled from sibling modules"
    );
    let mut config = config(&mount, env, siblings)?;
    // The internal listener opens with a realm (`server::spawn_internal_listener`). It is bound
    // before the state is built: the stream deployer is wired from the config, and its
    // validation stage and run-log sinks post to this listener.
    let listener = match config.oidc {
        Some(_) => Some(
            tokio::net::TcpListener::bind(config.internal_bind)
                .await
                .map_err(|e| err(format!("internal listener: {e}")))?,
        ),
        None => None,
    };
    if let Some(addr) = listener.as_ref().and_then(|l| l.local_addr().ok()) {
        runner_reaches(&mut config, addr);
    }
    // The health probe marks its calls so the gateway door writes them as a poll's.
    let _ = joinedcontext_portal::health_probe::MARK.set(crate::access::probe_mark().to_owned());
    let state = AppState::from_config(config).await.map_err(err)?;
    // Set once the state is built: a failed attempt's listener goes, the next binds anew.
    if let Some(addr) = listener.as_ref().and_then(|l| l.local_addr().ok()) {
        let _ = super::PORTAL_INTERNAL.set(addr);
    }
    server::spawn_background(&state);
    let token = shutdown.clone();
    let internal = listener
        .map(|l| server::serve_internal_listener(&state, l, async move { token.cancelled().await }))
        .transpose()
        .map_err(|e| err(format!("internal listener: {e}")))?;
    let token = shutdown.clone();
    server::spawn_build_listener(&state, async move { token.cancelled().await })
        .await
        .map_err(|e| err(format!("build listener: {e}")))?;
    Ok((server::app(state), internal))
}

/// Points jc's own runner at the Portal's internal listener on `addr` (prod's deployment sets
/// `JC_PORTAL_PIPELINE_TEST_CAPTURE_URL` to it, base-values): without it the Portal renders no
/// validation stage (PL-60) and no outcome sink, so a run never reaches the run log (PL-62).
/// Only a loopback listener, which jc's ekiben reaches in the same process; an environment
/// value wins.
fn runner_reaches(config: &mut Config, addr: std::net::SocketAddr) {
    if config.pipeline_test_capture_url.is_none() && addr.ip().is_loopback() {
        config.pipeline_test_capture_url = Some(format!("http://{addr}"));
    }
}

/// The Portal's settings for the `[[organizations]]` of `cfg`, each one's Portal host and own
/// catalogue: `JC_PORTAL_ORGANIZATION_HOSTS` and, when dataroom runs here,
/// `JC_PORTAL_ORGANIZATION_CATALOGUES` for every Organization that has a Portal token, called on
/// this jc's own `listener` under the catalogue's host name. Empty without Organizations; an
/// environment value still wins.
pub(crate) fn organization_vars(
    cfg: &crate::config::Config,
    listener: std::net::SocketAddr,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if cfg.organizations.is_empty() {
        return out;
    }
    let scheme = &cfg.scheme;
    let hosts: Vec<String> = cfg
        .organizations
        .iter()
        .map(|org| format!("{}={scheme}://{}", org.name, org.hosts.portal))
        .collect();
    out.insert("JC_PORTAL_ORGANIZATION_HOSTS".to_owned(), hosts.join(","));
    if cfg.roles.contains(&Module::Dataroom) {
        let reach = crate::siblings::reach(listener);
        let catalogues: Vec<serde_json::Value> = cfg
            .organizations
            .iter()
            .filter_map(|org| {
                let token = org.portal_token_file.as_ref()?;
                // The gateway links a dataset's resources; off https a whole origin.
                let data_host = match scheme.as_str() {
                    "https" => org.hosts.api.clone(),
                    _ => format!("{scheme}://{}", org.hosts.api),
                };
                Some(serde_json::json!({
                    "organization": org.name,
                    "url": format!("{scheme}://{}", org.hosts.opendata),
                    "api": format!("http://{}", org.hosts.opendata),
                    "reach": reach.to_string(),
                    "dataHost": data_host,
                    "tokenFile": token,
                }))
            })
            .collect();
        if !catalogues.is_empty() {
            out.insert(
                "JC_PORTAL_ORGANIZATION_CATALOGUES".to_owned(),
                serde_json::Value::Array(catalogues).to_string(),
            );
        }
    }
    out
}

/// `JC_PORTAL_APPS_URL`, the apex every App host sits under, from the Portal's App-host entry
/// `*.apps.{domain}` (edge-session-design S4a): `{scheme}://{domain}`, and
/// `JC_PORTAL_SERVE_APP_HOSTS`, since no edge composes the App routes in front of jc. Empty
/// without one; an environment value still wins.
pub(crate) fn apps_vars(cfg: &crate::config::Config) -> BTreeMap<String, String> {
    cfg.mounts
        .get(&Module::Portal)
        .into_iter()
        .flatten()
        .find_map(|entry| {
            let host = entry.host.as_deref()?.trim().to_ascii_lowercase();
            crate::app::apps_wildcard(&host).map(str::to_owned)
        })
        .map(|domain| {
            let url = format!("{}://{}", cfg.scheme, domain);
            [
                ("JC_PORTAL_APPS_URL".to_owned(), url),
                ("JC_PORTAL_SERVE_APP_HOSTS".to_owned(), "true".to_owned()),
            ]
        })
        .into_iter()
        .flatten()
        .collect()
}

/// The Portal's router behind the host jc placed the request on (`Mount`, a trusted proxy's
/// `X-Forwarded-Host` applied), which pins an Organization's host (`organization_host`).
pub(crate) fn with_request_host(router: Router) -> Router {
    router.layer(axum::middleware::from_fn(
        |mut req: axum::extract::Request, next: axum::middleware::Next| async move {
            let host = req.extensions().get::<Mount>().map(|m| m.host.clone());
            if let Some(host) = host {
                req.extensions_mut()
                    .insert(joinedcontext_portal::organization_host::RequestHost(host));
            }
            next.run(req).await
        },
    ))
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
    fn the_runner_reaches_a_loopback_internal_listener_unless_the_environment_names_one() {
        let mount = Mount::new("portal", "https", "city.example", "/portal").unwrap();
        let mut cfg = config(&mount, |_| None, BTreeMap::new()).unwrap();
        runner_reaches(&mut cfg, "127.0.0.1:55551".parse().unwrap());
        assert_eq!(
            cfg.pipeline_test_capture_url.as_deref(),
            Some("http://127.0.0.1:55551")
        );
        let mut cfg = config(&mount, |_| None, BTreeMap::new()).unwrap();
        runner_reaches(&mut cfg, "10.0.0.5:9090".parse().unwrap());
        assert_eq!(cfg.pipeline_test_capture_url, None);
        let set = |k: &str| {
            (k == "JC_PORTAL_PIPELINE_TEST_CAPTURE_URL")
                .then(|| "http://portal.internal:9090".to_owned())
        };
        let mut cfg = config(&mount, set, BTreeMap::new()).unwrap();
        runner_reaches(&mut cfg, "127.0.0.1:55551".parse().unwrap());
        assert_eq!(
            cfg.pipeline_test_capture_url.as_deref(),
            Some("http://portal.internal:9090")
        );
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
    /// jc-multiorg-portal: every Organization's Portal host reaches the Portal, and with
    /// dataroom here its own catalogue, called on this jc under the catalogue's name, with the
    /// Organization's token file; an Organization without a token publishes nowhere.
    #[test]
    fn each_organization_gets_its_portal_host_and_its_own_catalogue() {
        use clap::Parser as _;
        let text = include_str!("../../tests/fixtures/two-orgs.toml");
        let file = crate::config::Config::parse_file(std::path::Path::new("jc.toml"), text)
            .expect("parses");
        let cli =
            |roles: &str| crate::config::Cli::try_parse_from(["jc", "--role", roles]).expect("cli");
        let listener: std::net::SocketAddr = "0.0.0.0:42301".parse().unwrap();
        let mut cfg =
            crate::config::Config::merge(cli("portal,dataroom"), file.clone()).expect("config");
        cfg.organizations[0].portal_token_file = Some("/run/alpha-token".into());
        let vars = organization_vars(&cfg, listener);
        assert_eq!(
            vars["JC_PORTAL_ORGANIZATION_HOSTS"],
            format!(
                "alpha={0}://portal.alpha.test,beta={0}://portal.beta.test",
                cfg.scheme
            )
        );
        let catalogues: serde_json::Value =
            serde_json::from_str(&vars["JC_PORTAL_ORGANIZATION_CATALOGUES"]).expect("json");
        let parsed = joinedcontext_portal::organization_host::parse_catalogues(
            &vars["JC_PORTAL_ORGANIZATION_CATALOGUES"],
        )
        .expect("the Portal reads it");
        assert_eq!(parsed.len(), 1, "beta has no token: {catalogues}");
        assert_eq!(parsed[0].organization, "alpha");
        assert_eq!(
            parsed[0].api.as_ref().map(url::Url::as_str),
            Some("http://opendata.alpha.test/")
        );
        assert!(parsed[0].reach.is_some_and(|r| r.ip().is_loopback()));
        assert_eq!(
            parsed[0].token_file,
            std::path::PathBuf::from("/run/alpha-token")
        );
        assert!(!vars["JC_PORTAL_ORGANIZATION_CATALOGUES"].contains("beta"));
        // Without dataroom here, the hosts alone; without Organizations, nothing.
        let mut portal_only = crate::config::Config::merge(cli("portal"), file).expect("config");
        portal_only.organizations[0].portal_token_file = Some("/run/alpha-token".into());
        let vars = organization_vars(&portal_only, listener);
        assert!(!vars.contains_key("JC_PORTAL_ORGANIZATION_CATALOGUES"));
        portal_only.organizations.clear();
        assert!(organization_vars(&portal_only, listener).is_empty());
    }
}
