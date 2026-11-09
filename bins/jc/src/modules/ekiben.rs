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
//!   `JC_CLIENT_ID`, and each `JC_CLIENT_SECRET_<PROJECT>` of `[ekiben] client_secrets_dir`
//!   (project mode: the Portal's default, as on the local and production profiles);
//! - private and loopback HTTP targets stay refused (ekiben's default quotas), so a stream cannot
//!   call the other modules' listeners from inside the process; the one exception is `jc`'s own
//!   listener under the gateway's `/api/endpoint` and kaisatsu's token endpoint
//!   ([`HostTarget`]), the two doors a stream reaches on dev.
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
    ServerAuthConfig, ServerState, StreamManager, build_full_registry, build_router,
};
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
    if let Some(target) = host_target(siblings, &defaults) {
        tracing::info!(addr = %target.addr, prefixes = ?target.path_prefixes, "ekiben streams reach jc's gateway and token endpoint");
        resources = resources.with_host_target(target);
    }
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
    );
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

/// The default organization's variable map, as the Portal's reconciler sees the runner: what
/// `jc` was given (`EKIBEN_VAR_*`) plus the pipeline credentials of the last sync. A given name
/// wins over a pipeline's, so a project cannot replace a variable the operator set.
pub struct HostRunner {
    org: Arc<OrgContext>,
    given: HashMap<String, String>,
}

impl HostRunner {
    /// The variables after a sync handed `resolved`.
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
/// harness client. None is a secret. `JC_MQTT_NAMESPACE` stays unset: `jc` runs no MQTT broker.
fn runner_defaults(
    siblings: &Siblings,
    realm: &crate::setup::Realm,
    harness_client: Option<&str>,
    org_domain: Option<String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(gateway) = siblings.get(Module::Gateway) {
        out.insert("JC_GATEWAY_URL".to_owned(), gateway.internal.clone());
        let host = gateway
            .internal
            .split_once("://")
            .map_or(gateway.internal.as_str(), |(_, rest)| rest);
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
    out.insert("JC_ORG_DOMAIN".to_owned(), org_domain);
    if let Some(client) = harness_client {
        out.insert("JC_CLIENT_ID".to_owned(), client.to_owned());
    }
    out
}

/// The part of `jc`'s listener a stream may call: the gateway's Endpoint door and kaisatsu's
/// token endpoint, where their URLs name that listener's own address (the gateway's internal URL
/// in slug mode; the token endpoint only when the public base is that address, as in a loopback
/// rehearsal with `--base-host 127.0.0.1:<port>`). Any other URL is a public name and passes
/// ekiben's ordinary checks; a loopback name (`localhost`) stays refused.
fn host_target(siblings: &Siblings, defaults: &BTreeMap<String, String>) -> Option<HostTarget> {
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
    (!prefixes.is_empty()).then_some(HostTarget {
        addr: siblings.listener,
        path_prefixes: prefixes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(
            !vars.contains_key("JC_MQTT_NAMESPACE"),
            "jc runs no MQTT broker"
        );
        assert!(!vars.keys().any(|k| k.starts_with("JC_CLIENT_SECRET")));
        let named = runner_defaults(
            &siblings,
            &crate::setup::Realm::default(),
            None,
            Some("hel.fi".into()),
        );
        assert_eq!(named["JC_ORG_DOMAIN"], "hel.fi");
        assert!(!named.contains_key("JC_CLIENT_ID"));
        // Without the gateway and kaisatsu nothing points at them.
        let alone = runner_defaults(
            &Siblings::new(&[], "127.0.0.1:1".parse().unwrap()),
            &crate::setup::Realm::default(),
            None,
            None,
        );
        assert_eq!(alone.keys().collect::<Vec<_>>(), ["JC_ORG_DOMAIN"]);
    }

    #[test]
    fn streams_reach_only_the_gateway_door_and_the_token_endpoint_of_jcs_listener() {
        // The public token endpoint is a name: only the gateway door is on the listener.
        let siblings = slug_siblings();
        let target = host_target(
            &siblings,
            &runner_defaults(&siblings, &crate::setup::Realm::default(), None, None),
        )
        .unwrap();
        assert_eq!(target.path_prefixes, ["/api/endpoint"]);
        // A loopback rehearsal whose public base is the listener's own address.
        let siblings = siblings_at("127.0.0.1:8443", "http");
        let target = host_target(
            &siblings,
            &runner_defaults(&siblings, &crate::setup::Realm::default(), None, None),
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
        // Nothing of jc's listener when neither module is there.
        let alone = Siblings::new(&[], "127.0.0.1:1".parse().unwrap());
        assert!(
            host_target(
                &alone,
                &runner_defaults(&alone, &crate::setup::Realm::default(), None, None)
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
