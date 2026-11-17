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
use joinedcontext_portal::host_registry::{SeedHost, SeedOrganization};
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
    orgs: &[crate::config::OrganizationSection],
    organizations: BTreeMap<String, String>,
    registry: HostRegistry,
    mount: Mount,
    shutdown: &CancellationToken,
) -> Result<(Router, Option<std::net::SocketAddr>), StartError> {
    let env = crate::config::portal_env;
    let mut siblings = if wire_siblings {
        crate::siblings::portal_vars(ctx.siblings, ctx.setup, env)
    } else {
        BTreeMap::new()
    };
    let follow_hosts = env("JC_PORTAL_ORGANIZATION_HOSTS").is_none()
        && organizations.contains_key("JC_PORTAL_ORGANIZATION_HOSTS");
    siblings.extend(organizations);
    let follow_realms = organization_realm_vars(
        &super::sign_in::from_config(orgs),
        ctx.siblings,
        &mount.scheme,
        &mut siblings,
        env,
    );
    tracing::info!(
        keys = ?siblings.keys().collect::<Vec<_>>(),
        "portal: settings filled from sibling modules"
    );
    let mut config = config(&mount, env, siblings)?;
    // The internal listener opens with a realm (`server::spawn_internal_listener`). It is bound
    // before the state is built: the stream deployer is wired from the config, and its
    // validation stage and run-log sinks post to this listener.
    let listener = match (&config.oidc, ctx.portal_internal) {
        (None, _) => None,
        // The listener jc reserved: each attempt takes a handle of it, so a failed attempt
        // leaves the address the proxy already holds bound.
        (Some(_), Some(reserved)) => Some(
            reserved
                .try_clone()
                .and_then(tokio::net::TcpListener::from_std)
                .map_err(|e| err(format!("internal listener: {e}")))?,
        ),
        (Some(_), None) => Some(
            tokio::net::TcpListener::bind(config.internal_bind)
                .await
                .map_err(|e| err(format!("internal listener: {e}")))?,
        ),
    };
    if let Some(addr) = listener.as_ref().and_then(|l| l.local_addr().ok()) {
        runner_reaches(&mut config, addr);
    }
    // The health probe marks its calls so the gateway door writes them as a poll's.
    let _ = joinedcontext_portal::health_probe::MARK.set(crate::access::probe_mark().to_owned());
    let state = AppState::from_config(config).await.map_err(err)?;
    if let Some(pool) = &state.db {
        let added =
            joinedcontext_portal::host_registry::seed_organizations(pool, &registry.organizations)
                .await
                .map_err(|e| {
                    err(format!(
                        "Organization registry: the jc.toml Organizations could not be seeded: {e}"
                    ))
                })?;
        tracing::info!(
            added,
            of = registry.organizations.len(),
            "Organization registry: jc.toml Organizations seeded"
        );
        let added = joinedcontext_portal::host_registry::seed(pool, &registry.seed)
            .await
            .map_err(|e| {
                err(format!(
                    "host registry: the jc.toml hosts could not be seeded: {e}"
                ))
            })?;
        tracing::info!(
            added,
            of = registry.seed.len(),
            "host registry: jc.toml hosts seeded"
        );
    }
    // Set once the state is built: a failed attempt's listener goes, the next binds anew.
    if let Some(addr) = listener.as_ref().and_then(|l| l.local_addr().ok()) {
        let _ = super::PORTAL_INTERNAL.set(addr);
    }
    let _ = super::PORTAL_STATE.set(state.clone());
    server::spawn_background(&state);
    if let Some(reach) = follow_realms {
        let (portal, scheme) = (state.clone(), mount.scheme.clone());
        super::sign_in::follow("portal", move |set| {
            match portal
                .replace_organization_realms(&organization_realms(set, &scheme), Some(reach))
            {
                Ok(held) => tracing::info!(held, "portal: Organization realms replaced"),
                Err(e) => tracing::error!(error = %e, "portal: Organization realms kept"),
            }
        });
    }
    if let (Some(pool), Some(hosts)) = (&state.db, registry.hosts) {
        // The Portal's Organization hosts follow the registry where jc filled them (T-7183).
        let run = Run {
            hosts,
            roles: registry.roles,
            named: registry.organizations.into_iter().map(|o| o.name).collect(),
            scheme: mount.scheme.clone(),
            portal: follow_hosts.then(|| state.clone()),
        };
        tokio::spawn(watch_registry(pool.clone(), run, shutdown.clone()));
    }
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

/// The host registry as the Portal's build gets it (T-7129 §1): the jc.toml Organizations and
/// hosts it seeds, and the router's host table it then keeps on the registry's served rows.
pub struct HostRegistry {
    pub organizations: Vec<SeedOrganization>,
    pub seed: Vec<SeedHost>,
    /// The modules jc runs: each takes its entries on the hosts the registry adds (T-7184).
    pub roles: Vec<Module>,
    /// `None` where no router swaps its table (no Organizations, or a Portal built in line).
    pub hosts: Option<jc_router::HostSwap>,
}

/// How often jc rebuilds its host table from the registry without a notification.
const REGISTRY_REBUILD: std::time::Duration = std::time::Duration::from_secs(60);

/// Keeps the router's host table on the registry's `served` rows (T-7129 §1, T-7136): a rebuild
/// at once, on every `NOTIFY organization_hosts` and every [`REGISTRY_REBUILD`]. A rebuild that
/// fails keeps the table that answers and sets `jc_host_registry_stale` to 1 until one succeeds.
/// Each rebuild is [`apply`]; a write to `organizations` wakes it too (migration 0063, T-7181).
async fn watch_registry(pool: sqlx::PgPool, run: Run, shutdown: CancellationToken) {
    let mut listener: Option<sqlx::postgres::PgListener> = None;
    loop {
        if listener.is_none() {
            listener = listen_registry(&pool).await;
        }
        let stale = match rebuild_hosts(&pool, &run).await {
            Ok(served) => {
                tracing::debug!(served, "host registry: host table rebuilt");
                0.0
            }
            Err(e) => {
                tracing::error!(error = %e, "host registry: host table kept, the rebuild failed");
                1.0
            }
        };
        metrics::gauge!("jc_host_registry_stale").set(stale);
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(REGISTRY_REBUILD) => {}
            woke = async {
                match listener.as_mut() {
                    Some(l) => l.recv().await.map(drop),
                    None => std::future::pending().await,
                }
            } => {
                if let Err(e) = woke {
                    tracing::warn!(error = %e, "host registry: notifications lost, listening again");
                    listener = None;
                }
            }
        }
    }
}

/// A listener on `organization_hosts`; `None` (logged) when the database refuses one, so the
/// timed rebuild carries on alone.
async fn listen_registry(pool: &sqlx::PgPool) -> Option<sqlx::postgres::PgListener> {
    let mut listener = match sqlx::postgres::PgListener::connect_with(pool).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(error = %e, "host registry: no notifications, rebuilding on the timer");
            return None;
        }
    };
    match listener.listen("organization_hosts").await {
        Ok(()) => Some(listener),
        Err(e) => {
            tracing::warn!(error = %e, "host registry: no notifications, rebuilding on the timer");
            None
        }
    }
}

/// Reads both registries, [`apply`]s them, and last hands the served sign-in hosts of the active
/// Organizations to every module with an issuer table (T-7180, `sign_in`), so a realm is always
/// taken beside its Organization's host.
async fn rebuild_hosts(pool: &sqlx::PgPool, run: &Run) -> Result<usize, String> {
    let rows = joinedcontext_portal::host_registry::rows(pool)
        .await
        .map_err(|e| format!("the registry could not be read: {e}"))?;
    let organizations = joinedcontext_portal::host_registry::organizations(pool)
        .await
        .map_err(|e| format!("the Organization registry could not be read: {e}"))?;
    let served = apply(run, &rows, &organizations)?;
    super::sign_in::publish(super::sign_in::from_registry(
        rows.iter().map(|r| {
            (
                r.host.as_str(),
                r.organization.as_str(),
                r.role.as_str(),
                r.state.as_str(),
            )
        }),
        &active(&organizations),
    ));
    Ok(served)
}

/// What the registry's watcher keeps on the rows: the router's host table and the entries of
/// the hosts it adds, the gateway's links of the Organizations jc.toml does not name (`named`
/// are those it does), and the Portal's Organization hosts where jc filled them.
struct Run {
    hosts: jc_router::HostSwap,
    roles: Vec<Module>,
    named: Vec<String>,
    scheme: String,
    portal: Option<AppState>,
}

/// Applies the registry's rows; the number of hosts served. The Portal takes the working hosts
/// (T-7183), the gateway links the Organizations jc.toml does not name and the router mounts each
/// module of jc on the hosts jc.toml did not mount (T-7184); last the host table takes only the
/// `served` hosts of `active` Organizations (T-7181), and a removed Organization leaves it whole,
/// its shared paths too. A step that fails stops the rest: the table that answers stays.
fn apply(
    run: &Run,
    rows: &[joinedcontext_portal::host_registry::HostRow],
    organizations: &[joinedcontext_portal::host_registry::OrganizationRow],
) -> Result<usize, String> {
    let known = active(organizations);
    let registry: Vec<jc_router::RegistryHost<'_>> = rows
        .iter()
        .map(|r| jc_router::RegistryHost {
            host: &r.host,
            organization: &r.organization,
            organization_id: r.organization_id.as_deref(),
            role: &r.role,
            served: r.state == "served" && known.contains(&r.organization),
        })
        .collect();
    let orgs = crate::app::registry_hosts(
        registry
            .iter()
            .filter(|r| r.served)
            .map(|r| (r.host, r.organization, r.role)),
    );
    // The Organization of a host is in place where it is read before the host answers: an
    // added host is pinned in the Portal and linked at the gateway first; a removed one is
    // unknown to the gateway (refused there) until the table drops it.
    if let Some(state) = &run.portal {
        state
            .replace_organization_hosts(&portal_hosts(organizations, &orgs, &run.scheme))
            .map_err(|e| format!("the Portal kept its Organization hosts: {e}"))?;
    }
    #[cfg(feature = "gateway")]
    super::gateway::replace_registry_links(
        &orgs
            .iter()
            .filter(|(name, _)| !run.named.contains(name))
            .map(|(name, hosts)| (name.clone(), hosts.clone()))
            .collect(),
        &run.scheme,
    );
    run.hosts
        .mount_hosts(&crate::app::registry_specs(&orgs, &run.roles))
        .map_err(|e| format!("the hosts the registry added kept their entries: {e}"))?;
    let removed: Vec<&str> = organizations
        .iter()
        .filter(|o| o.state != "active")
        .map(|o| o.name.as_str())
        .collect();
    run.hosts
        .rebuild_without(&registry, &removed)
        .map_err(|e| e.to_string())?;
    Ok(registry.iter().filter(|r| r.served).count())
}

/// `JC_PORTAL_ORGANIZATION_HOSTS` from the registry (T-7183): each `active` Organization at its
/// working host while the host registry serves it for that Organization (`served`,
/// `app::registry_hosts`), else at the first working host it serves (jc.toml moved it).
fn portal_hosts(
    organizations: &[joinedcontext_portal::host_registry::OrganizationRow],
    served: &BTreeMap<String, crate::config::OrganizationHosts>,
    scheme: &str,
) -> String {
    organizations
        .iter()
        .filter(|o| o.state == "active")
        .filter_map(|o| {
            let working = served.get(&o.name).map(|h| h.portal.as_str())?;
            let host = if jc_router::registry::bare(&o.host) == working {
                o.host.as_str()
            } else {
                working
            };
            (!host.is_empty()).then(|| format!("{}={scheme}://{host}", o.name))
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// The names of the `active` Organization rows: the Organizations jc runs now.
fn active(rows: &[joinedcontext_portal::host_registry::OrganizationRow]) -> Vec<String> {
    rows.iter()
        .filter(|r| r.state == "active")
        .map(|r| r.name.clone())
        .collect()
}

/// The Organization registry's seed (T-7181): each `[[organizations]]` entry's name, the id it
/// sets and its working host, lowercase, with the port jc.toml gives it (the Portal's public base
/// keeps it).
pub(crate) fn organization_seed(cfg: &crate::config::Config) -> Vec<SeedOrganization> {
    cfg.organizations
        .iter()
        .map(|o| SeedOrganization {
            name: o.name.clone(),
            organization_id: o.id.as_ref().map(|id| id.as_str().to_owned()),
            host: o.hosts.portal.to_ascii_lowercase(),
        })
        .collect()
}

/// The host registry's seed (T-7129 §1): every host of its own each `[[organizations]]` entry
/// names (whole-host mounts, Apps domain) and each `[[hosts]]` row, lowercase without a port,
/// with its role. The Portal inserts only hosts without a row, so a removed host stays removed.
pub(crate) fn registry_seed(cfg: &crate::config::Config) -> Vec<SeedHost> {
    let bare = |h: &str| jc_router::registry::bare(h).to_ascii_lowercase();
    let id = |name: &str| {
        cfg.organizations
            .iter()
            .find(|o| o.name == name)
            .and_then(|o| o.id.as_ref().map(|id| id.as_str().to_owned()))
    };
    let mut out: Vec<SeedHost> = Vec::new();
    let mut add = |host: &str, organization: &str, role: &str| {
        let host = bare(host);
        if !out.iter().any(|s| s.host == host) {
            out.push(SeedHost {
                host,
                organization: organization.to_owned(),
                organization_id: id(organization),
                role: role.to_owned(),
            });
        }
    };
    for org in &cfg.organizations {
        for m in org.hosts.by_module() {
            if m.bound && m.prefix.is_none() {
                add(
                    m.host,
                    &org.name,
                    m.role.map_or("portal-site", |r| r.as_str()),
                );
            }
        }
        if let Some(apps) = &org.hosts.apps {
            add(apps, &org.name, jc_router::HostRole::Apps.as_str());
        }
    }
    for row in &cfg.hosts {
        add(&row.host, &row.organization, "host");
    }
    out
}

/// The realm of each Organization with a sign-in host of its own (`sign_ins`: jc.toml's
/// `hosts.idm` at start, T-7001; the host registry's at run time, T-7180), where the Portal signs
/// that Organization's people in on its host (`JC_PORTAL_ORGANIZATION_REALMS`, T-7133): issuer
/// `{scheme}://{idm}/realms/{name}` as kaisatsu names it there, keys and tokens at `http://{idm}`
/// dialed at jc's listener (`JC_PORTAL_REACH`), where kaisatsu answers the realm's back channel.
/// Only beside the installation realm jc filled into `vars`, and never over an environment that
/// names the realms or its own reach. The address the realms' hosts are reached at when jc
/// fills them, so the registry's later sets go to the Portal too; `None` when it does not.
fn organization_realm_vars(
    sign_ins: &[super::sign_in::SignIn],
    siblings: &crate::siblings::Siblings,
    scheme: &str,
    vars: &mut BTreeMap<String, String>,
    env: impl Fn(&str) -> Option<String>,
) -> Option<std::net::SocketAddr> {
    if siblings.get(Module::Kaisatsu).is_none()
        || !vars.contains_key("JC_OIDC_ISSUER")
        || env("JC_PORTAL_ORGANIZATION_REALMS").is_some()
    {
        return None;
    }
    if env("JC_PORTAL_REACH").is_some() || env("JC_PORTAL_REACH_HOSTS").is_some() {
        if !sign_ins.is_empty() {
            tracing::warn!(
                "portal: JC_PORTAL_REACH is set, so no Organization signs in through its own \
                 realm; set JC_PORTAL_ORGANIZATION_REALMS beside it"
            );
        }
        return None;
    }
    let reach = vars
        .get("JC_PORTAL_REACH")
        .and_then(|at| at.parse().ok())
        .unwrap_or_else(|| crate::siblings::reach(siblings.listener));
    if sign_ins.is_empty() {
        return Some(reach);
    }
    vars.insert(
        "JC_PORTAL_ORGANIZATION_REALMS".to_owned(),
        organization_realms(sign_ins, scheme),
    );
    vars.entry("JC_PORTAL_REACH".to_owned())
        .or_insert_with(|| reach.to_string());
    let reached = vars.entry("JC_PORTAL_REACH_HOSTS".to_owned()).or_default();
    for own in sign_ins {
        if !reached.is_empty() {
            reached.push(',');
        }
        reached.push_str(&own.host);
    }
    Some(reach)
}

/// `JC_PORTAL_ORGANIZATION_REALMS` of `sign_ins`, `[]` for none.
fn organization_realms(sign_ins: &[super::sign_in::SignIn], scheme: &str) -> String {
    let realms: Vec<serde_json::Value> = sign_ins
        .iter()
        .map(|own| {
            let (host, name) = (&own.host, &own.organization);
            serde_json::json!({
                "organization": name,
                "issuer": format!("{scheme}://{host}/realms/{name}"),
                "jwks_url": format!("http://{host}/realms/{name}/protocol/openid-connect/certs"),
            })
        })
        .collect();
    serde_json::Value::from(realms).to_string()
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

/// Every address of each `[[organizations]]` row as the host registry routes it (API/15 §2,
/// T-7013), for the Organization page's Addresses: its own hosts, the `[[hosts]]` bound to it
/// and, while the base host is shared, its `/{id}/` path there (its name until an id is set).
/// Where its people sign in the Portal adds from its own issuer.
fn organization_addresses(cfg: &crate::config::Config) -> serde_json::Value {
    let scheme = &cfg.scheme;
    let at = |kind: &str, host: &str, path: &str| serde_json::json!({ "kind": kind, "url": format!("{scheme}://{host}{path}") });
    let bare = |host: &str| host.split(':').next().unwrap_or(host).to_owned();
    let base = bare(&cfg.base_host);
    let base_bound = cfg.hosts.iter().any(|row| bare(&row.host) == base)
        || cfg.organizations.iter().any(|org| {
            org.hosts
                .by_module()
                .iter()
                .any(|m| m.bound && bare(m.host) == base)
        });
    let orgs: Vec<serde_json::Value> = cfg
        .organizations
        .iter()
        .map(|org| {
            let h = &org.hosts;
            // The router's first segment on a shared host: the id once set, else the name.
            let segment = org.id.as_ref().map_or(org.name.as_str(), |id| id.as_str());
            let root = if h.shared {
                format!("/{segment}/")
            } else {
                "/".to_owned()
            };
            let opendata = match &h.opendata {
                Some(host) => at("opendata", host, &root),
                None => at(
                    "opendata",
                    &h.portal,
                    &format!("{}/", crate::config::DATAROOM_ON_WORKING_HOST),
                ),
            };
            // Without an API host of its own the gateway answers on the working host's paths.
            let (api_host, api_root, api_path) = match &h.api {
                Some(host) => (host.as_str(), root.clone(), root.clone()),
                None => (
                    h.portal.as_str(),
                    "/".to_owned(),
                    "/api/endpoint/".to_owned(),
                ),
            };
            let mcp = match h.mcp.as_deref().filter(|m| Some(*m) != h.api.as_deref()) {
                Some(host) => at("mcp", host, "/api/mcp/"),
                None => at("mcp", api_host, &format!("{api_root}api/mcp/")),
            };
            let api = at("api", api_host, &api_path);
            let mut addresses = vec![at("portal", &h.portal, "/"), opendata, api, mcp];
            addresses.extend(h.portals.iter().map(|p| at("portal-site", p, "/")));
            addresses.extend(
                cfg.hosts
                    .iter()
                    .filter(|row| row.organization == org.name)
                    .map(|row| {
                        let root = row.root.as_deref().unwrap_or("/");
                        let root = if root.ends_with('/') {
                            root.to_owned()
                        } else {
                            format!("{root}/")
                        };
                        at("host", &row.host, &root)
                    }),
            );
            if !base_bound {
                addresses.push(at("shared", &cfg.base_host, &format!("/{segment}/")));
            }
            serde_json::json!({
                "organization": org.name,
                "organizationId": org.id.as_ref().map(|id| id.as_str()),
                "addresses": addresses,
            })
        })
        .collect();
    serde_json::Value::Array(orgs)
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
    // The host registry's rules at the operator's door (T-7129 §1): the same two hosts the
    // seed's check holds against.
    let bare = |h: &str| jc_router::registry::bare(h).to_ascii_lowercase();
    out.insert(
        "JC_PORTAL_INSTALLATION_DOMAIN".to_owned(),
        bare(&cfg.base_host),
    );
    out.insert("JC_PORTAL_LOGIN_HOST".to_owned(), bare(&cfg.login_host));
    out.insert(
        "JC_PORTAL_ORGANIZATION_ADDRESSES".to_owned(),
        organization_addresses(cfg).to_string(),
    );
    // ODP-08, ODP-09: the hosts a Portal manifest may take, and the deployment's cap.
    let portal_hosts: Vec<String> = cfg
        .organizations
        .iter()
        .flat_map(|org| {
            org.hosts
                .portals
                .iter()
                .map(|h| format!("{}={h}", org.name))
        })
        .collect();
    out.insert("JC_PORTAL_PORTAL_HOSTS".to_owned(), portal_hosts.join(","));
    out.insert(
        "JC_PORTAL_HARVEST_HOSTS".to_owned(),
        cfg.dataroom.harvest_hosts().join(","),
    );
    out.insert(
        "JC_PORTAL_PORTALS_MAX".to_owned(),
        cfg.dataroom
            .portals_max(cfg.organizations.len())
            .to_string(),
    );
    if cfg.roles.contains(&Module::Dataroom) {
        let reach = crate::siblings::reach(listener);
        let catalogues: Vec<serde_json::Value> = cfg
            .organizations
            .iter()
            .filter_map(|org| {
                let token = org.portal_token_file.as_ref()?;
                // The gateway links a dataset's resources; off https a whole origin.
                let data_host = match scheme.as_str() {
                    "https" => org.api_base(),
                    _ => format!("{scheme}://{}", org.api_base()),
                };
                // The Portal writes into the catalogue on the working host's `/data`: the open
                // data host answers reads only (T-6991 §1).
                let writes = format!(
                    "{}{}",
                    org.hosts.portal,
                    crate::config::DATAROOM_ON_WORKING_HOST
                );
                Some(serde_json::json!({
                    "organization": org.name,
                    "url": format!("{scheme}://{}", org.opendata_base()),
                    "api": format!("http://{writes}"),
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

/// `JC_PORTAL_BACKUP_TENANTS` (T-7169): the tenant of each module running here as its data
/// backup reader sets it for row-level security: shako's one `[instance, org]`, daiya's
/// installation (each project's daiya organization derives from it) and each Organization's
/// dataroom OrgId and catalogue. Empty when none of them runs here; an environment value still
/// wins.
pub(crate) fn backup_vars(cfg: &crate::config::Config) -> BTreeMap<String, String> {
    let mut tenants = serde_json::Map::new();
    #[cfg(feature = "shako")]
    if cfg.roles.contains(&Module::Shako) {
        tenants.insert(
            "shako".to_owned(),
            serde_json::json!([
                crate::modules::shako::INSTANCE.to_string(),
                crate::modules::shako::ORG.to_string()
            ]),
        );
    }
    #[cfg(feature = "daiya")]
    if cfg.roles.contains(&Module::Daiya) {
        // DaiyaState::for_standalone's installation, the one jc's daiya runs as.
        tenants.insert(
            "daiyaInstallation".to_owned(),
            daiya_model::InstallationId::from_u128(1).to_string().into(),
        );
    }
    #[cfg(feature = "dataroom")]
    if cfg.roles.contains(&Module::Dataroom) {
        use dataroom_core::{CatalogueId, OrgId};
        // A malformed id stops dataroom's own start with the reason; here it names no tenant.
        if let Ok(installation) = OrgId::parse_verified(&cfg.dataroom.org_id) {
            let own: serde_json::Map<String, serde_json::Value> = cfg
                .organizations
                .iter()
                .filter_map(|org| {
                    let catalogue = CatalogueId::parse_verified(&org.catalogue_id).ok()?;
                    Some((
                        org.name.clone(),
                        serde_json::json!({
                            "org": OrgId::of_organization(installation, &org.name).to_string(),
                            "catalogue": catalogue.to_string(),
                        }),
                    ))
                })
                .collect();
            tenants.insert("dataroom".to_owned(), own.into());
        }
    }
    let mut out = BTreeMap::new();
    if !tenants.is_empty() {
        out.insert(
            "JC_PORTAL_BACKUP_TENANTS".to_owned(),
            serde_json::Value::Object(tenants).to_string(),
        );
    }
    out
}

/// `JC_PORTAL_APPS_URL`, the apex every App host sits under, from the Portal's App-host entry
/// `*.apps.{domain}` (edge-session-design S4a): `{scheme}://{domain}`, and
/// `JC_PORTAL_SERVE_APP_HOSTS`, since no edge composes the App routes in front of jc. Empty
/// without one; an environment value still wins.
/// The removal hold `[portal]` sets (T-6708). The reconciler's state directory is not derived
/// from `state_dir`, which a cluster mounts as a small memory volume: the deployment names a
/// directory on its data volume with `JC_PORTAL_RECONCILER_STATE_DIR`.
pub(crate) fn reconciler_vars(cfg: &crate::config::Config) -> BTreeMap<String, String> {
    cfg.portal
        .removal_hold_percent
        .map(|percent| {
            (
                "JC_PORTAL_REMOVAL_HOLD_PERCENT".to_owned(),
                percent.to_string(),
            )
        })
        .into_iter()
        .collect()
}

/// `JC_PORTAL_INGEST_URL`, where a webhook DataSource's sender posts (PL-122): the ekiben entry
/// at [`crate::module::INGEST_PREFIX`] on a host, `{scheme}://{host}/ingest/default`, since
/// every stream of jc's runner belongs to its default organization. Empty without one, so the
/// Portal shows no address.
pub(crate) fn ingest_vars(cfg: &crate::config::Config) -> BTreeMap<String, String> {
    cfg.mounts
        .get(&Module::Ekiben)
        .into_iter()
        .flatten()
        .filter(|entry| entry.path_prefix.as_deref() == Some(crate::module::INGEST_PREFIX))
        .find_map(|entry| entry.host.as_deref())
        .map(|host| {
            (
                "JC_PORTAL_INGEST_URL".to_owned(),
                format!(
                    "{}://{}{}/default",
                    cfg.scheme,
                    host.trim().to_ascii_lowercase(),
                    crate::module::INGEST_PREFIX
                ),
            )
        })
        .into_iter()
        .collect()
}

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

/// `JC_PORTAL_ORGANIZATION_BOUNDS`, the deployment's limit ceilings of `[portal.limits]` as JSON
/// (LIM-01, T-6716); empty without the section, so the Portal keeps its built-in ceilings. An
/// environment value still wins, and the Portal refuses to start when the bounds file is set too.
/// With `[kaisatsu] second_factor = "required"` also `JC_PORTAL_SECOND_FACTOR_FLOOR`, so
/// Organization settings shows the floor every Organization inherits (T-6895).
pub(crate) fn limits_vars(cfg: &crate::config::Config) -> BTreeMap<String, String> {
    let mut vars = BTreeMap::new();
    if cfg.kaisatsu.second_factor == crate::config::SecondFactorFloor::Required {
        vars.insert(
            "JC_PORTAL_SECOND_FACTOR_FLOOR".to_owned(),
            "required".to_owned(),
        );
    }
    if !cfg.portal.limits.is_empty() {
        // A map of strings to two optional integers always serializes.
        let json = serde_json::to_string(&cfg.portal.limits).unwrap_or_default();
        vars.insert("JC_PORTAL_ORGANIZATION_BOUNDS".to_owned(), json);
    }
    vars
}

/// The Portal's router behind the host jc placed the request on (`Mount`, a trusted proxy's
/// `X-Forwarded-Host` applied) and the Organization jc's host registry decided for it, which
/// pin the request to that Organization (`organization_host`).
pub(crate) fn with_request_host(router: Router) -> Router {
    router.layer(axum::middleware::from_fn(
        |mut req: axum::extract::Request, next: axum::middleware::Next| async move {
            let host = req.extensions().get::<Mount>().map(|m| m.host.clone());
            if let Some(host) = host {
                req.extensions_mut()
                    .insert(joinedcontext_portal::organization_host::RequestHost(host));
            }
            let org = req.extensions().get::<jc_router::Organization>().cloned();
            if let Some(org) = org {
                req.extensions_mut()
                    .insert(joinedcontext_portal::organization_host::RequestOrganization(org.0));
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
    /// T-7169: the Portal's data backup sets each module's tenant as the module itself keys its
    /// rows: shako's forge tenant, daiya's installation and each Organization's own dataroom
    /// OrgId and catalogue, in the shape the Portal reads.
    #[test]
    #[cfg(all(feature = "shako", feature = "daiya", feature = "dataroom"))]
    fn the_backup_tenants_are_the_ones_the_modules_key_rows_by() {
        use clap::Parser as _;
        let text = include_str!("../../tests/fixtures/two-orgs.toml");
        let file = crate::config::Config::parse_file(std::path::Path::new("jc.toml"), text)
            .expect("parses");
        let cli =
            |roles: &str| crate::config::Cli::try_parse_from(["jc", "--role", roles]).expect("cli");
        let cfg = crate::config::Config::merge(cli("portal,dataroom,shako,daiya"), file.clone())
            .expect("config");
        let vars = backup_vars(&cfg);
        let read = joinedcontext_portal::org_backup::modules::BackupModules::from_lookup(|k| {
            vars.get(k).cloned()
        })
        .expect("the Portal reads what jc writes");
        let tenants = read.tenants;
        let one = "00000000-0000-0000-0000-000000000001".to_owned();
        assert_eq!(
            tenants.shako.map(|pair| pair.map(|id| id.to_string())),
            Some([one.clone(), one.clone()])
        );
        assert_eq!(
            tenants.daiya_installation.map(|id| id.to_string()),
            Some(one)
        );
        let installation = dataroom_core::OrgId::parse_verified(&cfg.dataroom.org_id).unwrap();
        let alpha = &tenants.dataroom["alpha"];
        assert_eq!(
            alpha.org.to_string(),
            dataroom_core::OrgId::of_organization(installation, "alpha").to_string()
        );
        assert_eq!(
            alpha.catalogue.to_string(),
            "00000000-0000-7000-8000-0000000000a1"
        );
        assert_ne!(tenants.dataroom["beta"].org, alpha.org);
        let portal_only = crate::config::Config::merge(cli("portal"), file).expect("config");
        assert!(backup_vars(&portal_only).is_empty());
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
            Some("http://portal.alpha.test/data"),
            "the open data host takes no write: the Portal publishes on the working host"
        );
        assert!(parsed[0].reach.is_some_and(|r| r.ip().is_loopback()));
        assert_eq!(
            parsed[0].token_file,
            std::path::PathBuf::from("/run/alpha-token")
        );
        assert!(!vars["JC_PORTAL_ORGANIZATION_CATALOGUES"].contains("beta"));
        // T-7013: every address the Portal lists under Addresses, read back by its own parser.
        let addresses = joinedcontext_portal::organization_host::parse_addresses(
            &vars["JC_PORTAL_ORGANIZATION_ADDRESSES"],
        )
        .expect("the Portal reads the addresses");
        let alpha: Vec<String> = addresses[0]
            .addresses
            .iter()
            .map(|a| a.url.to_string())
            .collect();
        let s = &cfg.scheme;
        assert_eq!(
            alpha,
            [
                format!("{s}://portal.alpha.test/"),
                format!("{s}://opendata.alpha.test/"),
                format!("{s}://api.alpha.test/"),
                format!("{s}://mcp.alpha.test/api/mcp/"),
                format!("{s}://{}/alpha/", cfg.base_host),
            ]
        );
        assert_eq!(addresses[1].organization, "beta");
        // With an id the shared path is the id (EP-186).
        let mut with_id = cfg.clone();
        with_id.organizations[0].id =
            Some(jc_core::names::OrganizationId::parse("test_alpha").expect("an id"));
        let parsed = joinedcontext_portal::organization_host::parse_addresses(
            &organization_addresses(&with_id).to_string(),
        )
        .expect("parses");
        assert_eq!(parsed[0].organization_id.as_deref(), Some("test_alpha"));
        assert!(
            parsed[0].addresses[4].url.path() == "/test_alpha/",
            "{:?}",
            parsed[0].addresses
        );
        // `host = "auto"`: the shared open data and API hosts carry the id as the first segment;
        // with the working host alone, the open data and the gateway are its paths.
        let urls = |cfg: &crate::config::Config, i: usize| -> Vec<String> {
            joinedcontext_portal::organization_host::parse_addresses(
                &organization_addresses(cfg).to_string(),
            )
            .expect("parses")[i]
                .addresses
                .iter()
                .take(4)
                .map(|a| a.url.to_string())
                .collect()
        };
        with_id.organizations[0].hosts = crate::config::OrganizationHosts {
            portal: "alpha.base.test".into(),
            opendata: Some("opendata.base.test".into()),
            api: Some("api.base.test".into()),
            shared: true,
            ..crate::config::OrganizationHosts::default()
        };
        with_id.organizations[1].hosts = crate::config::OrganizationHosts {
            portal: "portal.beta.test".into(),
            ..crate::config::OrganizationHosts::default()
        };
        assert_eq!(
            urls(&with_id, 0),
            [
                format!("{s}://alpha.base.test/"),
                format!("{s}://opendata.base.test/test_alpha/"),
                format!("{s}://api.base.test/test_alpha/"),
                format!("{s}://api.base.test/test_alpha/api/mcp/"),
            ]
        );
        assert_eq!(
            urls(&with_id, 1),
            [
                format!("{s}://portal.beta.test/"),
                format!("{s}://portal.beta.test/data/"),
                format!("{s}://portal.beta.test/api/endpoint/"),
                format!("{s}://portal.beta.test/api/mcp/"),
            ]
        );
        // Without dataroom here, the hosts alone; without Organizations, nothing.
        let mut portal_only = crate::config::Config::merge(cli("portal"), file).expect("config");
        portal_only.organizations[0].portal_token_file = Some("/run/alpha-token".into());
        let vars = organization_vars(&portal_only, listener);
        assert!(!vars.contains_key("JC_PORTAL_ORGANIZATION_CATALOGUES"));
        portal_only.organizations.clear();
        assert!(organization_vars(&portal_only, listener).is_empty());
    }

    /// T-7133: an Organization with `hosts.idm` gets its realm in the Portal's settings, its
    /// sign-in host dialed at jc's listener beside the hosts jc already reaches; one without gets
    /// none, and neither does a Portal whose realm or reach the environment named.
    /// T-7135 (T-7129 §1): the registry's seed is every host of an Organization's own with its
    /// role, once, lowercase and without a port; `auto`'s shared hosts and the working host's
    /// paths are no Organization's row.
    #[test]
    fn the_registry_seed_lists_each_own_host_once_with_its_role() {
        let text = "base_host = \"city.example\"\n\
            [[organizations]]\nname = \"bbsk\"\nid = \"sk_bbsk\"\ncatalogue_id = \"00000000-0000-7000-8000-0000000000a1\"\n\
            hosts = { portal = \"Smartcity.bbsk.sk:8443\", api = \"api.bbsk.sk\", mcp = \"api.bbsk.sk\", idm = \"idm.bbsk.sk\", apps = \"bbsk-apps.example\", portals = [\"mobility.bbsk.sk\"] }\n\
            [[organizations]]\nname = \"zilina\"\ncatalogue_id = \"00000000-0000-7000-8000-0000000000a2\"\nhost = \"auto\"\n\
            [[hosts]]\nhost = \"data.bbsk.sk\"\norganization = \"bbsk\"\n";
        let file = crate::config::Config::parse_file(std::path::Path::new("jc.toml"), text)
            .expect("parses");
        let cfg = crate::config::Config::merge(crate::config::Cli::default(), file).expect("cfg");
        let rows = registry_seed(&cfg);
        let seed: Vec<(&str, &str, Option<&str>, &str)> = rows
            .iter()
            .map(|s| {
                (
                    s.host.as_str(),
                    s.organization.as_str(),
                    s.organization_id.as_deref(),
                    s.role.as_str(),
                )
            })
            .collect();
        assert_eq!(
            seed,
            [
                ("smartcity.bbsk.sk", "bbsk", Some("sk_bbsk"), "working"),
                ("api.bbsk.sk", "bbsk", Some("sk_bbsk"), "api"),
                ("mobility.bbsk.sk", "bbsk", Some("sk_bbsk"), "portal-site"),
                ("idm.bbsk.sk", "bbsk", Some("sk_bbsk"), "signin"),
                ("bbsk-apps.example", "bbsk", Some("sk_bbsk"), "apps"),
                ("zilina.city.example", "zilina", None, "working"),
                ("data.bbsk.sk", "bbsk", Some("sk_bbsk"), "host"),
            ]
        );
    }

    /// T-7184 (and T-7181's removal): the registry's rows mount each module on a host jc.toml
    /// did not mount, so a run-time Organization's working and API hosts answer its Portal and
    /// gateway, and the gateway links its own API host; once the Organization is removed its hosts
    /// answer 421 and the gateway links it no more. jc.toml's host keeps its own entry.
    #[tokio::test]
    async fn a_run_time_organizations_hosts_answer_its_modules_and_a_removed_one_421() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use joinedcontext_portal::host_registry::{HostRow, OrganizationRow};
        let answer = |name: &'static str| {
            Router::new().fallback(
                move |org: Option<axum::Extension<jc_router::Organization>>| async move {
                    format!("{name}|{}", org.map_or_else(String::new, |o| o.0.0))
                },
            )
        };
        let table = jc_router::HostTable::new()
            .organization("bbsk")
            .unwrap()
            .bound("smartcity.bbsk.sk", "bbsk", "/")
            .unwrap();
        let spec = |module: &str, host: &str| jc_router::MountSpec {
            module: module.into(),
            host: Some(host.into()),
            path_prefix: None,
        };
        let jc = jc_router::JcRouterBuilder::new("https", "city.example")
            .hosts(table)
            .module_router("portal", answer("portal"))
            .module_router("gateway", answer("gateway"))
            .mount(spec("portal", "smartcity.bbsk.sk"), answer("bbsk-portal"))
            .build()
            .unwrap();
        let run = Run {
            hosts: jc.host_swap().expect("a host table"),
            roles: vec![Module::Portal, Module::Gateway],
            named: vec!["bbsk".into()],
            scheme: "https".into(),
            portal: None,
        };
        let host = |host: &str, organization: &str, role: &str| HostRow {
            host: host.into(),
            organization: organization.into(),
            organization_id: None,
            role: role.into(),
            state: "served".into(),
        };
        let rows = [
            host("smartcity.bbsk.sk", "bbsk", "working"),
            host("smartcity.rt7184.sk", "rt7184", "working"),
            host("api.rt7184.sk", "rt7184", "api"),
        ];
        let org = |name: &str, state: &str| OrganizationRow {
            name: name.into(),
            organization_id: None,
            host: format!("smartcity.{name}.sk"),
            state: state.into(),
        };
        let get = |host: &'static str, path: &'static str| {
            let jc = jc.clone();
            async move {
                let req = axum::http::Request::builder()
                    .uri(path)
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap();
                let resp = jc.dispatch(req, Some("127.0.0.1".parse().unwrap())).await;
                let status = resp.status();
                let body = resp.into_body().collect().await.unwrap().to_bytes();
                (status, String::from_utf8(body.to_vec()).unwrap())
            }
        };
        let ok = |body: &str| (axum::http::StatusCode::OK, body.to_owned());
        assert_eq!(
            get("smartcity.rt7184.sk", "/").await.0,
            axum::http::StatusCode::MISDIRECTED_REQUEST
        );
        let served = apply(
            &run,
            &rows,
            &[org("bbsk", "active"), org("rt7184", "active")],
        );
        assert_eq!(served, Ok(3));
        assert_eq!(get("smartcity.rt7184.sk", "/").await, ok("portal|rt7184"));
        assert_eq!(
            get("smartcity.rt7184.sk", "/api/endpoint/x").await,
            ok("gateway|rt7184"),
            "the gateway's paths on the working host"
        );
        assert_eq!(get("api.rt7184.sk", "/x").await, ok("gateway|rt7184"));
        assert_eq!(get("smartcity.bbsk.sk", "/").await, ok("bbsk-portal|bbsk"));
        let links = super::super::gateway::registry_links();
        assert_eq!(
            links
                .get("rt7184")
                .map(|(api, own)| (api.as_str(), own.len())),
            Some(("https://api.rt7184.sk", 1))
        );
        assert!(!links.contains_key("bbsk"), "jc.toml links its own");
        let served = apply(
            &run,
            &rows,
            &[org("bbsk", "active"), org("rt7184", "removed")],
        );
        assert_eq!(served, Ok(1));
        for at in ["smartcity.rt7184.sk", "api.rt7184.sk"] {
            assert_eq!(
                get(at, "/").await.0,
                axum::http::StatusCode::MISDIRECTED_REQUEST,
                "{at}"
            );
        }
        assert!(!super::super::gateway::registry_links().contains_key("rt7184"));
    }

    /// T-7181: the Organization registry's seed is each Organization's name, id and working
    /// host; the sign-in set follows the registry's active rows, never jc.toml's names.
    #[test]
    fn the_organization_seed_is_name_id_and_working_host_and_only_active_rows_run() {
        let text = "base_host = \"city.example\"\n\
            [[organizations]]\nname = \"bbsk\"\nid = \"sk_bbsk\"\ncatalogue_id = \"00000000-0000-7000-8000-0000000000a1\"\n\
            hosts = { portal = \"Smartcity.bbsk.sk:8443\" }\n\
            [[organizations]]\nname = \"zilina\"\ncatalogue_id = \"00000000-0000-7000-8000-0000000000a2\"\nhost = \"auto\"\n";
        let file = crate::config::Config::parse_file(std::path::Path::new("jc.toml"), text)
            .expect("parses");
        let cfg = crate::config::Config::merge(crate::config::Cli::default(), file).expect("cfg");
        assert_eq!(
            organization_seed(&cfg),
            [
                SeedOrganization {
                    name: "bbsk".into(),
                    organization_id: Some("sk_bbsk".into()),
                    host: "smartcity.bbsk.sk:8443".into(),
                },
                SeedOrganization {
                    name: "zilina".into(),
                    organization_id: None,
                    host: "zilina.city.example".into(),
                },
            ]
        );
        let row = |name: &str, state: &str| joinedcontext_portal::host_registry::OrganizationRow {
            name: name.into(),
            organization_id: None,
            host: format!("smartcity.{name}.sk"),
            state: state.into(),
        };
        assert_eq!(
            active(&[row("bbsk", "removed"), row("run-time", "active")]),
            ["run-time"]
        );
    }

    #[test]
    fn an_organization_with_its_own_sign_in_host_gets_its_realm_in_the_portal() {
        let text = "base_host = \"city.example\"\n\
            [[organizations]]\nname = \"bbsk\"\ncatalogue_id = \"00000000-0000-7000-8000-0000000000a1\"\n\
            hosts = { portal = \"smartcity.bbsk.sk\", idm = \"idm.bbsk.sk\" }\n\
            [[organizations]]\nname = \"zilina\"\ncatalogue_id = \"00000000-0000-7000-8000-0000000000a2\"\n\
            hosts = { portal = \"smartcity.zilina.sk\" }\n";
        let file = crate::config::Config::parse_file(std::path::Path::new("jc.toml"), text)
            .expect("parses");
        let cfg = crate::config::Config::merge(crate::config::Cli::default(), file).expect("cfg");
        let mut siblings = crate::siblings::Siblings::new(&[], "0.0.0.0:4000".parse().unwrap());
        siblings.insert_remote(
            Module::Kaisatsu,
            crate::siblings::Sibling::direct(
                "https://idm.city.example".into(),
                "http://idm.city.example".into(),
            ),
        );
        let installation = || {
            BTreeMap::from([
                (
                    "JC_OIDC_ISSUER".to_owned(),
                    "https://idm.city.example/realms/jc".to_owned(),
                ),
                ("JC_OIDC_CLIENT_ID".to_owned(), "portal-api".to_owned()),
                ("JC_OIDC_CLIENT_SECRET".to_owned(), "s".to_owned()),
                (
                    "JC_PORTAL_COOKIE_KEY".to_owned(),
                    "cookie-key-of-the-jc-tests-it-seals-no-session-anybody-uses-000001".to_owned(),
                ),
                ("JC_PORTAL_REACH".to_owned(), "127.0.0.1:4000".to_owned()),
                (
                    "JC_PORTAL_REACH_HOSTS".to_owned(),
                    "idm.city.example".to_owned(),
                ),
                (
                    "JC_PORTAL_ORGANIZATION_HOSTS".to_owned(),
                    "bbsk=https://smartcity.bbsk.sk,zilina=https://smartcity.zilina.sk".to_owned(),
                ),
            ])
        };
        let none = |_: &str| None;
        let mut vars = installation();
        let sign_ins = crate::modules::sign_in::from_config(&cfg.organizations);
        assert_eq!(
            organization_realm_vars(&sign_ins, &siblings, "https", &mut vars, none),
            Some("127.0.0.1:4000".parse().unwrap()),
            "the registry's later sets reach the Portal too"
        );
        assert_eq!(
            vars["JC_PORTAL_REACH_HOSTS"],
            "idm.city.example,idm.bbsk.sk"
        );
        let mount = Mount::new("portal", "https", "city.example", "/portal").unwrap();
        let portal = config(&mount, none, vars).expect("the Portal takes the realms");
        let realms = &portal.organization_realms;
        assert_eq!(realms.len(), 1, "zilina has no sign-in host of its own");
        assert_eq!(realms[0].organization, "bbsk");
        assert_eq!(realms[0].issuer.as_str(), "https://idm.bbsk.sk/realms/bbsk");
        assert_eq!(
            realms[0].jwks_url.as_str(),
            "http://idm.bbsk.sk/realms/bbsk/protocol/openid-connect/certs"
        );
        assert!(portal.reach.iter().any(|(host, at)| host == "idm.bbsk.sk"
            && *at == "127.0.0.1:4000".parse::<std::net::SocketAddr>().unwrap()));

        for set in ["JC_PORTAL_ORGANIZATION_REALMS", "JC_PORTAL_REACH"] {
            let mut vars = installation();
            let env = |key: &str| (key == set).then(|| "[]".to_owned());
            assert_eq!(
                organization_realm_vars(&sign_ins, &siblings, "https", &mut vars, env),
                None
            );
            assert!(!vars.contains_key("JC_PORTAL_ORGANIZATION_REALMS"), "{set}");
        }
        let mut operator_realm = installation();
        operator_realm.remove("JC_OIDC_ISSUER");
        organization_realm_vars(&sign_ins, &siblings, "https", &mut operator_realm, none);
        assert!(!operator_realm.contains_key("JC_PORTAL_ORGANIZATION_REALMS"));
    }
}
