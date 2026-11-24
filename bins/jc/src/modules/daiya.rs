//! daiya (dashboards) inside `jc`: the state of the standalone `daiya serve`, on Postgres when
//! `[daiya.database]` is set (daiya's own `open_store`: migration as the owning role when one is
//! configured, then the request pool and the boot check that refuses a role or schema without
//! forced RLS), else on its in-memory store. When the Portal runs beside it, first-start setup
//! holds a password for the Portal's Grafana client, which daiya accepts as its server
//! administrator (HTTP Basic, `DaiyaState::with_server_admin`), the role Grafana's admin user
//! holds for the Portal. A browser signed in to the Portal beside it reads the dashboard page of
//! the projects it may read (`page_reader`).

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
use daiya_server::{DaiyaState, EndpointBase, OperatorReach, OwnReach, router};

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

/// The person a browser's Portal cookie names, when the Portal in this process lets them read
/// the project whose organization daiya numbers `num` and names `name` (the Portal names each
/// project's organization after the project; number 1 is the installation's, the `org`
/// namespace). `None` while no Portal runs here: daiya keeps no sessions (PORTAL-CALLS.md G-5).
#[cfg(feature = "portal")]
fn page_reader(headers: &axum::http::HeaderMap, num: i64, name: &str) -> Option<String> {
    let portal = super::PORTAL_STATE.get()?;
    let project = if num == 1 {
        joinedcontext_portal::permissions::ORG_NAMESPACE
    } else {
        name
    };
    joinedcontext_portal::api::grafana_dashboards::page_reader(portal, headers, project)
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

/// The gateway's Endpoint bases at each address the Portal may name in a datasource
/// (`{public}/api/endpoint/` and `{internal}/api/endpoint/`): the only URLs, beside
/// [`gateway_reach`], a person's token is sent to. Empty when no gateway is known.
#[must_use]
pub fn gateway_endpoint_bases(siblings: &Siblings) -> Vec<EndpointBase> {
    let Some(gateway) = siblings.get(Module::Gateway) else {
        return Vec::new();
    };
    let mut bases: Vec<EndpointBase> = [&gateway.public, &gateway.internal]
        .into_iter()
        .filter_map(|at| EndpointBase::parse(&format!("{at}/api/endpoint/")).ok())
        .collect();
    bases.dedup();
    bases
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
    #[cfg(feature = "portal")]
    {
        state = state.with_page_reader(page_reader);
    }
    declare_organizations(&state, &cfg.organizations)
        .await
        .map_err(|e| e.to_string())?;
    state.own_reach = gateway_reach(siblings).into_iter().collect();
    state.endpoint_bases = gateway_endpoint_bases(siblings);
    if let Some(raw) = &cfg.prometheus_reach {
        let reach = OperatorReach::parse(raw).map_err(|e| e.to_string())?;
        state.outbound_policy.prometheus_reach = Some((state.default_org_id, reach));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pg::DatabaseSection;
    use crate::siblings::Sibling;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn database(migrate: bool) -> DatabaseSection {
        DatabaseSection {
            url: "postgres://daiya_app@db:5432/daiya".into(),
            password_file: PathBuf::from("/run/secrets/app"),
            max_connections: Some(7),
            migrate_url: migrate.then(|| "postgres://daiya_owner@db:5432/daiya".into()),
            migrate_password_file: migrate.then(|| PathBuf::from("/run/secrets/owner")),
            maintenance_url: None,
            maintenance_password_file: None,
        }
    }

    #[test]
    fn the_store_is_memory_without_a_database_and_postgres_with_one() {
        assert!(matches!(
            store_config(&DaiyaSection::default()),
            StoreConfig::Memory
        ));
        let cfg = DaiyaSection {
            database: Some(database(true)),
            ..DaiyaSection::default()
        };
        assert!(
            matches!(
                store_config(&cfg),
                StoreConfig::Postgres { url, password_file, max_connections: 7, migrate: Some(m) }
                    if url == "postgres://daiya_app@db:5432/daiya"
                        && password_file == std::path::Path::new("/run/secrets/app")
                        && m.url == "postgres://daiya_owner@db:5432/daiya"
                        && m.password_file == std::path::Path::new("/run/secrets/owner")
            ),
            "a database section makes a Postgres store with its migration role"
        );
        // A migration URL without its password file migrates nothing.
        let mut half = database(false);
        half.migrate_url = Some("postgres://daiya_owner@db/daiya".into());
        let cfg = DaiyaSection {
            database: Some(half),
            ..DaiyaSection::default()
        };
        assert!(matches!(
            store_config(&cfg),
            StoreConfig::Postgres { migrate: None, .. }
        ));
    }

    fn mount() -> Mount {
        Mount::new("daiya", "http", "localhost", "").unwrap()
    }

    fn setup(dir: &std::path::Path, roles: &[Module]) -> Setup {
        Setup::prepare(dir, roles, &BTreeMap::new()).unwrap()
    }

    fn siblings(gateway: Option<(&str, &str)>) -> Siblings {
        let mut siblings = Siblings::new(&[], "127.0.0.1:8080".parse().unwrap());
        if let Some((public, internal)) = gateway {
            siblings.insert_remote(
                Module::Gateway,
                Sibling::direct(public.into(), internal.into()),
            );
        }
        siblings
    }

    #[tokio::test]
    async fn alone_daiya_keeps_no_server_admin_and_beside_the_portal_holds_one() {
        let tmp = tempfile::tempdir().unwrap();
        let alone = state(
            &DaiyaSection::default(),
            &setup(tmp.path(), &[Module::Daiya]),
        )
        .await
        .unwrap();
        let beside = setup(tmp.path(), &[Module::Daiya, Module::Portal]);
        let password = beside.value(Secret::DashboardsPassword).unwrap().to_owned();
        let held = state(&DaiyaSection::default(), &beside).await.unwrap();
        let basic = |user: &str, password: &str| {
            use base64::Engine as _;
            let mut headers = axum::http::HeaderMap::new();
            let token =
                base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
            headers.insert(
                axum::http::header::AUTHORIZATION,
                format!("Basic {token}").parse().unwrap(),
            );
            headers.insert("x-grafana-org-id", "1".parse().unwrap());
            headers
        };
        let ask = |st: DaiyaState, headers: axum::http::HeaderMap| async move {
            use tower::ServiceExt as _;
            let mut request = axum::http::Request::get("/api/org")
                .body(axum::body::Body::empty())
                .unwrap();
            *request.headers_mut() = headers;
            router(st, mount()).oneshot(request).await.unwrap().status()
        };
        assert!(
            ask(held.clone(), basic(DASHBOARDS_USER, &password))
                .await
                .is_success(),
            "the Portal's client is daiya's server administrator"
        );
        assert_eq!(
            ask(held, basic(DASHBOARDS_USER, "wrong")).await,
            axum::http::StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            ask(alone, basic(DASHBOARDS_USER, &password)).await,
            axum::http::StatusCode::UNAUTHORIZED,
            "standalone daiya holds no Portal password"
        );
    }

    #[tokio::test]
    async fn a_database_that_cannot_open_refuses_the_start() {
        let tmp = tempfile::tempdir().unwrap();
        let mut db = database(false);
        db.password_file = tmp.path().join("missing");
        let cfg = DaiyaSection {
            database: Some(db),
            ..DaiyaSection::default()
        };
        let e = state(&cfg, &setup(tmp.path(), &[Module::Daiya]))
            .await
            .err()
            .expect("an unreadable password file refuses the start");
        assert!(!e.is_empty());
    }

    #[test]
    fn only_a_gateway_on_jcs_own_listener_is_reached_there() {
        assert_eq!(gateway_reach(&siblings(None)), None, "no gateway known");
        let own = siblings(Some((
            "https://city.test/gateway",
            "http://127.0.0.1:8080/gateway",
        )));
        assert!(gateway_reach(&own).is_some());
        let bare = siblings(Some(("https://gw.city.test", "http://127.0.0.1:8080")));
        assert!(gateway_reach(&bare).is_some(), "the listener's root counts");
        // A longer port that starts like the listener's is another listener.
        let other_port = siblings(Some(("https://gw.city.test", "http://127.0.0.1:80801/gw")));
        assert_eq!(gateway_reach(&other_port), None);
        let elsewhere = siblings(Some(("https://gw.city.test", "http://gateway:8080/gw")));
        assert_eq!(gateway_reach(&elsewhere), None, "a host of its own");
    }

    #[test]
    fn the_endpoint_bases_are_the_gateways_two_addresses_once_each() {
        assert!(gateway_endpoint_bases(&siblings(None)).is_empty());
        let two = siblings(Some((
            "https://city.test/gateway",
            "http://127.0.0.1:8080/gateway",
        )));
        assert_eq!(gateway_endpoint_bases(&two).len(), 2);
        let same = siblings(Some((
            "https://city.test/gateway",
            "https://city.test/gateway",
        )));
        assert_eq!(
            gateway_endpoint_bases(&same).len(),
            1,
            "an equal pair counts once"
        );
    }

    #[tokio::test]
    async fn a_bad_prometheus_reach_or_provisioning_root_refuses_the_start() {
        let _log =
            tracing::subscriber::set_default(tracing_subscriber::fmt().with_test_writer().finish());
        let tmp = tempfile::tempdir().unwrap();
        let roles = [Module::Daiya];
        let bad_reach = DaiyaSection {
            prometheus_reach: Some("not a url".into()),
            ..DaiyaSection::default()
        };
        let e = build(
            &bad_reach,
            &setup(tmp.path(), &roles),
            &siblings(None),
            mount(),
        )
        .await
        .expect_err("an unparsable reach refuses the start");
        assert!(!e.is_empty());
        let bad_dir = DaiyaSection {
            provisioning_dir: Some(tmp.path().join("missing")),
            ..DaiyaSection::default()
        };
        assert!(
            build(
                &bad_dir,
                &setup(tmp.path(), &roles),
                &siblings(None),
                mount()
            )
            .await
            .is_err(),
            "a provisioning root that is not there refuses the start"
        );
        let good = DaiyaSection {
            prometheus_reach: Some("http://prometheus-proxy:9090".into()),
            provisioning_dir: Some(tmp.path().join("empty")),
            ..DaiyaSection::default()
        };
        std::fs::create_dir_all(tmp.path().join("empty")).unwrap();
        let router = build(&good, &setup(tmp.path(), &roles), &siblings(None), mount())
            .await
            .expect("a valid reach and an empty provisioning root start");
        use tower::ServiceExt as _;
        let health = axum::http::Request::get("/api/health")
            .body(axum::body::Body::empty())
            .unwrap();
        assert!(router.oneshot(health).await.unwrap().status().is_success());
    }
}
