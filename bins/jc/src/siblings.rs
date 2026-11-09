//! Where the Portal finds its sibling modules inside `jc`, and the Portal settings `jc` fills from
//! them.
//!
//! Each sibling has two addresses:
//! - `public`: its mount's public base, the address a browser and a token issuer use;
//! - `internal`: the address the Portal calls it at. In slug mode (no host of its own) that is
//!   `jc`'s own listener on loopback plus the slug, so a sibling call never leaves the host; a
//!   module mounted on a host of its own is called at its public base (the host routes by name).
//!
//! A setting comes from a sibling only when the sibling is in the role set and the environment
//! does not set it; an explicit value always wins. A sibling's credential is filled only together
//! with the sibling's URL that `jc` filled, so a generated token is never sent to an address the
//! operator configured for something else.

use crate::app::MountPlan;
use crate::module::Module;
use crate::setup::{self, Secret, Setup};
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// The two addresses of one mounted module, without a trailing slash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sibling {
    pub public: String,
    pub internal: String,
}

/// Every mounted module's addresses, plus `jc`'s own listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Siblings {
    pub listener: SocketAddr,
    modules: BTreeMap<Module, Sibling>,
    /// The Portal's public base on each `[[organizations]]` host, beside its own mount's: what
    /// the realm registers a redirect URI for.
    pub organization_portals: Vec<String>,
}

/// The loopback address that reaches a listener bound to `addr` (an unspecified bind address is
/// reached on loopback of its family).
#[must_use]
pub fn reach(addr: SocketAddr) -> SocketAddr {
    let ip = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, addr.port())
}

impl Siblings {
    /// Addresses of every planned module, given the listener `jc` bound.
    #[must_use]
    pub fn new(plans: &[MountPlan], listener: SocketAddr) -> Self {
        let reach = reach(listener);
        let modules = plans
            .iter()
            .map(|plan| {
                let public = plan.mount.public_base();
                let internal = match plan.spec.host {
                    None => format!("http://{reach}{}", plan.mount.path_prefix),
                    Some(_) => public.clone(),
                };
                (plan.module, Sibling { public, internal })
            })
            .collect();
        Self {
            listener: reach,
            modules,
            organization_portals: Vec::new(),
        }
    }

    /// Adds a module that runs in another container: `public` is its mount there, `internal` the
    /// loopback forwarder that carries the Portal's calls to it ([`crate::door::Remote`]).
    pub fn insert_remote(&mut self, module: Module, sibling: Sibling) {
        self.modules.insert(module, sibling);
    }

    /// Adds a module that serves a loopback listener of its own and no public mount: both its
    /// addresses are that listener.
    pub fn insert_loopback(&mut self, module: Module, addr: SocketAddr) {
        let url = format!("http://{}", reach(addr));
        self.modules.insert(
            module,
            Sibling {
                public: url.clone(),
                internal: url,
            },
        );
    }

    /// Points a mounted module's internal address at a loopback listener of its own, keeping its
    /// public mount (the assistant's administration paths answer only there).
    pub fn set_internal(&mut self, module: Module, addr: SocketAddr) {
        if let Some(sibling) = self.modules.get_mut(&module) {
            sibling.internal = format!("http://{}", reach(addr));
        }
    }

    /// Drops a module that did not start, so no sibling setting points at it.
    pub fn remove(&mut self, module: Module) {
        self.modules.remove(&module);
    }

    #[must_use]
    pub fn get(&self, module: Module) -> Option<&Sibling> {
        self.modules.get(&module)
    }

    /// The Portal's public base with a trailing slash, as `JC_PORTAL_PUBLIC_URL` holds it.
    #[must_use]
    pub fn portal_base(&self) -> Option<String> {
        self.get(Module::Portal).map(|p| format!("{}/", p.public))
    }
}

/// The catalogue's live parts as `jc` places them ([`crate::config::DataroomAddresses`]): each a
/// base without a trailing slash, or "" when nobody serves it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveAddresses {
    pub viz: String,
    pub api: String,
    pub tiles: String,
    pub assistant: String,
    pub portal: String,
    /// The default catalogue's public base (dataroom's mount).
    pub catalogue: String,
}

/// The addresses `cfg` names, the others from the mounted modules: viz, the gateway's origin (it
/// answers `/api/endpoint` and `/api/mcp` at the root of its host), the Portal and dataroom. No
/// module serves tiles or the assistant, so those come from `cfg` alone.
#[must_use]
pub fn live_addresses(
    cfg: &crate::config::DataroomAddresses,
    siblings: &Siblings,
) -> LiveAddresses {
    let pick =
        |named: &Option<String>, from: Option<String>| named.clone().or(from).unwrap_or_default();
    let public = |m: Module| siblings.get(m).map(|s| s.public.clone());
    LiveAddresses {
        viz: pick(&cfg.viz, public(Module::Viz)),
        api: pick(&cfg.api, public(Module::Gateway).map(|g| origin(&g))),
        tiles: pick(&cfg.tiles, None),
        assistant: pick(&cfg.assistant, None),
        portal: pick(&cfg.portal, public(Module::Portal)),
        catalogue: public(Module::Dataroom).unwrap_or_default(),
    }
}

/// `http(s)://host[:port][/path]` with a lowercase host, no user, query, fragment or trailing
/// slash: what a page may frame, link and name in its CSP (dataroom-core `is_base_url` holds the
/// same rule for a catalogue's own branding).
#[must_use]
pub fn is_base_url(v: &str) -> bool {
    let Some(rest) = v
        .strip_prefix("https://")
        .or_else(|| v.strip_prefix("http://"))
    else {
        return false;
    };
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    let label_ok = |l: &str| {
        !l.is_empty()
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    let host_ok = host.split('.').all(label_ok);
    let port_ok =
        port.is_none_or(|p| (1..=5).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()));
    let path_ok = !path.ends_with('/')
        && !path.contains("//")
        && !path.split('/').any(|s| s == "." || s == "..")
        && path
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"/-_.~".contains(&b));
    host_ok && port_ok && path_ok
}

/// The origin (`scheme://host:port`) of a URL string without a path.
pub(crate) fn origin(url: &str) -> String {
    match url.find("://") {
        Some(i) => {
            let rest = &url[i + 3..];
            let end = rest.find('/').unwrap_or(rest.len());
            format!("{}{}", &url[..i + 3], &rest[..end])
        }
        None => url.to_owned(),
    }
}

/// The batch-lane runs the Portal hands jc's own ekiben at once: the deployment's value on dev
/// and prod (components/portal development-values `JC_PORTAL_PIPELINES_BATCH_CONCURRENCY`,
/// T-5778). The Portal's default of one ran about four runs a minute against the seed's 5,400
/// batch runs a day, so a run asked now waited behind the queue for hours.
const PIPELINE_BATCH_RUNS: &str = "3";

/// The Portal settings `jc` fills from its siblings, for the keys `env` leaves unset.
///
/// | Portal setting | sibling |
/// |---|---|
/// | `JC_OIDC_ISSUER`, `JC_OIDC_CLIENT_*`, `JC_KEYCLOAK_ADMIN_URL`, `JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_*`, `JC_PORTAL_COOKIE_KEY` | kaisatsu |
/// | `JC_SHAKO_URL`, `_PUBLIC_URL`, `_OWNER`, `_REPO`, `_TOKEN`, `_APPS_OWNER`, `_APPS_TOKEN`, `_WEBHOOK_SECRET` | shako |
/// | `JC_PORTAL_PIPELINE_RUNNER_URL`, `JC_PORTAL_PIPELINE_RUNNER_TOKEN`, `JC_PORTAL_PIPELINES_BATCH_CONCURRENCY` (3) | ekiben |
/// | `JC_PORTAL_CKAN_URL`, `JC_PORTAL_CKAN_API_URL` (https, EP-67; http only with `JC_PORTAL_CKAN_ALLOW_HTTP=true`) | dataroom |
/// | `JC_PORTAL_GRAFANA_URL`, `_PUBLIC_URL`, `_USER`, `_PASSWORD` | daiya |
/// | `JC_PORTAL_BROKER_URL`, `JC_PORTAL_BROKER_ADMIN_TOKEN_FILE` | antares |
/// | `JC_PORTAL_GATEWAY_URL`, `JC_PORTAL_GATEWAY_CLIENT_ID` | gateway |
/// | `JC_PORTAL_MODEL_TOOLS_URL` | model-tools (its loopback listener) |
/// | `JC_PORTAL_KNOWLEDGE_URL` | assistant (its loopback listener; absent while the module is off) |
#[must_use]
pub fn portal_vars(
    siblings: &Siblings,
    setup: &Setup,
    env: impl Fn(&str) -> Option<String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let unset = |key: &str| env(key).is_none_or(|v| v.trim().is_empty());
    // A group of settings: filled only when its URL key is unset, and then every other key of
    // the group the environment leaves unset. A value is `None` when its secret is not held.
    let mut group = |url_key: &str, values: Vec<(&str, Option<String>)>| {
        if !unset(url_key) {
            return;
        }
        if values.iter().any(|(_, v)| v.is_none()) {
            return;
        }
        for (key, value) in values {
            if unset(key) {
                out.insert(key.to_owned(), value.unwrap_or_default());
            }
        }
    };
    let secret = |s: Secret| setup.value(s).map(str::to_owned);

    if let Some(id) = siblings.get(Module::Kaisatsu) {
        let mut values = vec![
            (
                "JC_OIDC_ISSUER",
                Some(format!("{}/realms/{}", id.public, setup.realm)),
            ),
            ("JC_OIDC_CLIENT_ID", Some(setup::PORTAL_CLIENT.to_owned())),
            ("JC_OIDC_CLIENT_SECRET", secret(Secret::PortalClientSecret)),
            ("JC_PORTAL_COOKIE_KEY", secret(Secret::PortalCookieKey)),
            (
                "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ID",
                Some(setup::PORTAL_ADMIN_CLIENT.to_owned()),
            ),
            (
                "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SECRET",
                secret(Secret::PortalAdminClientSecret),
            ),
        ];
        // The admin and token back channel goes to the listener when kaisatsu shares it
        // (`backchannel()` adds the issuer's path to this origin).
        if id.internal != id.public {
            values.push(("JC_KEYCLOAK_ADMIN_URL", Some(origin(&id.internal))));
        }
        group("JC_OIDC_ISSUER", values);
    }
    if let Some(git) = siblings.get(Module::Shako) {
        group(
            "JC_SHAKO_URL",
            vec![
                ("JC_SHAKO_URL", Some(git.internal.clone())),
                ("JC_SHAKO_PUBLIC_URL", Some(git.public.clone())),
                ("JC_SHAKO_OWNER", Some(setup::FORGE_OWNER.to_owned())),
                ("JC_SHAKO_REPO", Some(setup::FORGE_REPO.to_owned())),
                ("JC_SHAKO_TOKEN", secret(Secret::ShakoToken)),
                (
                    "JC_SHAKO_APPS_OWNER",
                    Some(setup::FORGE_APPS_OWNER.to_owned()),
                ),
                ("JC_SHAKO_APPS_TOKEN", secret(Secret::ShakoToken)),
                (
                    "JC_SHAKO_WEBHOOK_SECRET",
                    secret(Secret::ForgeWebhookSecret),
                ),
            ],
        );
    }
    if let Some(streams) = siblings.get(Module::Ekiben) {
        // ekiben refuses every call without its bearer: the Portal sends jc's ekiben token,
        // handed over from memory as the shako token is.
        let token = secret(Secret::EkibenToken);
        group(
            "JC_PORTAL_PIPELINE_RUNNER_URL",
            vec![
                (
                    "JC_PORTAL_PIPELINE_RUNNER_URL",
                    Some(streams.internal.clone()),
                ),
                ("JC_PORTAL_PIPELINE_RUNNER_TOKEN", token),
                (
                    "JC_PORTAL_PIPELINES_BATCH_CONCURRENCY",
                    Some(PIPELINE_BATCH_RUNS.to_owned()),
                ),
            ],
        );
    }
    if let Some(catalogue) = siblings.get(Module::Dataroom) {
        // The Portal holds both catalogue URLs to https (EP-67; plain http only to a cluster
        // Service), so an https installation calls the API at its public URL too. An http
        // installation fills the route only with the operator's explicit
        // `JC_PORTAL_CKAN_ALLOW_HTTP=true` (a local rehearsal), else leaves it unset rather than
        // failing the Portal's start.
        let rehearsal = env("JC_PORTAL_CKAN_ALLOW_HTTP").as_deref() == Some("true");
        let api = if catalogue.public.starts_with("https://") {
            Some(catalogue.public.clone())
        } else if rehearsal {
            Some(catalogue.internal.clone())
        } else {
            None
        };
        if let Some(api) = api {
            group(
                "JC_PORTAL_CKAN_URL",
                vec![
                    ("JC_PORTAL_CKAN_URL", Some(catalogue.public.clone())),
                    ("JC_PORTAL_CKAN_API_URL", Some(api)),
                ],
            );
        }
    }
    if let Some(dash) = siblings.get(Module::Daiya) {
        group(
            "JC_PORTAL_GRAFANA_URL",
            vec![
                ("JC_PORTAL_GRAFANA_URL", Some(dash.internal.clone())),
                ("JC_PORTAL_GRAFANA_PUBLIC_URL", Some(dash.public.clone())),
                (
                    "JC_PORTAL_GRAFANA_USER",
                    Some(setup::DASHBOARDS_USER.to_owned()),
                ),
                (
                    "JC_PORTAL_GRAFANA_PASSWORD",
                    secret(Secret::DashboardsPassword),
                ),
            ],
        );
    }
    if let Some(broker) = siblings.get(Module::Antares) {
        group(
            "JC_PORTAL_BROKER_URL",
            vec![
                ("JC_PORTAL_BROKER_URL", Some(broker.internal.clone())),
                (
                    "JC_PORTAL_BROKER_ADMIN_TOKEN_FILE",
                    setup
                        .path(Secret::AntaresAdminToken)
                        .map(|p| p.display().to_string()),
                ),
            ],
        );
    }
    if let Some(gateway) = siblings.get(Module::Gateway) {
        // The gateway reads the internal listener with its own client's token (PF-46).
        group(
            "JC_PORTAL_GATEWAY_URL",
            vec![
                ("JC_PORTAL_GATEWAY_URL", Some(gateway.internal.clone())),
                (
                    "JC_PORTAL_GATEWAY_CLIENT_ID",
                    Some(setup::GATEWAY_CLIENT.to_owned()),
                ),
            ],
        );
    }
    if let Some(tools) = siblings.get(Module::ModelTools) {
        group(
            "JC_PORTAL_MODEL_TOOLS_URL",
            vec![("JC_PORTAL_MODEL_TOOLS_URL", Some(tools.internal.clone()))],
        );
    }
    if let Some(assistant) = siblings.get(Module::Assistant) {
        group(
            "JC_PORTAL_KNOWLEDGE_URL",
            vec![("JC_PORTAL_KNOWLEDGE_URL", Some(assistant.internal.clone()))],
        );
    }
    // The Portal's own secret backend (SOPS + age) unless the environment names a backend: the
    // identity the operator gave (checked at start), else the one setup generated.
    if unset("JC_PORTAL_SOPS_AGE_KEY_FILE")
        && unset("JC_PORTAL_OPENBAO_ADDR")
        && let Some(path) = setup
            .age_key_file
            .as_deref()
            .or_else(|| setup.path(Secret::PortalAgeKey))
    {
        out.insert(
            "JC_PORTAL_SOPS_AGE_KEY_FILE".to_owned(),
            path.display().to_string(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Cli, Config, FileConfig, MountOverride};

    const LISTEN: &str = "0.0.0.0:8443";

    fn plans(mounts: BTreeMap<Module, Vec<MountOverride>>, scheme: &str) -> Vec<MountPlan> {
        let mut cfg = Config::merge(
            Cli {
                role: Some("all".into()),
                base_host: Some("city.example".into()),
                scheme: Some(scheme.into()),
                ..Cli::default()
            },
            FileConfig::default(),
        )
        .unwrap();
        cfg.mounts = mounts;
        crate::app::plan_mounts(&cfg).unwrap()
    }

    fn fixture(scheme: &str) -> (Siblings, Setup, tempfile::TempDir) {
        let siblings = Siblings::new(&plans(BTreeMap::new(), scheme), LISTEN.parse().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let setup = Setup::prepare(dir.path(), &Module::ALL, &BTreeMap::new()).unwrap();
        (siblings, setup, dir)
    }

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn slug_mode_calls_siblings_on_loopback_and_host_mounts_at_their_host() {
        let (siblings, _, _d) = fixture("https");
        let id = siblings.get(Module::Kaisatsu).unwrap();
        assert_eq!(id.public, "https://city.example/id");
        assert_eq!(id.internal, "http://127.0.0.1:8443/id");
        assert_eq!(siblings.listener, "127.0.0.1:8443".parse().unwrap());
        assert_eq!(
            siblings.portal_base().as_deref(),
            Some("https://city.example/portal/")
        );
        let mut mounts = BTreeMap::new();
        mounts.insert(
            Module::Shako,
            vec![MountOverride {
                host: Some("git.city.example".into()),
                path_prefix: None,
            }],
        );
        let siblings = Siblings::new(&plans(mounts, "https"), "[::]:1".parse().unwrap());
        let git = siblings.get(Module::Shako).unwrap();
        assert_eq!(git.internal, git.public);
        assert_eq!(git.public, "https://git.city.example");
        assert_eq!(siblings.listener, "[::1]:1".parse().unwrap());
    }

    /// daiya reaches the gateway's Endpoints on the own listener at the address the Portal names
    /// in the datasources it makes (`{JC_PORTAL_GATEWAY_URL}/api/endpoint/{slug}`), and nothing
    /// when the gateway answers on a host of its own.
    #[cfg(feature = "daiya")]
    #[test]
    fn daiya_reaches_the_gateway_endpoints_the_portal_names() {
        use crate::modules::daiya::gateway_reach;
        use daiya_server::OwnReach;
        let (siblings, _, _d) = fixture("https");
        let gateway = siblings.get(Module::Gateway).unwrap().internal.clone();
        assert_eq!(
            gateway_reach(&siblings),
            Some(OwnReach::endpoints(&format!("{gateway}/api/endpoint/")).unwrap())
        );
        assert_eq!(
            gateway_reach(&siblings).map(|r| r.addr()),
            Some(siblings.listener)
        );
        let mut mounts = BTreeMap::new();
        mounts.insert(
            Module::Gateway,
            vec![MountOverride {
                host: Some("api.city.example".into()),
                path_prefix: None,
            }],
        );
        let siblings = Siblings::new(&plans(mounts, "https"), LISTEN.parse().unwrap());
        assert_eq!(gateway_reach(&siblings), None);
    }

    #[test]
    fn live_addresses_come_from_the_mounts_unless_named_and_nobody_serves_tiles_or_assistant() {
        let (siblings, _, _d) = fixture("https");
        let live = live_addresses(&crate::config::DataroomAddresses::default(), &siblings);
        assert_eq!(live.viz, "https://city.example/viz");
        assert_eq!(
            live.api, "https://city.example",
            "the gateway answers at the host's root"
        );
        assert_eq!(live.portal, "https://city.example/portal");
        assert_eq!(live.catalogue, "https://city.example/opendata");
        assert_eq!((live.tiles.as_str(), live.assistant.as_str()), ("", ""));

        let named = crate::config::DataroomAddresses {
            api: Some("https://api.dev.example".into()),
            tiles: Some("https://tiles.dev.example".into()),
            ..Default::default()
        };
        let live = live_addresses(&named, &siblings);
        assert_eq!(live.api, "https://api.dev.example");
        assert_eq!(live.tiles, "https://tiles.dev.example");
        assert_eq!(live.assistant, "");
    }

    #[test]
    fn a_base_url_has_no_query_user_trailing_slash_or_odd_character() {
        for ok in [
            "http://localhost:42241/viz",
            "https://api.city.example",
            "http://dataroom:8080",
        ] {
            assert!(is_base_url(ok), "{ok}");
        }
        for bad in [
            "https://api.example/",
            "https://api.example/a?x",
            "https://u@api.example",
            "javascript:alert(1)",
            "https://API.example",
            "https://api.example/a/../b",
            "https://api.example'; script-src *",
            "https://api.example:123456",
            "https://",
        ] {
            assert!(!is_base_url(bad), "{bad}");
        }
    }

    #[test]
    fn the_portal_signs_in_at_the_configured_realm() {
        let (siblings, mut setup, _d) = fixture("https");
        let issuer = |setup: &Setup| portal_vars(&siblings, setup, none)["JC_OIDC_ISSUER"].clone();
        assert_eq!(issuer(&setup), "https://city.example/id/realms/jc");
        setup.realm = setup::Realm::parse("dev").unwrap();
        assert_eq!(issuer(&setup), "https://city.example/id/realms/dev");
    }

    /// Asserts the keys of one row come from the sibling and an explicit URL keeps the whole row
    /// (credentials included) out of the fill.
    fn row(url_key: &str, keys: &[&str], scheme: &str) -> BTreeMap<String, String> {
        let (siblings, setup, _d) = fixture(scheme);
        let filled = portal_vars(&siblings, &setup, none);
        for key in keys {
            assert!(filled.contains_key(*key), "{key} filled");
        }
        let explicit = portal_vars(&siblings, &setup, |k| {
            (k == url_key).then(|| "https://external.example/x".to_owned())
        });
        for key in keys {
            assert!(
                !explicit.contains_key(*key),
                "{key} left to the environment"
            );
        }
        // Without the sibling in the role set the row stays as today (unset).
        let others: Vec<Module> = Module::ALL
            .into_iter()
            .filter(|m| !keys.is_empty() && module_of(url_key) != *m)
            .collect();
        let alone = plans(BTreeMap::new(), scheme)
            .into_iter()
            .filter(|p| others.contains(&p.module))
            .collect::<Vec<_>>();
        let without = portal_vars(
            &Siblings::new(&alone, LISTEN.parse().unwrap()),
            &setup,
            none,
        );
        for key in keys {
            assert!(
                !without.contains_key(*key),
                "{key} unset without its sibling"
            );
        }
        filled
    }

    fn module_of(url_key: &str) -> Module {
        match url_key {
            "JC_OIDC_ISSUER" => Module::Kaisatsu,
            "JC_SHAKO_URL" => Module::Shako,
            "JC_PORTAL_PIPELINE_RUNNER_URL" => Module::Ekiben,
            "JC_PORTAL_CKAN_URL" => Module::Dataroom,
            "JC_PORTAL_GRAFANA_URL" => Module::Daiya,
            "JC_PORTAL_BROKER_URL" => Module::Antares,
            other => panic!("{other}"),
        }
    }

    #[test]
    fn kaisatsu_row() {
        let v = row(
            "JC_OIDC_ISSUER",
            &[
                "JC_OIDC_ISSUER",
                "JC_OIDC_CLIENT_ID",
                "JC_OIDC_CLIENT_SECRET",
                "JC_KEYCLOAK_ADMIN_URL",
                "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ID",
                "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SECRET",
                "JC_PORTAL_COOKIE_KEY",
            ],
            "https",
        );
        assert_eq!(v["JC_OIDC_ISSUER"], "https://city.example/id/realms/jc");
        assert_eq!(v["JC_KEYCLOAK_ADMIN_URL"], "http://127.0.0.1:8443");
        assert_eq!(v["JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ID"], "portal-reconciler");
        assert_eq!(v["JC_OIDC_CLIENT_ID"], "portal-api");
    }

    #[test]
    fn model_tools_row_comes_from_its_loopback_listener_and_an_explicit_url_wins() {
        let (mut siblings, setup, _d) = fixture("https");
        let key = "JC_PORTAL_MODEL_TOOLS_URL";
        assert!(!portal_vars(&siblings, &setup, none).contains_key(key));
        siblings.insert_loopback(Module::ModelTools, "0.0.0.0:40123".parse().unwrap());
        let filled = portal_vars(&siblings, &setup, none);
        assert_eq!(filled[key], "http://127.0.0.1:40123");
        let explicit = portal_vars(&siblings, &setup, |k| {
            (k == key).then(|| "http://model-tools.example:8080".to_owned())
        });
        assert!(!explicit.contains_key(key));
    }

    #[test]
    fn knowledge_row_follows_the_assistant_and_an_explicit_url_wins() {
        let (mut siblings, setup, _d) = fixture("https");
        let key = "JC_PORTAL_KNOWLEDGE_URL";
        siblings.set_internal(Module::Assistant, "0.0.0.0:40124".parse().unwrap());
        let filled = portal_vars(&siblings, &setup, none);
        assert_eq!(filled[key], "http://127.0.0.1:40124");
        assert_eq!(
            siblings.get(Module::Assistant).unwrap().public,
            "https://city.example",
            "the public mount stays"
        );
        let explicit = portal_vars(&siblings, &setup, |k| {
            (k == key).then(|| "http://jc-assistant.example:8080".to_owned())
        });
        assert!(!explicit.contains_key(key));
        // Off: no knowledge address at all.
        siblings.remove(Module::Assistant);
        assert!(!portal_vars(&siblings, &setup, none).contains_key(key));
    }

    #[test]
    fn an_explicit_key_inside_a_filled_row_wins() {
        let (siblings, setup, _d) = fixture("https");
        let v = portal_vars(&siblings, &setup, |k| {
            (k == "JC_OIDC_CLIENT_ID").then(|| "my-portal".to_owned())
        });
        assert!(!v.contains_key("JC_OIDC_CLIENT_ID"));
        assert!(v.contains_key("JC_OIDC_ISSUER"));
        // A blank value counts as unset.
        let v = portal_vars(&siblings, &setup, |k| {
            (k == "JC_OIDC_ISSUER").then(|| " ".to_owned())
        });
        assert!(v.contains_key("JC_OIDC_ISSUER"));
    }

    #[test]
    fn shako_row() {
        let v = row(
            "JC_SHAKO_URL",
            &[
                "JC_SHAKO_URL",
                "JC_SHAKO_PUBLIC_URL",
                "JC_SHAKO_OWNER",
                "JC_SHAKO_REPO",
                "JC_SHAKO_TOKEN",
                "JC_SHAKO_APPS_OWNER",
                "JC_SHAKO_APPS_TOKEN",
                "JC_SHAKO_WEBHOOK_SECRET",
            ],
            "https",
        );
        assert_eq!(v["JC_SHAKO_URL"], "http://127.0.0.1:8443/git");
        assert_eq!(v["JC_SHAKO_OWNER"], "jc");
    }

    #[test]
    fn ekiben_row() {
        let v = row(
            "JC_PORTAL_PIPELINE_RUNNER_URL",
            &[
                "JC_PORTAL_PIPELINE_RUNNER_URL",
                "JC_PORTAL_PIPELINE_RUNNER_TOKEN",
                "JC_PORTAL_PIPELINES_BATCH_CONCURRENCY",
            ],
            "https",
        );
        assert_eq!(
            v["JC_PORTAL_PIPELINE_RUNNER_URL"],
            "http://127.0.0.1:8443/streams"
        );
        // Three batch runs at once, as the deployment runs prod's Portal (T-5778).
        assert_eq!(v["JC_PORTAL_PIPELINES_BATCH_CONCURRENCY"], "3");
        assert!(!v["JC_PORTAL_PIPELINE_RUNNER_TOKEN"].trim().is_empty());
    }

    #[test]
    fn dataroom_row_and_no_http_public_catalogue() {
        let v = row(
            "JC_PORTAL_CKAN_URL",
            &["JC_PORTAL_CKAN_URL", "JC_PORTAL_CKAN_API_URL"],
            "https",
        );
        assert_eq!(v["JC_PORTAL_CKAN_URL"], "https://city.example/opendata");
        // The Portal refuses a plain http API outside a cluster Service (EP-67).
        assert_eq!(v["JC_PORTAL_CKAN_API_URL"], "https://city.example/opendata");
        let (siblings, setup, _d) = fixture("http");
        assert!(!portal_vars(&siblings, &setup, none).contains_key("JC_PORTAL_CKAN_URL"));
        for typo in ["TRUE", "1", "yes", ""] {
            let v = portal_vars(&siblings, &setup, |k: &str| {
                (k == "JC_PORTAL_CKAN_ALLOW_HTTP").then(|| typo.to_owned())
            });
            assert!(!v.contains_key("JC_PORTAL_CKAN_URL"), "{typo}");
        }
    }

    /// A one-machine rehearsal over http fills the catalogue only on the operator's explicit
    /// `JC_PORTAL_CKAN_ALLOW_HTTP=true`, which the Portal reads too.
    #[test]
    fn an_http_catalogue_needs_the_rehearsal_switch() {
        let (siblings, setup, _d) = fixture("http");
        let v = portal_vars(&siblings, &setup, |k: &str| {
            (k == "JC_PORTAL_CKAN_ALLOW_HTTP").then(|| "true".to_owned())
        });
        assert_eq!(v["JC_PORTAL_CKAN_URL"], "http://city.example/opendata");
        assert_eq!(
            v["JC_PORTAL_CKAN_API_URL"],
            "http://127.0.0.1:8443/opendata"
        );
    }

    #[test]
    fn daiya_row() {
        let v = row(
            "JC_PORTAL_GRAFANA_URL",
            &[
                "JC_PORTAL_GRAFANA_URL",
                "JC_PORTAL_GRAFANA_PUBLIC_URL",
                "JC_PORTAL_GRAFANA_USER",
                "JC_PORTAL_GRAFANA_PASSWORD",
            ],
            "https",
        );
        assert_eq!(
            v["JC_PORTAL_GRAFANA_PUBLIC_URL"],
            "https://city.example/dashboards"
        );
    }

    #[test]
    fn antares_row() {
        let v = row(
            "JC_PORTAL_BROKER_URL",
            &["JC_PORTAL_BROKER_URL", "JC_PORTAL_BROKER_ADMIN_TOKEN_FILE"],
            "https",
        );
        assert_eq!(v["JC_PORTAL_BROKER_URL"], "http://127.0.0.1:8443/broker");
        assert!(v["JC_PORTAL_BROKER_ADMIN_TOKEN_FILE"].ends_with("antares-admin-token"));
    }

    #[test]
    fn a_row_without_its_secret_is_not_filled() {
        let (siblings, _, _d) = fixture("https");
        let v = portal_vars(&siblings, &Setup::default(), none);
        assert!(!v.contains_key("JC_SHAKO_URL"));
        assert!(!v.contains_key("JC_OIDC_ISSUER"));
        // A runner URL without its bearer only meets 401s.
        assert!(!v.contains_key("JC_PORTAL_PIPELINE_RUNNER_URL"));
    }

    #[test]
    fn origin_drops_the_path() {
        assert_eq!(origin("http://127.0.0.1:1/id"), "http://127.0.0.1:1");
        assert_eq!(origin("https://a.example"), "https://a.example");
        assert_eq!(origin("nope"), "nope");
    }

    #[test]
    fn the_portal_resolves_secrets_with_its_setup_identity_unless_a_backend_is_named() {
        let (siblings, setup, _d) = fixture("https");
        let v = portal_vars(&siblings, &setup, none);
        let key = setup.path(Secret::PortalAgeKey).unwrap();
        assert_eq!(v["JC_PORTAL_SOPS_AGE_KEY_FILE"], key.display().to_string());
        let bao = portal_vars(&siblings, &setup, |k| {
            (k == "JC_PORTAL_OPENBAO_ADDR").then(|| "https://bao.example".to_owned())
        });
        assert!(!bao.contains_key("JC_PORTAL_SOPS_AGE_KEY_FILE"));
    }
}
