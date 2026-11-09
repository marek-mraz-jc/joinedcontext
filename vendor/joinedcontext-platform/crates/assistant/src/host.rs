//! What a host of the assistant runs beside the library's chat: its settings read through one
//! lookup (the `jc-assistant` binary's environment, or `jc`'s siblings with an explicit
//! environment value winning), the database pool, the probe and the minute's work. The lookup is
//! the caller's: a key it answers with a blank value counts as set, so the caller filters blanks.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use jc_core::ClientCredential;
use sqlx::PgPool;

use crate::chat::{self, ChatState};
use crate::crawl::Crawler;
use crate::embed::{embed_until, Embedder};
use crate::fill_terms;
use crate::service::{self, ChatConfig, ChatSettings, Setting};
use crate::worker;

/// The organization checkout of a deployment: `JC_ASSISTANT_REPO_DIR`, `JC_ASSISTANT_PROJECTS_DIR`
/// and `JC_ASSISTANT_ASSEMBLY_DIR` (default the system temporary directory).
pub fn checkout(lookup: impl Fn(&str) -> Option<String>) -> Option<worker::Checkout> {
    let organization = PathBuf::from(lookup("JC_ASSISTANT_REPO_DIR")?);
    Some(worker::Checkout {
        organization,
        projects: lookup("JC_ASSISTANT_PROJECTS_DIR").map(PathBuf::from),
        assembly: lookup("JC_ASSISTANT_ASSEMBLY_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("jc-assistant-assembly")),
    })
}

/// `JC_ASSISTANT_EMBED_THREADS`: 2 when unset, else a number from 1 to 16.
pub fn embed_threads(lookup: &impl Fn(&str) -> Option<String>) -> Result<usize, String> {
    match lookup("JC_ASSISTANT_EMBED_THREADS").map(|value| value.parse::<usize>()) {
        None => Ok(2),
        Some(Ok(threads)) if (1..=16).contains(&threads) => Ok(threads),
        Some(_) => Err("JC_ASSISTANT_EMBED_THREADS is a number from 1 to 16".into()),
    }
}

/// The chat's Keycloak credential: the pod's projected token or the client secret, never both.
pub fn credential(lookup: &impl Fn(&str) -> Option<String>) -> Result<ClientCredential, String> {
    match (
        lookup("JC_ASSISTANT_CLIENT_ASSERTION_FILE"),
        lookup("JC_ASSISTANT_CLIENT_SECRET_FILE"),
    ) {
        (Some(_), Some(_)) => Err(
            "JC_ASSISTANT_CLIENT_ASSERTION_FILE and JC_ASSISTANT_CLIENT_SECRET_FILE \
             are both set; a federated client holds no secret beside it"
                .into(),
        ),
        (Some(file), None) => Ok(ClientCredential::Assertion(file.into())),
        (None, Some(secret_file)) => {
            let client_secret = std::fs::read_to_string(&secret_file)
                .map_err(|err| format!("JC_ASSISTANT_CLIENT_SECRET_FILE could not be read: {err}"))?
                .trim()
                .to_owned();
            if client_secret.is_empty() {
                return Err("JC_ASSISTANT_CLIENT_SECRET_FILE is empty".into());
            }
            Ok(ClientCredential::Secret(jc_core::Secret::new(
                client_secret,
            )))
        }
        (None, None) => Err("JC_ASSISTANT_CLIENT_ASSERTION_FILE or \
                             JC_ASSISTANT_CLIENT_SECRET_FILE is required"
            .into()),
    }
}

/// The key each [`Setting`] is read under.
pub fn env_key(setting: Setting) -> &'static str {
    match setting {
        Setting::GatewayUrl => "JC_ASSISTANT_GATEWAY_URL",
        Setting::ProxyUrl => "JC_ASSISTANT_PROXY_URL",
        Setting::FunctionsUrl => "JC_ASSISTANT_FUNCTIONS_URL",
        Setting::EndpointUrl => "JC_ASSISTANT_ENDPOINT_URL",
        Setting::PortalUrl => "JC_ASSISTANT_PORTAL_URL",
        Setting::VizUrl => "JC_ASSISTANT_VIZ_URL",
    }
}

/// The chat's settings under their `JC_ASSISTANT_*` names: the gateway, the model the chat asks
/// through the agent proxy and the addresses its answers link to. `credential` is how the
/// service proves its client; [`credential`] reads it from the lookup.
pub fn chat_settings(
    lookup: &impl Fn(&str) -> Option<String>,
    credential: ClientCredential,
) -> Result<ChatSettings, String> {
    let required = |setting| {
        let key = env_key(setting);
        lookup(key).ok_or_else(|| format!("{key} is required"))
    };
    Ok(ChatSettings {
        gateway_url: required(Setting::GatewayUrl)?,
        proxy_url: required(Setting::ProxyUrl)?,
        token_url: lookup("JC_ASSISTANT_TOKEN_URL").ok_or("JC_ASSISTANT_TOKEN_URL is required")?,
        client_id: lookup("JC_ASSISTANT_CLIENT_ID").unwrap_or_else(|| "jc-assistant".into()),
        credential,
        llm: lookup("JC_ASSISTANT_LLM").ok_or("JC_ASSISTANT_LLM is required")?,
        functions_url: lookup(env_key(Setting::FunctionsUrl)),
        portal_client: lookup("JC_ASSISTANT_PORTAL_CLIENT").unwrap_or_else(|| "portal-api".into()),
        public_origin: lookup("JC_ASSISTANT_PUBLIC_ORIGIN"),
        endpoint_url: lookup(env_key(Setting::EndpointUrl)),
        portal_url: lookup(env_key(Setting::PortalUrl)),
        viz_url: lookup(env_key(Setting::VizUrl)),
    })
}

/// [`chat_settings`] checked by the library; a refusal is what failed and why, the why naming
/// the key.
pub fn chat_config(
    lookup: &impl Fn(&str) -> Option<String>,
    credential: ClientCredential,
) -> Result<ChatConfig, (&'static str, String)> {
    let settings =
        chat_settings(lookup, credential).map_err(|why| ("the chat is not configured", why))?;
    service::chat_config(settings).map_err(|invalid| {
        let what = match invalid.setting {
            Setting::GatewayUrl | Setting::ProxyUrl => "the chat is not configured",
            Setting::FunctionsUrl => "the sandbox address is not usable",
            Setting::EndpointUrl => "the public Endpoint address is not usable",
            Setting::PortalUrl => "the Portal address is not usable",
            Setting::VizUrl => "the chart renderer's address is not usable",
        };
        (what, invalid.problem.named(env_key(invalid.setting)))
    })
}

/// 10, not 4 (2026-10-26): a question searches 3 projects at once beside the embedding
/// backfill and the catalogue counts, and with 4 each waited ~9.7 s to acquire on prod.
/// Idle connections close after 60 s (T-6201): the background loops touch the pool more often
/// than sqlx's 10 min default, so the idle set never shrank and each held a ~19 MiB backend.
pub fn pool_options() -> sqlx::postgres::PgPoolOptions {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .min_connections(0)
        .idle_timeout(Duration::from_secs(60))
}

/// The chat route beside `/healthz` and `/readyz`, which answers 503 until `ready` is set.
pub fn probe(chat_state: Arc<ChatState>, ready: Arc<AtomicBool>) -> axum::Router {
    chat::router(chat_state)
        .route("/healthz", axum::routing::get(|| async { "ok" }))
        .route(
            "/readyz",
            axum::routing::get(move || {
                let ready = ready.load(Ordering::Relaxed);
                async move {
                    if ready {
                        (axum::http::StatusCode::OK, "ready")
                    } else {
                        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "starting")
                    }
                }
            }),
        )
}

/// How long a new pod waits for its first warm-up before it turns ready: past the 20 s a
/// connector's list may take, well inside the readiness probe's patience.
const FIRST_WARM: Duration = Duration::from_secs(30);

/// What the minute's work reads and keeps between minutes.
pub struct Minute {
    pub pool: PgPool,
    pub embedder: Embedder,
    pub crawler: Crawler,
    pub reader: crate::catalogue::Reader,
    pub organization: String,
    /// The worker's name in the queue.
    pub name: String,
    pub checkout: worker::Checkout,
    pub chat_state: Arc<ChatState>,
    pub ready: Arc<AtomicBool>,
    pub catalogue_digests: BTreeMap<(String, String), String>,
    /// Origins the host adds to a deployment's `allowedOrigins`, by `publicId`: the pages of the
    /// host's own catalogue, which frame its assistant (each `https://host[:port]`).
    pub extra_origins: BTreeMap<String, Vec<String>>,
}

impl Minute {
    /// A worker over `checkout` with nothing kept yet and no origin added.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: PgPool,
        embedder: Embedder,
        crawler: Crawler,
        reader: crate::catalogue::Reader,
        organization: String,
        name: String,
        checkout: worker::Checkout,
        chat_state: Arc<ChatState>,
        ready: Arc<AtomicBool>,
    ) -> Self {
        Self {
            pool,
            embedder,
            crawler,
            reader,
            organization,
            name,
            checkout,
            chat_state,
            ready,
            catalogue_digests: BTreeMap::new(),
            extra_origins: BTreeMap::new(),
        }
    }

    /// Reads the manifests, queues what is due or changed, works the queue and embeds.
    pub async fn work(&mut self) {
        let snapshot = match worker::snapshot(&self.checkout) {
            Ok(mut snapshot) => {
                add_origins(&mut snapshot, &self.extra_origins);
                Arc::new(snapshot)
            }
            Err(err) => {
                // A checkout mid-update is read again next minute; the queue keeps its jobs.
                tracing::warn!(%err, "manifests not read this minute");
                return;
            }
        };
        if let Ok(mut held) = self.chat_state.snapshot.write() {
            *held = Arc::clone(&snapshot);
        }
        // Routing's type lists, ahead of the first question (T-4611). A new pod turns ready only
        // once its first warm-up listed them, or after `FIRST_WARM`: ready at once, its first
        // questions met no connector and were answered as if the data held nothing (T-5782).
        let warming = tokio::spawn(chat::warm_types(Arc::clone(&self.chat_state)));
        if !self.ready.load(Ordering::Relaxed)
            && tokio::time::timeout(FIRST_WARM, warming).await.is_err()
        {
            tracing::warn!(
                seconds = FIRST_WARM.as_secs(),
                "the first warm-up did not finish in time; ready without every type list"
            );
        }
        // The context pages' entity counts, so no visitor's request asks the gateway (T-4692).
        tokio::spawn(chat::context::count(Arc::clone(&self.chat_state)));
        let sources = &snapshot.sources;
        self.ready.store(true, Ordering::Relaxed);
        let pool = &self.pool;
        let now = time::OffsetDateTime::now_utc()
            .replace_second(0)
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        if let Err(err) = chat::forget_expired(pool).await {
            tracing::warn!(%err, "expired conversations not deleted this minute");
        }
        match worker::enqueue_due(pool, sources, now).await {
            Ok(0) => {}
            Ok(queued) => tracing::info!(queued, "sources due"),
            Err(err) => tracing::warn!(%err, "sources not queued this minute"),
        }
        match worker::enqueue_changed(pool, sources, &mut self.catalogue_digests).await {
            Ok(0) => {}
            Ok(queued) => tracing::info!(queued, "catalogues changed"),
            Err(err) => tracing::warn!(%err, "changed catalogues not queued this minute"),
        }
        // Work what is ready, one job at a time, then wait for the next minute.
        loop {
            let worked = worker::work_one(
                pool,
                &self.crawler,
                &self.reader,
                &self.organization,
                &self.name,
                sources,
            )
            .await;
            match worked {
                Ok(true) => continue,
                Ok(false) => break,
                Err(err) => {
                    tracing::warn!(%err, "the queue could not be worked");
                    break;
                }
            }
        }
        // Then the passages without an embedding: what this minute's crawls stored and what an
        // earlier failure left. The projects share EMBED_BUDGET of the minute and the rest waits
        // for the next one, so a large first crawl never starves the queue; a question still goes
        // before any passage (T-4751).
        let projects: BTreeSet<&str> = sources
            .iter()
            .map(|source| source.project.as_str())
            .collect();
        fill_all_terms(pool, &projects).await;
        let until = std::time::Instant::now() + EMBED_BUDGET;
        for project in projects {
            match embed_until(pool, &self.embedder, project, until).await {
                Ok(0) => {}
                Ok(embedded) => tracing::info!(project, embedded, "passages embedded"),
                Err(err) => tracing::warn!(project, %err, "passages not embedded this minute"),
            }
        }
    }
}

/// Adds the host's origins to the deployments they name, each origin once.
fn add_origins(snapshot: &mut worker::Snapshot, extra: &BTreeMap<String, Vec<String>>) {
    for deployment in &mut snapshot.deployments {
        let Some(origins) = extra.get(&deployment.spec.public_id) else {
            continue;
        };
        let allowed = &mut deployment.spec.allowed_origins;
        for origin in origins {
            if !allowed.contains(origin) {
                allowed.push(origin.clone());
            }
        }
    }
}

/// The terms of the chunks stored before chunk_terms existed (T-5799), a batch at a time within
/// TERMS_BUDGET; a project with none left costs one empty index read.
async fn fill_all_terms(pool: &PgPool, projects: &BTreeSet<&str>) {
    let until = std::time::Instant::now() + TERMS_BUDGET;
    for project in projects {
        let mut filled = 0;
        while std::time::Instant::now() < until {
            match fill_terms(pool, project, TERMS_BATCH).await {
                Ok(0) => break,
                Ok(batch) => filled += batch,
                Err(err) => {
                    tracing::warn!(project, %err, "passage terms not filled this minute");
                    break;
                }
            }
        }
        if filled > 0 {
            tracing::info!(project, filled, "passage terms filled");
        }
    }
}

/// The share of each worker minute, and the chunks per transaction, that give the chunks stored
/// before T-5799 their terms: ~130k on prod, a few minutes after the first boot.
const TERMS_BUDGET: Duration = Duration::from_secs(10);
const TERMS_BATCH: i64 = 1000;

/// The share of each worker minute that embeds passages (T-4749): a count of 256 used a few
/// seconds and left a first full index waiting for days.
const EMBED_BUDGET: Duration = Duration::from_secs(45);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| map.get(key).cloned()
    }

    fn chat_env(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        let mut pairs = vec![
            ("JC_ASSISTANT_CLIENT_ASSERTION_FILE", "/var/run/token"),
            ("JC_ASSISTANT_GATEWAY_URL", "http://gateway:8080/"),
            ("JC_ASSISTANT_PROXY_URL", "http://proxy:8080"),
            ("JC_ASSISTANT_TOKEN_URL", "https://idm.test/token"),
            ("JC_ASSISTANT_LLM", "a-model"),
        ];
        pairs.extend_from_slice(extra);
        pairs
    }

    fn config(
        lookup: &impl Fn(&str) -> Option<String>,
    ) -> Result<ChatConfig, (&'static str, String)> {
        let credential = credential(lookup).map_err(|why| ("the chat is not configured", why))?;
        chat_config(lookup, credential)
    }

    /// A service address from `key`: scheme and authority only, returned without its last `/`.
    fn base(lookup: &impl Fn(&str) -> Option<String>, key: &str) -> Result<String, String> {
        let value = lookup(key).ok_or_else(|| format!("{key} is required"))?;
        service::base(&value).map_err(|problem| problem.named(key))
    }

    fn optional_base(
        lookup: &impl Fn(&str) -> Option<String>,
        key: &str,
    ) -> Result<Option<String>, String> {
        match lookup(key) {
            None => Ok(None),
            Some(_) => base(lookup, key).map(Some),
        }
    }

    fn endpoint_base(lookup: &impl Fn(&str) -> Option<String>) -> Result<Option<url::Url>, String> {
        let key = "JC_ASSISTANT_ENDPOINT_URL";
        lookup(key)
            .map(|value| service::endpoint_base(&value).map_err(|problem| problem.named(key)))
            .transpose()
    }

    fn viz_url(lookup: &impl Fn(&str) -> Option<String>) -> Result<Option<url::Url>, String> {
        let key = "JC_ASSISTANT_VIZ_URL";
        lookup(key)
            .map(|value| service::viz_url(&value).map_err(|problem| problem.named(key)))
            .transpose()
    }

    #[test]
    fn embed_threads_default_to_two_and_stay_between_one_and_sixteen() {
        assert_eq!(embed_threads(&env(&[])), Ok(2));
        assert_eq!(
            embed_threads(&env(&[("JC_ASSISTANT_EMBED_THREADS", "16")])),
            Ok(16)
        );
        for wrong in ["0", "17", "two", "-1"] {
            assert!(embed_threads(&env(&[("JC_ASSISTANT_EMBED_THREADS", wrong)])).is_err());
        }
    }

    #[test]
    fn a_base_is_scheme_and_authority_only() {
        let key = "JC_ASSISTANT_GATEWAY_URL";
        assert_eq!(
            base(&env(&[(key, "http://gw:8080/")]), key),
            Ok("http://gw:8080".into())
        );
        assert_eq!(base(&env(&[]), key), Err(format!("{key} is required")));
        for wrong in ["http://gw/mcp", "http://gw/?a=1"] {
            assert_eq!(
                base(&env(&[(key, wrong)]), key),
                Err(format!("{key} is scheme and authority only"))
            );
        }
        assert!(base(&env(&[(key, "not a url")]), key).is_err_and(|e| e.starts_with(key)));
        assert_eq!(optional_base(&env(&[]), key), Ok(None));
        assert!(optional_base(&env(&[(key, "http://gw/x")]), key).is_err());
    }

    #[test]
    fn the_viz_url_is_https_on_a_viz_host_or_unset() {
        let ok = viz_url(&env(&[(
            "JC_ASSISTANT_VIZ_URL",
            "https://viz.dev.city.test",
        )]));
        assert_eq!(
            ok.ok()
                .flatten()
                .map(|u| u.origin().ascii_serialization())
                .as_deref(),
            Some("https://viz.dev.city.test")
        );
        assert_eq!(viz_url(&env(&[])), Ok(None));
        for bad in [
            "http://viz.dev.city.test",
            "https://charts.dev.city.test",
            "https://viz.dev.city.test/x",
        ] {
            assert!(
                viz_url(&env(&[("JC_ASSISTANT_VIZ_URL", bad)])).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_endpoint_base_ends_with_a_slash_for_joins() {
        let set = env(&[("JC_ASSISTANT_ENDPOINT_URL", "https://data.city.test")]);
        let endpoint = endpoint_base(&set).expect("usable").expect("set");
        assert_eq!(
            endpoint.join("api/endpoint/air").expect("joins").as_str(),
            "https://data.city.test/api/endpoint/air"
        );
        assert_eq!(endpoint_base(&env(&[])), Ok(None));
    }

    #[test]
    fn the_credential_is_one_of_the_token_or_a_non_empty_secret() {
        let both = chat_env(&[("JC_ASSISTANT_CLIENT_SECRET_FILE", "/x")]);
        assert!(credential(&env(&both)).is_err_and(|e| e.contains("both set")));
        assert!(credential(&env(&[])).is_err_and(|e| e.contains("is required")));
        assert!(matches!(
            credential(&env(&chat_env(&[]))),
            Ok(ClientCredential::Assertion(path)) if path == std::path::Path::new("/var/run/token")
        ));

        let dir = std::env::temp_dir().join(format!("jc-assistant-secret-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let file = dir.join("secret");
        let path = file.to_str().expect("utf-8 temp dir");
        std::fs::write(&file, " s3cret\n").expect("written");
        assert!(matches!(
            credential(&env(&[("JC_ASSISTANT_CLIENT_SECRET_FILE", path)])),
            Ok(ClientCredential::Secret(secret)) if secret.expose() == "s3cret"
        ));
        std::fs::write(&file, "  \n").expect("written");
        assert_eq!(
            credential(&env(&[("JC_ASSISTANT_CLIENT_SECRET_FILE", path)])).err(),
            Some("JC_ASSISTANT_CLIENT_SECRET_FILE is empty".into())
        );
        let missing = dir.join("missing");
        let missing = missing.to_str().expect("utf-8 temp dir");
        assert!(
            credential(&env(&[("JC_ASSISTANT_CLIENT_SECRET_FILE", missing)]))
                .is_err_and(|e| e.contains("could not be read"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pool_releases_idle_connections_after_a_minute() {
        let options = pool_options();
        assert_eq!(options.get_idle_timeout(), Some(Duration::from_secs(60)));
        assert_eq!(options.get_min_connections(), 0);
        assert_eq!(options.get_max_connections(), 10);
    }

    #[test]
    fn a_refused_address_names_its_variable_and_what_it_is_for() {
        for (key, value, what, why) in [
            (
                "JC_ASSISTANT_VIZ_URL",
                "https://charts.city.test",
                "the chart renderer's address is not usable",
                "JC_ASSISTANT_VIZ_URL is https://viz.{domain}",
            ),
            (
                "JC_ASSISTANT_PORTAL_URL",
                "https://portal.city.test/x",
                "the Portal address is not usable",
                "JC_ASSISTANT_PORTAL_URL is scheme and authority only",
            ),
            (
                "JC_ASSISTANT_FUNCTIONS_URL",
                "http://fn/?a=1",
                "the sandbox address is not usable",
                "JC_ASSISTANT_FUNCTIONS_URL is scheme and authority only",
            ),
            (
                "JC_ASSISTANT_ENDPOINT_URL",
                "https://data.city.test/api",
                "the public Endpoint address is not usable",
                "JC_ASSISTANT_ENDPOINT_URL is scheme and authority only",
            ),
            (
                "JC_ASSISTANT_GATEWAY_URL",
                "http://gw/mcp",
                "the chat is not configured",
                "JC_ASSISTANT_GATEWAY_URL is scheme and authority only",
            ),
        ] {
            let pairs: Vec<_> = chat_env(&[])
                .into_iter()
                .filter(|(k, _)| *k != key)
                .chain([(key, value)])
                .collect();
            assert_eq!(
                config(&env(&pairs)).err(),
                Some((what, why.into())),
                "{key}"
            );
        }
    }

    #[test]
    fn the_chat_config_names_what_is_missing() {
        let ChatConfig { gateway, model, .. } = config(&env(&chat_env(&[]))).expect("configured");
        assert_eq!(gateway, "http://gateway:8080");
        assert_eq!(model.proxy, "http://proxy:8080");
        assert_eq!(model.client_id, "jc-assistant");
        assert_eq!(model.model, "a-model");
        for key in [
            "JC_ASSISTANT_TOKEN_URL",
            "JC_ASSISTANT_LLM",
            "JC_ASSISTANT_PROXY_URL",
        ] {
            let pairs: Vec<_> = chat_env(&[])
                .into_iter()
                .filter(|(k, _)| *k != key)
                .collect();
            assert_eq!(
                config(&env(&pairs)).err(),
                Some(("the chat is not configured", format!("{key} is required")))
            );
        }
    }

    #[test]
    fn a_host_origin_joins_only_the_deployment_it_names_and_only_once() {
        let deployment = |public_id: &str, origins: &[&str]| {
            let spec: jc_core::kinds::AssistantDeploymentSpec =
                serde_json::from_value(serde_json::json!({
                    "publicId": public_id,
                    "channel": "public",
                    "allowedOrigins": origins,
                }))
                .expect("a deployment spec");
            worker::Deployment {
                project: "p".into(),
                name: public_id.into(),
                spec,
            }
        };
        let mut snapshot = worker::Snapshot {
            deployments: vec![
                deployment("data", &["https://opendata.city.test"]),
                deployment("other", &[]),
            ],
            ..worker::Snapshot::default()
        };
        let extra = BTreeMap::from([(
            "data".to_owned(),
            vec![
                "https://jc.city.test".to_owned(),
                "https://opendata.city.test".to_owned(),
            ],
        )]);
        add_origins(&mut snapshot, &extra);
        add_origins(&mut snapshot, &extra);
        assert_eq!(
            snapshot.deployments[0].spec.allowed_origins,
            ["https://opendata.city.test", "https://jc.city.test"]
        );
        assert!(snapshot.deployments[1].spec.allowed_origins.is_empty());
    }
}
