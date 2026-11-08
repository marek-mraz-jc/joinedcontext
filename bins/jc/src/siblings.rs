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
        }
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

/// The origin (`scheme://host:port`) of a URL string without a path.
fn origin(url: &str) -> String {
    match url.find("://") {
        Some(i) => {
            let rest = &url[i + 3..];
            let end = rest.find('/').unwrap_or(rest.len());
            format!("{}{}", &url[..i + 3], &rest[..end])
        }
        None => url.to_owned(),
    }
}

/// The Portal settings `jc` fills from its siblings, for the keys `env` leaves unset.
///
/// | Portal setting | sibling |
/// |---|---|
/// | `JC_OIDC_ISSUER`, `JC_OIDC_CLIENT_*`, `JC_KEYCLOAK_ADMIN_URL`, `JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_*`, `JC_PORTAL_COOKIE_KEY` | kaisatsu |
/// | `JC_GITEA_URL`, `_PUBLIC_URL`, `_OWNER`, `_REPO`, `_TOKEN`, `_APPS_OWNER`, `_APPS_TOKEN`, `_WEBHOOK_SECRET` | shako |
/// | `JC_PORTAL_PIPELINE_RUNNER_URL` | ekiben |
/// | `JC_PORTAL_CKAN_URL`, `JC_PORTAL_CKAN_API_URL` (only when the public scheme is https, EP-67) | dataroom |
/// | `JC_PORTAL_GRAFANA_URL`, `_PUBLIC_URL`, `_USER`, `_PASSWORD` | daiya |
/// | `JC_PORTAL_BROKER_URL`, `JC_PORTAL_BROKER_ADMIN_TOKEN_FILE` | antares |
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
                Some(format!("{}/realms/{}", id.public, setup::PORTAL_REALM)),
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
            "JC_GITEA_URL",
            vec![
                ("JC_GITEA_URL", Some(git.internal.clone())),
                ("JC_GITEA_PUBLIC_URL", Some(git.public.clone())),
                ("JC_GITEA_OWNER", Some(setup::FORGE_OWNER.to_owned())),
                ("JC_GITEA_REPO", Some(setup::FORGE_REPO.to_owned())),
                ("JC_GITEA_TOKEN", secret(Secret::ShakoToken)),
                (
                    "JC_GITEA_APPS_OWNER",
                    Some(setup::FORGE_APPS_OWNER.to_owned()),
                ),
                ("JC_GITEA_APPS_TOKEN", secret(Secret::ShakoToken)),
                (
                    "JC_GITEA_WEBHOOK_SECRET",
                    secret(Secret::ForgeWebhookSecret),
                ),
            ],
        );
    }
    if let Some(streams) = siblings.get(Module::Ekiben) {
        // The runner bearer has no Portal setting yet (compat gap in STATUS.md).
        group(
            "JC_PORTAL_PIPELINE_RUNNER_URL",
            vec![(
                "JC_PORTAL_PIPELINE_RUNNER_URL",
                Some(streams.internal.clone()),
            )],
        );
    }
    if let Some(catalogue) = siblings.get(Module::Dataroom) {
        // The Portal holds the public catalogue URL to https (EP-67); an http installation
        // leaves the route unset rather than failing the Portal's start.
        if catalogue.public.starts_with("https://") {
            group(
                "JC_PORTAL_CKAN_URL",
                vec![
                    ("JC_PORTAL_CKAN_URL", Some(catalogue.public.clone())),
                    ("JC_PORTAL_CKAN_API_URL", Some(catalogue.internal.clone())),
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Cli, Config, FileConfig, MountOverride};

    const LISTEN: &str = "0.0.0.0:8443";

    fn plans(mounts: BTreeMap<Module, MountOverride>, scheme: &str) -> Vec<MountPlan> {
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
        let setup = Setup::prepare(dir.path(), &Module::ALL).unwrap();
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
            MountOverride {
                host: Some("git.city.example".into()),
                path_prefix: None,
            },
        );
        let siblings = Siblings::new(&plans(mounts, "https"), "[::]:1".parse().unwrap());
        let git = siblings.get(Module::Shako).unwrap();
        assert_eq!(git.internal, git.public);
        assert_eq!(git.public, "https://git.city.example");
        assert_eq!(siblings.listener, "[::1]:1".parse().unwrap());
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
            "JC_GITEA_URL" => Module::Shako,
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
        assert_eq!(v["JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ID"], "portal-admin");
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
            "JC_GITEA_URL",
            &[
                "JC_GITEA_URL",
                "JC_GITEA_PUBLIC_URL",
                "JC_GITEA_OWNER",
                "JC_GITEA_REPO",
                "JC_GITEA_TOKEN",
                "JC_GITEA_APPS_OWNER",
                "JC_GITEA_APPS_TOKEN",
                "JC_GITEA_WEBHOOK_SECRET",
            ],
            "https",
        );
        assert_eq!(v["JC_GITEA_URL"], "http://127.0.0.1:8443/git");
        assert_eq!(v["JC_GITEA_OWNER"], "jc");
    }

    #[test]
    fn ekiben_row() {
        let v = row(
            "JC_PORTAL_PIPELINE_RUNNER_URL",
            &["JC_PORTAL_PIPELINE_RUNNER_URL"],
            "https",
        );
        assert_eq!(
            v["JC_PORTAL_PIPELINE_RUNNER_URL"],
            "http://127.0.0.1:8443/streams"
        );
    }

    #[test]
    fn dataroom_row_and_no_http_public_catalogue() {
        let v = row(
            "JC_PORTAL_CKAN_URL",
            &["JC_PORTAL_CKAN_URL", "JC_PORTAL_CKAN_API_URL"],
            "https",
        );
        assert_eq!(v["JC_PORTAL_CKAN_URL"], "https://city.example/opendata");
        assert_eq!(
            v["JC_PORTAL_CKAN_API_URL"],
            "http://127.0.0.1:8443/opendata"
        );
        let (siblings, setup, _d) = fixture("http");
        assert!(!portal_vars(&siblings, &setup, none).contains_key("JC_PORTAL_CKAN_URL"));
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
        assert!(!v.contains_key("JC_GITEA_URL"));
        assert!(!v.contains_key("JC_OIDC_ISSUER"));
        assert!(v.contains_key("JC_PORTAL_PIPELINE_RUNNER_URL"));
    }

    #[test]
    fn origin_drops_the_path() {
        assert_eq!(origin("http://127.0.0.1:1/id"), "http://127.0.0.1:1");
        assert_eq!(origin("https://a.example"), "https://a.example");
        assert_eq!(origin("nope"), "nope");
    }
}
