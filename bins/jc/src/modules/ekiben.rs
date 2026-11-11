//! ekiben (stream processor) inside `jc`, hardened for a shared process:
//! - the API refuses every request without the configured bearer token (`require_auth`), even
//!   the read-only endpoints; only `/ping` is open;
//! - a posted config and Bloblang `env()` read no process environment (`EnvAccess::None`): `jc`'s
//!   config loader reads the `EKIBEN_VAR_*` values and hands them over as the default
//!   organization's variable map, so a stream never reads another module's secret;
//! - `file` inputs and outputs are confined to one directory (`file_root`), so a stream cannot
//!   read another module's key file or `/proc/self/environ`;
//! - with the Portal in the same process, the pipeline credentials its reconciler resolves from
//!   each project's own SOPS files reach that same map ([`HostRunner`]), in place of the
//!   Kubernetes Secret the Portal writes for a runner in a cluster;
//! - the runner variables dev's Deployment sets come from `jc`'s own mounts and files
//!   ([`runner_defaults`]): `JC_GATEWAY_URL`, `JC_GATEWAY_HOST`, `JC_TOKEN_URL`, `JC_ORG_DOMAIN`,
//!   `JC_ORG_ID` (the owner segment of a minted id, ADR-N-068), `JC_CLIENT_ID`, and each `JC_CLIENT_SECRET_<PROJECT>` of `[ekiben] client_secrets_dir`
//!   (project mode: the Portal's default, as on the local and production profiles);
//! - private and loopback HTTP targets stay refused (ekiben's default quotas), so a stream cannot
//!   call the other modules' listeners from inside the process; the one exception is `jc`'s own
//!   listener under the gateway's `/api/endpoint` and kaisatsu's token endpoint
//!   ([`HostTarget`]), the two doors a stream reaches on dev;
//! - the platform's MQTT brokers are private hosts each of one project (`JC_EKIBEN_PRIVATE_HOSTS`,
//!   [`admit`]): every stream, posted, put or restored, and every resource posted through the API
//!   is refused when it names another project's broker, a runner username other than its own
//!   project's or the Portal's capture username. The gateway admits a tokenless CONNECT by that
//!   username from the jc pod, where every project's stream runs, so the check binds it here.
//!
//! With `[ekiben.database]` every continuous stream posted through the API is kept in Postgres
//! (`ekiben_pg`) and started again at the next start; without it streams live in memory.

use crate::app::StartError;
use crate::config::EkibenSection;
use crate::module::Module;
use crate::siblings::Siblings;
use axum::Router;
use ekiben_core::{EnvAccess, HostTarget, OrgContext, OrgId, ResourceManager};
use ekiben_server::{
    Admission, ServerAuthConfig, ServerState, StreamManager, build_full_registry, build_router,
};
use jc_core::kinds::mqtt_broker::{RUNNER_USERNAME, runner_username};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::module(Module::Ekiben, e)
}

/// The sandbox directory for `file` components: the configured one, or a private (0700)
/// directory of this process under the system temp dir.
fn file_root(configured: Option<&Path>) -> Result<PathBuf, StartError> {
    let dir = match configured {
        Some(dir) => dir.to_path_buf(),
        None => std::env::temp_dir().join(format!("jc-ekiben-{}", std::process::id())),
    };
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&dir)
        .map_err(|e| err(format!("cannot create file root {}: {e}", dir.display())))?;
    Ok(dir)
}

pub async fn build(
    cfg: &EkibenSection,
    setup: &crate::setup::Setup,
    secrets: &crate::config::Secrets,
    siblings: &Siblings,
    portal_beside: bool,
) -> Result<Router, StartError> {
    let hosts = private_hosts(&crate::config::ekiben_private_hosts());
    build_with(cfg, setup, secrets, siblings, portal_beside, hosts).await
}

/// [`build`] with the private hosts given rather than read from `JC_EKIBEN_PRIVATE_HOSTS`.
async fn build_with(
    cfg: &EkibenSection,
    setup: &crate::setup::Setup,
    secrets: &crate::config::Secrets,
    siblings: &Siblings,
    portal_beside: bool,
    hosts: Vec<PrivateHost>,
) -> Result<Router, StartError> {
    let token = setup
        .value(crate::setup::Secret::EkibenToken)
        .ok_or_else(|| err("no API token"))?
        .to_owned();
    let root = file_root(cfg.file_root.as_deref())?;
    tracing::info!(file_root = %root.display(), "ekiben file components confined");
    let defaults = runner_defaults(
        siblings,
        &setup.realm,
        cfg.harness_client.as_deref(),
        crate::config::portal_env("JC_PORTAL_ORG_DOMAIN"),
    );
    let mut resources = ResourceManager::new();
    let late = portal_beside.then(|| super::PORTAL_INTERNAL.clone());
    if let Some(target) = host_target(siblings, &defaults, late) {
        tracing::info!(addr = %target.addr, prefixes = ?target.path_prefixes, portal_internal = target.late.is_some(), "ekiben streams reach jc's gateway and token endpoint");
        resources = resources.with_host_target(target);
    }
    open_private_hosts(&resources, &hosts);
    let names: Vec<&String> = defaults
        .keys()
        .chain(secrets.pipeline_client_secrets.keys())
        .collect();
    tracing::info!(variables = ?names, "ekiben runner variables from jc");
    let runner = HostRunner {
        org: resources.get_or_create_org(&OrgId::default()),
        given: variables(
            &defaults,
            &secrets.pipeline_client_secrets,
            &secrets.ekiben_vars,
        ),
    };
    runner.org.replace_secrets(runner.given.clone());
    if portal_beside {
        hand_to_portal(runner);
    }
    let mut manager = StreamManager::new(
        build_full_registry(),
        resources,
        Some(root),
        EnvAccess::None,
    )
    .with_admission(admission(hosts));
    // The deployment's shared resources first (Bento's `streams -r`): a restored or posted
    // stream may name any of them (`helsinki__gtfs`, `events_tail`).
    if let Some(path) = &cfg.resources_file {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| err(format!("resources file {}: {e}", path.display())))?;
        let count = manager
            .register_resources(&OrgId::default(), &raw)
            .map_err(|e| err(format!("resources file {}: {e}", path.display())))?;
        tracing::info!(file = %path.display(), count, "ekiben shared resources loaded");
    }
    if let Some(db) = &cfg.database {
        if let Some(opts) = db.migrate_options().map_err(err)? {
            let pool = crate::pg::connect(opts, 1).await.map_err(err)?;
            super::ekiben_pg::migrate(&pool, &db.app_role().map_err(err)?)
                .await
                .map_err(err)?;
            pool.close().await;
        }
        let pool = crate::pg::connect(db.app_options().map_err(err)?, db.max_connections())
            .await
            .map_err(err)?;
        let streams =
            std::sync::Arc::new(super::ekiben_pg::PgStreams::open(pool).await.map_err(err)?);
        manager = manager.with_persistence(streams.clone());
        let started = streams.restore(&manager).await.map_err(err)?;
        tracing::info!(started, "ekiben: saved streams started");
    }
    let state = ServerState::new(
        manager,
        ServerAuthConfig {
            admin_bearer_token: Some(token),
            org_tokens: Vec::new(),
            require_auth: true,
            guard_probes: true,
            ..Default::default()
        },
    );
    Ok(build_router(state))
}

/// One entry of `JC_EKIBEN_PRIVATE_HOSTS`: a broker's name (and port) and the one project
/// whose streams may name it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PrivateHost {
    project: String,
    /// `name` or `name:port`, as ekiben's `private_hosts` reads it.
    entry: String,
    /// The name alone, lowercase.
    name: String,
}

/// Reads `JC_EKIBEN_PRIVATE_HOSTS`: comma separated `project=name` entries, `name:port` for one
/// port. An entry naming no project opens nothing and is logged.
fn private_hosts(list: &str) -> Vec<PrivateHost> {
    list.split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .filter_map(|item| {
            let owned = item
                .split_once('=')
                .map(|(p, e)| (p.trim(), e.trim()))
                .filter(|(p, e)| !p.is_empty() && !e.is_empty());
            let Some((project, entry)) = owned else {
                tracing::warn!(entry = item, "JC_EKIBEN_PRIVATE_HOSTS: an entry names no project (`project=name:port`); it is not opened");
                return None;
            };
            let name = entry.rsplit_once(':').map_or(entry, |(n, _)| n);
            Some(PrivateHost {
                project: project.to_owned(),
                entry: entry.to_owned(),
                name: name.trim_end_matches('.').to_ascii_lowercase(),
            })
        })
        .collect()
}

/// Opens `hosts` to the default organization's streams although they resolve to private
/// addresses: the platform's MQTT brokers, which a DataSource reads at
/// `tcp://<release>.<namespace>.svc.cluster.local:1883`. Which project's stream may name each
/// is [`admit`]'s. ekiben still refuses loopback, link-local and metadata addresses behind any
/// listed name.
fn open_private_hosts(resources: &ResourceManager, hosts: &[PrivateHost]) {
    if hosts.is_empty() {
        return;
    }
    let entries: Vec<String> = hosts.iter().map(|h| h.entry.clone()).collect();
    tracing::info!(hosts = ?entries, "ekiben streams may reach these private hosts, each from its project's streams");
    let org = resources.get_or_create_org(&OrgId::default());
    org.set_quotas(ekiben_core::OrgQuotas {
        private_hosts: entries,
        ..org.get_quotas()
    });
}

/// The username the Portal's capture subscriber CONNECTs with (context-gateway
/// `mqtt::CAPTURE_USERNAME`): the gateway admits it from the jc pod, so no stream may name it.
const CAPTURE_USERNAME: &str = "jc-portal-capture";

/// Whether a stream (`stream` its id, `{project}.{pipeline}`) or a resource posted through the
/// API (`None`; every stream of the organization may name it, so it belongs to no project) may
/// run `filled`, its config after `${VAR}` interpolation. Every text of the config counts, so a
/// URL list, a Bloblang literal and a socket address are read alike: it may name no private host
/// of another project, no runner username but its own project's and not the capture username.
/// The reason names only what the config itself names.
fn admit(hosts: &[PrivateHost], stream: Option<&str>, filled: &Value) -> Result<(), String> {
    let project = stream.and_then(|id| id.split_once('.')).map(|(p, _)| p);
    let own_runner = project.map(runner_username);
    let mut texts = Vec::new();
    strings(filled, &mut texts);
    for text in texts {
        let lower = text.to_ascii_lowercase();
        if let Some(host) = hosts
            .iter()
            .find(|h| Some(h.project.as_str()) != project && names_host(&lower, &h.name))
        {
            return Err(format!(
                "names the private host {}, which belongs to another project",
                host.name
            ));
        }
        if lower.contains(CAPTURE_USERNAME) {
            return Err(format!(
                "names the Portal's capture username {CAPTURE_USERNAME}, which no stream may use"
            ));
        }
        if lower.contains(RUNNER_USERNAME) && own_runner.as_deref() != Some(text) {
            return Err(match &own_runner {
                Some(own) => {
                    format!("names a runner username other than its own project's ({own})")
                }
                None => "names a runner username, which only a project's stream may use".to_owned(),
            });
        }
    }
    Ok(())
}

/// Whether `text` names the host `name`: it occurs where no other DNS label character precedes
/// it, so `mqtt-a-b.ns` does not count inside `mqtt-x-mqtt-a-b.ns`.
fn names_host(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(at, _)| {
        !text[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// Every string of `value`, keys included.
fn strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(s) => out.push(s),
        Value::Array(items) => items.iter().for_each(|v| strings(v, out)),
        Value::Object(map) => map.iter().for_each(|(k, v)| {
            out.push(k);
            strings(v, out);
        }),
        _ => {}
    }
}

/// [`admit`] as ekiben asks it, logging each refusal with its reason.
fn admission(hosts: Vec<PrivateHost>) -> Admission {
    Arc::new(move |stream, filled| {
        admit(&hosts, stream, filled).inspect_err(|reason| {
            tracing::warn!(
                stream = stream.unwrap_or("(an API resource)"),
                %reason,
                "ekiben refused a stream"
            );
        })
    })
}

/// The default organization's variable map, as the Portal's reconciler sees the runner: what
/// `jc` was given (`EKIBEN_VAR_*`) plus the pipeline credentials of the last sync. A given name
/// wins over a pipeline's, so a project cannot replace a variable the operator set.
pub struct HostRunner {
    org: Arc<OrgContext>,
    given: HashMap<String, String>,
}

impl HostRunner {
    /// The variables after a sync handed `resolved`.
    #[cfg_attr(not(feature = "portal"), allow(dead_code))]
    fn merged(&self, resolved: BTreeMap<String, String>) -> HashMap<String, String> {
        let mut out: HashMap<String, String> = resolved.into_iter().collect();
        for (name, value) in &self.given {
            if out.insert(name.clone(), value.clone()).is_some() {
                tracing::warn!(variable = %name, "a pipeline credential names a variable jc was given; jc's value is kept");
            }
        }
        out
    }
}

#[cfg(feature = "portal")]
impl joinedcontext_portal::pipeline_secrets::RunnerSecrets for HostRunner {
    fn replace(&self, values: BTreeMap<String, String>) {
        self.org.replace_secrets(self.merged(values));
    }
}

#[cfg(feature = "portal")]
fn hand_to_portal(runner: HostRunner) {
    joinedcontext_portal::pipeline_secrets::hand_runner_secrets_to(Arc::new(runner));
}

#[cfg(not(feature = "portal"))]
fn hand_to_portal(_: HostRunner) {}

/// The default organization's variable map: `jc`'s defaults, then the pipeline client secrets,
/// then what `jc` read from `EKIBEN_VAR_*`, each later one winning.
fn variables(
    defaults: &BTreeMap<String, String>,
    client_secrets: &BTreeMap<String, jc_secret::Secret>,
    given: &BTreeMap<String, jc_secret::Secret>,
) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = defaults.clone().into_iter().collect();
    for (k, v) in client_secrets.iter().chain(given) {
        out.insert(k.clone(), v.expose().to_owned());
    }
    out
}

/// What dev's runner Deployment sets (joinedcontext-deployment
/// `components/pipeline-runner/values/runner/development-values.yaml.gotmpl`), from `jc`'s mounts:
/// the gateway as the Portal reaches it, kaisatsu's public token endpoint of the Portal's realm
/// (kaisatsu names the issuer by the host a request reached it on, and the gateway verifies the
/// public one, as dev's runner asks the public `idm` host), the Organization's domain and the
/// harness client. None is a secret. `JC_MQTT_NAMESPACE` is the Organization's domain as a DNS
/// label: a DataSource on a broker outside the platform (HSL's `mqtt.hsl.fi`) renders its client
/// id `jc-<project>-<pipeline>-${JC_MQTT_NAMESPACE}`, which the runner refused unset (400 on every
/// pass), and the id must differ per installation or the broker drops one session for the other's
/// (T-4678). `EKIBEN_VAR_JC_MQTT_NAMESPACE` replaces it (a rehearsal of prod's domain must).
fn runner_defaults(
    siblings: &Siblings,
    realm: &crate::setup::Realm,
    harness_client: Option<&str>,
    org_domain: Option<String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(gateway) = siblings.get(Module::Gateway) {
        // ponytail: a stream has no reach (ekiben pins its own DNS), so in host mode it reads the
        // gateway at its public name; a host-mode HostTarget naming the host is the upgrade.
        let url = gateway.callable();
        out.insert("JC_GATEWAY_URL".to_owned(), url.to_owned());
        let host = url.split_once("://").map_or(url, |(_, rest)| rest);
        out.insert("JC_GATEWAY_HOST".to_owned(), host.to_owned());
    }
    if let Some(id) = siblings.get(Module::Kaisatsu) {
        out.insert(
            "JC_TOKEN_URL".to_owned(),
            format!("{}/realms/{realm}/protocol/openid-connect/token", id.public),
        );
    }
    let org_domain = org_domain
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| crate::provision::domain(siblings.portal_base().as_deref()));
    out.insert("JC_MQTT_NAMESPACE".to_owned(), dns_label(&org_domain));
    out.insert("JC_ORG_ID".to_owned(), jc_core::names::org_id(&org_domain));
    out.insert("JC_ORG_DOMAIN".to_owned(), org_domain);
    if let Some(client) = harness_client {
        out.insert("JC_CLIENT_ID".to_owned(), client.to_owned());
    }
    out
}

/// `text` as one DNS label: lowercase, every other character than `[a-z0-9]` a `-`, no edge `-`,
/// at most 63 characters.
fn dns_label(text: &str) -> String {
    let label: String = text
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(63)
        .collect();
    label.trim_matches('-').to_owned()
}

/// The part of `jc`'s listener a stream may call: the gateway's Endpoint door and kaisatsu's
/// token endpoint, where their URLs name that listener's own address (the gateway's internal URL
/// in slug mode; the token endpoint only when the public base is that address, as in a loopback
/// rehearsal with `--base-host 127.0.0.1:<port>`). Any other URL is a public name and passes
/// ekiben's ordinary checks; a loopback name (`localhost`) stays refused. With the Portal beside
/// it, `portal` is the Portal's internal listener once bound, under the routes a stream's
/// validation stage, run log and test capture post to (prod's runner reaches that port; each
/// route admits only the pipeline's own client).
fn host_target(
    siblings: &Siblings,
    defaults: &BTreeMap<String, String>,
    portal: Option<std::sync::Arc<std::sync::OnceLock<std::net::SocketAddr>>>,
) -> Option<HostTarget> {
    let origin = format!("http://{}", siblings.listener);
    let prefixes: Vec<String> = [
        defaults
            .get("JC_GATEWAY_URL")
            .map(|u| format!("{u}/api/endpoint")),
        defaults.get("JC_TOKEN_URL").cloned(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|u| u.strip_prefix(&origin).map(str::to_owned))
    .filter(|path| path.starts_with('/'))
    .collect();
    let late = portal.map(|addr| {
        (
            addr,
            vec![
                "/internal/pipelines".to_owned(),
                "/internal/pipeline-tests".to_owned(),
            ],
        )
    });
    (!prefixes.is_empty() || late.is_some()).then_some(HostTarget {
        addr: siblings.listener,
        path_prefixes: prefixes,
        late,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listed_private_hosts_open_to_the_default_organization_on_their_port() {
        let resources = ResourceManager::new();
        let broker = "mqtt-helsinki-sensors.jc-prod-mqtt.svc.cluster.local";
        let org = resources.get_or_create_org(&OrgId::default());
        assert!(!org.is_private_host_port_allowed(broker, 1883));
        open_private_hosts(&resources, &private_hosts(""));
        assert!(org.get_quotas().private_hosts.is_empty());
        open_private_hosts(
            &resources,
            &private_hosts(&format!(
                " helsinki={broker}:1883 , ,zilina = other.svc:1883"
            )),
        );
        assert!(org.is_private_host_port_allowed(broker, 1883));
        assert!(!org.is_private_host_port_allowed(broker, 6060));
        assert!(org.is_private_host_port_allowed("other.svc", 1883));
        // The rest of the quotas stay as they were.
        assert_eq!(
            org.get_quotas().max_concurrent_streams,
            ekiben_core::OrgQuotas::default().max_concurrent_streams
        );
    }

    const SENSORS: &str = "mqtt-helsinki-sensors.jc-prod-mqtt.svc.cluster.local";
    const OVZDUSIE: &str = "mqtt-banskabystrica-ovzdusie.jc-prod-mqtt.svc.cluster.local";

    /// prod's brokers as the deployment lists them, each with its project.
    fn prod_hosts() -> Vec<PrivateHost> {
        private_hosts(&format!(
            "helsinki={SENSORS}:1883,banskabystrica={OVZDUSIE}:1883"
        ))
    }

    /// A stream reading a platform broker as the Portal renders it (jc-manifest `bento.rs`).
    fn mqtt_stream(host: &str, user: &str) -> serde_json::Value {
        serde_json::json!({
            "input": {"mqtt": {
                "urls": [format!("tcp://{host}:1883")],
                "client_id": "jc-helsinki-p-jc-prod-mqtt",
                "user": user,
                "topics": ["sensors/#"]
            }},
            "output": {"drop": {}}
        })
    }

    /// jc's ekiben on a loopback port with prod's broker list; answers its URL and token.
    async fn serve_ekiben(dir: &Path) -> (String, String) {
        use crate::setup::{Secret, Setup};
        let setup = Setup::prepare(dir, &[Module::Ekiben], &Default::default()).unwrap();
        let cfg = EkibenSection {
            file_root: Some(dir.join("files")),
            ..Default::default()
        };
        let siblings = Siblings::new(&[], "127.0.0.1:1".parse().unwrap());
        let app = build_with(
            &cfg,
            &setup,
            &Default::default(),
            &siblings,
            false,
            prod_hosts(),
        )
        .await
        .unwrap();
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runner = format!("http://{}", socket.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(socket, app).await });
        (runner, setup.value(Secret::EkibenToken).unwrap().to_owned())
    }

    /// The run-time gap of jc-prod-mqtt's report (section 3): the gateway admits the runner of
    /// the broker's project from the jc pod, so a stream of project helsinki that names
    /// banskabystrica's broker and runner username read banskabystrica's topics. ekiben's API
    /// took it; now it refuses it, and helsinki's own stream on its own broker is still taken.
    #[tokio::test]
    async fn a_posted_stream_naming_another_projects_broker_and_runner_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (runner, token) = serve_ekiben(dir.path()).await;
        let client = reqwest::Client::new();
        let post = |id: &str, body: serde_json::Value| {
            client
                .post(format!("{runner}/streams/{id}"))
                .bearer_auth(&token)
                .json(&body)
                .send()
        };
        let r = post(
            "helsinki.steal",
            mqtt_stream(OVZDUSIE, "pipeline-runner-banskabystrica"),
        )
        .await
        .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::BAD_REQUEST);
        let why = r.text().await.unwrap();
        assert!(why.contains("belongs to another project"), "{why}");
        let r = post(
            "helsinki.own",
            mqtt_stream(SENSORS, "pipeline-runner-helsinki"),
        )
        .await
        .unwrap();
        assert!(r.status().is_success(), "{}", r.status());
        // The refused stream is not there.
        let r = client
            .get(format!("{runner}/streams/helsinki.steal"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::NOT_FOUND);
    }

    /// A PUT is asked before the running stream stops: a refused one leaves it running; a
    /// resource posted through the API belongs to no project, so it names no broker.
    #[tokio::test]
    async fn a_put_or_a_resource_naming_a_broker_is_refused_and_the_running_stream_stays() {
        let dir = tempfile::tempdir().unwrap();
        let (runner, token) = serve_ekiben(dir.path()).await;
        let client = reqwest::Client::new();
        let url = format!("{runner}/streams/helsinki.own");
        let own = mqtt_stream(SENSORS, "pipeline-runner-helsinki");
        let r = client
            .post(&url)
            .bearer_auth(&token)
            .json(&own)
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "{}", r.status());
        let forged = mqtt_stream(SENSORS, "pipeline-runner-banskabystrica");
        let r = client
            .put(&url)
            .bearer_auth(&token)
            .json(&forged)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::BAD_REQUEST);
        let r = client.get(&url).bearer_auth(&token).send().await.unwrap();
        assert!(
            r.status().is_success(),
            "the running stream stays: {}",
            r.status()
        );
        let r = client
            .post(format!("{runner}/resources/input/sensors"))
            .bearer_auth(&token)
            .json(&mqtt_stream(SENSORS, "pipeline-runner-helsinki")["input"])
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), reqwest::StatusCode::BAD_REQUEST);
        assert!(r.text().await.unwrap().contains("another project"));
    }

    #[test]
    fn another_projects_broker_host_is_refused() {
        let hosts = prod_hosts();
        let own_user = "pipeline-runner-helsinki";
        assert_eq!(
            admit(&hosts, Some("helsinki.p"), &mqtt_stream(OVZDUSIE, own_user)),
            Err(format!(
                "names the private host {OVZDUSIE}, which belongs to another project"
            ))
        );
        // Any spelling the network check would still open, and any component naming it.
        for text in [
            OVZDUSIE.to_uppercase(),
            format!("{OVZDUSIE}.:1883"),
            format!("ws://{OVZDUSIE}:1883/mqtt"),
            format!("root = \"tcp://{OVZDUSIE}:1883\""),
        ] {
            let config = serde_json::json!({"output": {"socket": {"address": text}}});
            assert!(
                admit(&hosts, Some("helsinki.p"), &config).is_err(),
                "{text}"
            );
        }
        // Its own project's stream reads it.
        let theirs = mqtt_stream(OVZDUSIE, "pipeline-runner-banskabystrica");
        assert_eq!(admit(&hosts, Some("banskabystrica.p"), &theirs), Ok(()));
        // A name merely ending in the listed one is another host.
        let longer = format!("mqtt-x-{OVZDUSIE}");
        assert_eq!(
            admit(&hosts, Some("helsinki.p"), &mqtt_stream(&longer, own_user)),
            Ok(())
        );
        // A stream with no project in its id names no private host.
        assert!(admit(&hosts, Some("orphan"), &mqtt_stream(SENSORS, "u")).is_err());
    }

    #[test]
    fn a_forged_runner_username_is_refused() {
        let hosts = prod_hosts();
        for forged in [
            "pipeline-runner-banskabystrica",
            "pipeline-runner",
            "Pipeline-Runner-helsinki",
            "pipeline-runner-helsinki-x",
        ] {
            assert_eq!(
                admit(&hosts, Some("helsinki.p"), &mqtt_stream(SENSORS, forged)),
                Err("names a runner username other than its own project's (pipeline-runner-helsinki)".to_owned()),
                "{forged}"
            );
        }
        assert_eq!(
            admit(
                &hosts,
                None,
                &mqtt_stream("mqtt.hsl.fi", "pipeline-runner-helsinki")
            ),
            Err("names a runner username, which only a project's stream may use".to_owned())
        );
        let capture = mqtt_stream(SENSORS, "jc-portal-capture");
        assert!(
            admit(&hosts, Some("helsinki.p"), &capture)
                .unwrap_err()
                .contains("capture username")
        );
        // An outside broker with its own account is no business of this check.
        assert_eq!(
            admit(&hosts, Some("helsinki.p"), &mqtt_stream("mqtt.hsl.fi", "")),
            Ok(())
        );
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn the_capture_username_is_the_gateways() {
        assert_eq!(CAPTURE_USERNAME, context_gateway::mqtt::CAPTURE_USERNAME);
    }

    /// A stream saved before these checks is started at the next start through
    /// `StreamManager::restore` (ekiben_pg logs the reason and starts the rest): refused.
    #[tokio::test]
    async fn a_restored_saved_stream_naming_another_projects_broker_is_not_started() {
        let manager = StreamManager::new(
            build_full_registry(),
            ResourceManager::new(),
            None,
            EnvAccess::None,
        )
        .with_admission(admission(prod_hosts()));
        let saved = mqtt_stream(OVZDUSIE, "pipeline-runner-banskabystrica");
        let e = manager
            .restore(OrgId::default(), "helsinki.saved".into(), saved)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("belongs to another project"), "{e}");
        assert!(manager.list(None).await.is_empty());
        let own = mqtt_stream(SENSORS, "pipeline-runner-helsinki");
        manager
            .restore(OrgId::default(), "helsinki.own".into(), own)
            .await
            .unwrap();
        assert_eq!(manager.list(None).await.len(), 1);
        manager.stop_all(std::time::Duration::from_secs(1)).await;
    }

    #[test]
    fn a_private_host_entry_admits_only_its_projects_streams() {
        let hosts = private_hosts(&format!(
            "banskabystrica={OVZDUSIE}:1883, {SENSORS}:1883, ={SENSORS}"
        ));
        // The entries naming no project open nothing.
        assert_eq!(
            hosts,
            [PrivateHost {
                project: "banskabystrica".into(),
                entry: format!("{OVZDUSIE}:1883"),
                name: OVZDUSIE.into(),
            }]
        );
        let stream = mqtt_stream(OVZDUSIE, "pipeline-runner-helsinki");
        assert!(admit(&hosts, Some("helsinki.p"), &stream).is_err());
        let stream = mqtt_stream(OVZDUSIE, "pipeline-runner-banskabystrica");
        assert_eq!(admit(&hosts, Some("banskabystrica.p"), &stream), Ok(()));
    }

    /// A refusal is logged with the stream and its reason, and with no variable's value.
    #[test]
    fn a_refusal_is_logged_with_its_reason() {
        #[derive(Clone, Default)]
        struct Buf(Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = Buf::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        let mut stream = mqtt_stream(OVZDUSIE, "pipeline-runner-banskabystrica");
        stream["input"]["mqtt"]["password"] = "filled-in-secret".into();
        let refused = tracing::subscriber::with_default(subscriber, || {
            admission(prod_hosts())(Some("helsinki.steal"), &stream)
        });
        assert!(refused.is_err());
        let log = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert!(log.contains("ekiben refused a stream"), "{log}");
        assert!(
            log.contains("helsinki.steal") && log.contains("belongs to another project"),
            "{log}"
        );
        assert!(!log.contains("filled-in-secret"), "{log}");
    }

    #[test]
    fn a_given_variable_wins_over_a_pipeline_credential_of_the_same_name() {
        let resources = ResourceManager::new();
        let runner = HostRunner {
            org: resources.get_or_create_org(&OrgId::default()),
            given: HashMap::from([("SHARED".to_owned(), "operator".to_owned())]),
        };
        let merged = runner.merged(BTreeMap::from([
            ("SHARED".to_owned(), "pipeline".to_owned()),
            ("GOLEMIO_KEY".to_owned(), "k".to_owned()),
        ]));
        assert_eq!(merged["SHARED"], "operator");
        assert_eq!(merged["GOLEMIO_KEY"], "k");
        // A later sync without the credential drops it again.
        assert_eq!(runner.merged(BTreeMap::new()).len(), 1);
    }

    /// jc's slug-mode siblings: every module on one listener.
    fn slug_siblings() -> Siblings {
        siblings_at("city.example", "https")
    }

    fn siblings_at(base_host: &str, scheme: &str) -> Siblings {
        let cfg = crate::config::Config::merge(
            crate::config::Cli {
                role: Some("all".into()),
                base_host: Some(base_host.into()),
                scheme: Some(scheme.into()),
                ..Default::default()
            },
            Default::default(),
        )
        .unwrap();
        let plans = crate::app::plan_mounts(&cfg).unwrap();
        Siblings::new(&plans, "0.0.0.0:8443".parse().unwrap())
    }

    #[test]
    fn the_runner_variables_are_jcs_own_mounts() {
        let siblings = slug_siblings();
        let vars = runner_defaults(
            &siblings,
            &crate::setup::Realm::default(),
            Some("helsinki-pipelines"),
            None,
        );
        assert_eq!(vars["JC_GATEWAY_URL"], "http://127.0.0.1:8443");
        assert_eq!(vars["JC_GATEWAY_HOST"], "127.0.0.1:8443");
        assert_eq!(
            vars["JC_TOKEN_URL"],
            "https://city.example/id/realms/jc/protocol/openid-connect/token"
        );
        // The domain first-start provisioning gives the Organization.
        assert_eq!(vars["JC_ORG_DOMAIN"], "city.example");
        assert_eq!(vars["JC_CLIENT_ID"], "helsinki-pipelines");
        // An outside broker's client id names the installation, so the runner takes the stream.
        assert_eq!(vars["JC_MQTT_NAMESPACE"], "city-example");
        assert!(!vars.keys().any(|k| k.starts_with("JC_CLIENT_SECRET")));
        let named = runner_defaults(
            &siblings,
            &crate::setup::Realm::default(),
            None,
            Some("hel.fi".into()),
        );
        assert_eq!(named["JC_ORG_DOMAIN"], "hel.fi");
        // T-6769: a mapping mints the owner segment of its ids from the Organization's id.
        assert_eq!(named["JC_ORG_ID"], "fi_hel");
        assert_eq!(named["JC_MQTT_NAMESPACE"], "hel-fi");
        assert!(!named.contains_key("JC_CLIENT_ID"));
        // Without the gateway and kaisatsu nothing points at them.
        let alone = runner_defaults(
            &Siblings::new(&[], "127.0.0.1:1".parse().unwrap()),
            &crate::setup::Realm::default(),
            None,
            None,
        );
        assert_eq!(
            alone.keys().collect::<Vec<_>>(),
            ["JC_MQTT_NAMESPACE", "JC_ORG_DOMAIN", "JC_ORG_ID"]
        );
    }

    #[test]
    fn streams_reach_only_the_gateway_door_and_the_token_endpoint_of_jcs_listener() {
        // The public token endpoint is a name: only the gateway door is on the listener.
        let siblings = slug_siblings();
        let target = host_target(
            &siblings,
            &runner_defaults(&siblings, &crate::setup::Realm::default(), None, None),
            None,
        )
        .unwrap();
        assert_eq!(target.path_prefixes, ["/api/endpoint"]);
        // A loopback rehearsal whose public base is the listener's own address.
        let siblings = siblings_at("127.0.0.1:8443", "http");
        let target = host_target(
            &siblings,
            &runner_defaults(&siblings, &crate::setup::Realm::default(), None, None),
            None,
        )
        .unwrap();
        let admits = |u: &str| target.admits(&url::Url::parse(u).unwrap());
        assert!(admits(
            "http://127.0.0.1:8443/api/endpoint/abc/ngsi-ld/v1/entityOperations/upsert"
        ));
        assert!(admits(
            "http://127.0.0.1:8443/id/realms/jc/protocol/openid-connect/token"
        ));
        for refused in [
            "http://127.0.0.1:8443/broker/ngsi-ld/v1/entities",
            "http://127.0.0.1:8443/id/admin/realms/jc/users",
            "http://127.0.0.1:8443/streams/streams",
            "http://127.0.0.1:8443/api/v1/projects",
            "http://127.0.0.1:8443/api/mcp",
            "http://127.0.0.1:9090/api/endpoint/abc",
        ] {
            assert!(!admits(refused), "{refused}");
        }
        // With the Portal beside it: its internal listener's pipeline routes, once bound.
        let portal = std::sync::Arc::new(std::sync::OnceLock::new());
        let target = host_target(
            &siblings,
            &runner_defaults(&siblings, &crate::setup::Realm::default(), None, None),
            Some(portal.clone()),
        )
        .unwrap();
        let admits = |u: &str| target.admits(&url::Url::parse(u).unwrap());
        let outcomes = "http://127.0.0.1:55551/internal/pipelines/helsinki/p/outcomes";
        assert!(!admits(outcomes), "refused until the Portal bound it");
        portal.set("127.0.0.1:55551".parse().unwrap()).unwrap();
        assert!(admits(outcomes));
        assert!(admits("http://127.0.0.1:55551/internal/pipeline-tests/abc"));
        for refused in [
            "http://127.0.0.1:55551/internal/revocations",
            "http://127.0.0.1:55551/internal/agent-runs/1/mcp",
            "http://127.0.0.1:55551/internal/model-key",
            "http://127.0.0.1:55552/internal/pipelines/helsinki/p/outcomes",
        ] {
            assert!(!admits(refused), "{refused}");
        }
        // Nothing of jc's listener when neither module is there.
        let alone = Siblings::new(&[], "127.0.0.1:1".parse().unwrap());
        assert!(
            host_target(
                &alone,
                &runner_defaults(&alone, &crate::setup::Realm::default(), None, None),
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn an_explicit_ekiben_var_wins_over_jcs_defaults_and_client_secrets() {
        let defaults = BTreeMap::from([
            ("JC_GATEWAY_URL".to_owned(), "http://127.0.0.1:1".to_owned()),
            ("JC_ORG_DOMAIN".to_owned(), "city.example".to_owned()),
        ]);
        let secret = |v: &str| jc_secret::Secret::new(v.to_owned());
        let clients = BTreeMap::from([
            ("JC_CLIENT_SECRET_ZILINA".to_owned(), secret("from-file")),
            ("JC_CLIENT_SECRET_PRAHA".to_owned(), secret("praha-file")),
        ]);
        let given = BTreeMap::from([
            ("JC_GATEWAY_URL".to_owned(), secret("http://gw")),
            ("JC_CLIENT_SECRET_ZILINA".to_owned(), secret("operator")),
        ]);
        let vars = variables(&defaults, &clients, &given);
        assert_eq!(vars["JC_GATEWAY_URL"], "http://gw");
        assert_eq!(vars["JC_CLIENT_SECRET_ZILINA"], "operator");
        assert_eq!(vars["JC_CLIENT_SECRET_PRAHA"], "praha-file");
        assert_eq!(vars["JC_ORG_DOMAIN"], "city.example");
    }

    /// jc names a project's client secret as the Portal's render does, and no stream the Portal
    /// renders for one project names another project's variable (PF-46). The runner's one
    /// environment holds every project's secret in project mode, as on dev's local and
    /// production profiles: isolation rests on the render alone.
    #[cfg(feature = "portal")]
    #[test]
    fn a_stream_of_one_project_names_only_its_own_client_secret() {
        use jc_core::kinds::data_source::DataSourceSpec;
        use jc_core::kinds::pipeline::PipelineSpec;
        let pipeline: PipelineSpec = serde_json::from_value(serde_json::json!({
            "class": "auto",
            "period": "60s",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "feed" } },
            "compute": { "kind": "bloblang", "bloblang": "root = this" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:city.example:zilina-mesto:all"
        }))
        .unwrap();
        let source: DataSourceSpec = serde_json::from_value(serde_json::json!({
            "type": "http",
            "http": { "url": "https://data.example.org/feed.json" }
        }))
        .unwrap();
        let rendered = joinedcontext_portal::reconciler::streams::render_stream(
            &pipeline,
            "drepo",
            "zilina-mesto",
            &source,
            "feed",
            "abcdefghijklmnopqrstuvwxyz",
            None,
        )
        .unwrap()
        .to_string();
        let mut named: Vec<&str> = rendered
            .match_indices("JC_CLIENT_SECRET")
            .map(|(at, _)| {
                let rest = &rendered[at..];
                &rest[..rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len())]
            })
            .collect();
        named.dedup();
        assert_eq!(
            named,
            [crate::config::client_secret_var("zilina-mesto")],
            "{rendered}"
        );
        assert_eq!(
            crate::config::client_secret_var("zilina-mesto"),
            "JC_CLIENT_SECRET_ZILINA_MESTO"
        );
        assert!(!rendered.contains(&crate::config::client_secret_var("helsinki")));
        assert!(rendered.contains("${JC_TOKEN_URL}") && rendered.contains("${JC_GATEWAY_URL}"));
    }

    #[test]
    fn file_root_is_created_private() {
        let dir = tempfile::tempdir().unwrap();
        let root = file_root(Some(&dir.path().join("a/b"))).unwrap();
        assert!(root.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
        let default = file_root(None).unwrap();
        assert!(default.ends_with(format!("jc-ekiben-{}", std::process::id())));
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, "x").unwrap();
        assert!(file_root(Some(&blocker.join("sub"))).is_err());
    }

    /// The deployment's resources file is the runner's `-r`: without it every stream naming a
    /// project cache or a shared processor was refused (`events_tail` 774 times in the prod
    /// rehearsal's jc.log).
    #[tokio::test]
    async fn a_stream_naming_a_resource_of_the_resources_file_is_taken() {
        use crate::setup::{Secret, Setup};
        let dir = tempfile::tempdir().unwrap();
        let setup = Setup::prepare(dir.path(), &[Module::Ekiben], &Default::default()).unwrap();
        let resources = dir.path().join("resources.yaml");
        std::fs::write(
            &resources,
            "cache_resources:\n  - label: helsinki__gtfs\n    memory:\n      default_ttl: 72h\n",
        )
        .unwrap();
        let cfg = EkibenSection {
            file_root: Some(dir.path().join("files")),
            resources_file: Some(resources),
            ..Default::default()
        };
        let siblings = Siblings::new(&[], "127.0.0.1:1".parse().unwrap());
        let app = build(&cfg, &setup, &Default::default(), &siblings, false)
            .await
            .unwrap();
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runner = format!("http://{}", socket.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(socket, app).await });
        let stream = serde_json::json!({
            "input": {"generate": {"interval": "1h", "mapping": "root = {}"}},
            "pipeline": {"processors": [
                {"cache": {"resource": "helsinki__gtfs", "operator": "get", "key": "k"}}
            ]},
            "output": {"drop": {}}
        });
        let r = reqwest::Client::new()
            .post(format!("{runner}/streams/helsinki.hsl-hfp-vehicles"))
            .bearer_auth(setup.value(Secret::EkibenToken).unwrap())
            .json(&stream)
            .send()
            .await
            .unwrap();
        let status = r.status();
        assert!(status.is_success(), "{status}: {}", r.text().await.unwrap());
    }

    /// The Portal's stream deletes reach jc's ekiben: the deployer sends the bearer of the token
    /// file siblings.rs hands the Portal (`JC_PORTAL_PIPELINE_RUNNER_TOKEN_FILE`); a deployer
    /// without it is refused 401 and the stream stays (jc-migrate's rehearsal, G15).
    #[cfg(feature = "portal")]
    #[tokio::test]
    async fn a_portal_issued_delete_reaches_ekiben() {
        use crate::setup::{Secret, Setup};
        use joinedcontext_portal::reconciler::streams::StreamDeployer;
        let dir = tempfile::tempdir().unwrap();
        let setup = Setup::prepare(dir.path(), &[Module::Ekiben], &Default::default()).unwrap();
        let cfg = EkibenSection {
            file_root: Some(dir.path().join("files")),
            ..Default::default()
        };
        let siblings = Siblings::new(&[], "127.0.0.1:1".parse().unwrap());
        let app = build(&cfg, &setup, &Default::default(), &siblings, false)
            .await
            .unwrap();
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runner = format!("http://{}", socket.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(socket, app).await });
        let token = setup.value(Secret::EkibenToken).unwrap().to_owned();
        let url = format!("{runner}/streams/helsinki.gone");
        let client = reqwest::Client::new();
        let stream = serde_json::json!({
            "input": {"generate": {"interval": "1h", "mapping": "root = {}"}},
            "output": {"drop": {}}
        });
        let r = client
            .post(&url)
            .bearer_auth(&token)
            .json(&stream)
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "{}", r.status());
        let held = || async {
            client
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .unwrap()
                .status()
        };

        StreamDeployer::new(runner.clone())
            .retire("helsinki", &["gone".to_owned()])
            .await;
        assert!(
            held().await.is_success(),
            "a delete without the bearer is refused"
        );

        StreamDeployer::new(runner)
            .with_runner_token(Some(token.clone()))
            .retire("helsinki", &["gone".to_owned()])
            .await;
        assert_eq!(
            held().await,
            reqwest::StatusCode::NOT_FOUND,
            "the Portal's delete reached ekiben"
        );
    }
}
