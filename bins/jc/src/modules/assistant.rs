//! The knowledge assistant inside `jc` (G7): the chat, the widget pages and the crawl worker of
//! `jc-assistant`, with its model proxy (`jc-agent-proxy`) on a loopback listener beside it.
//!
//! Publicly the module answers the two paths dev's edge routes to jc-assistant, `/d/...` and
//! `/api/v1/d/...` (slug mode at the root, host mode on `assistant.{base_host}`); everything else
//! (the Portal's administration paths, the gateway's document search) answers only on its own
//! loopback listener, which is the sibling address the Portal's `JC_PORTAL_KNOWLEDGE_URL` names.
//!
//! Settings: the assistant's own keys from the environment first ([`crate::config::assistant_env`]),
//! then [`vars`] from the siblings. The model key comes only from its file (`JCBIN_MODEL_KEY_FILE`
//! or `[assistant] model_key_file`, read by `jc_secret`), and goes to the proxy's config field,
//! never into a lookup, an environment or a log line.
//!
//! The module is off, with one warning naming why, when the embedding model, ONNX Runtime, the
//! model proxy (no key file and no `JC_ASSISTANT_PROXY_URL`), `[assistant.database]`, a sibling or
//! a client secret is missing, or when building it fails. Off, its mount answers 404 and it leaves
//! the sibling table, so no Portal setting and no dataroom launcher points at it.

use crate::config::Config;
use crate::module::Module;
use crate::setup::{self, Secret, Setup};
use crate::siblings::Siblings;
use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// The chat's model when the environment names none (the deployment's default).
pub const DEFAULT_LLM: &str = "google/gemini-3.8-flash";
/// The model provider the proxy asks when the environment names none (the deployment's default).
const DEFAULT_MODEL_BASE: &str = "https://openrouter.ai/api/v1";
const DEFAULT_MODEL_PROVIDER: &str = "openai-compatible";

/// The assistant settings `jc` fills from its siblings; [`lookup`] lets the environment win.
///
/// | settings | from |
/// |---|---|
/// | `JC_ASSISTANT_GATEWAY_URL`, `_ENDPOINT_URL` | gateway (internal, public) |
/// | `JC_ASSISTANT_TOKEN_URL`, `_CLIENT_ID`, `_CLIENT_SECRET_FILE` | kaisatsu, client `jc-assistant` |
/// | `JC_ASSISTANT_PORTAL_URL`, `_ORG_DOMAIN` | the Portal's mount |
/// | `JC_ASSISTANT_PUBLIC_ORIGIN` | the assistant's own mount |
/// | `JC_ASSISTANT_REPO_DIR`, `_PROJECTS_DIR`, `_ASSEMBLY_DIR` | the gateway's shako checkout |
/// | `JC_ASSISTANT_PROXY_URL` | the proxy's loopback listener, when `jc` runs it |
/// | `JC_ASSISTANT_LLM` | [`DEFAULT_LLM`] |
#[must_use]
pub fn vars(
    siblings: &Siblings,
    setup: &Setup,
    proxy: Option<SocketAddr>,
    embedder: Option<&str>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut put = |key: &str, value: Option<String>| {
        if let Some(value) = value {
            out.insert(key.to_owned(), value);
        }
    };
    let public = |m: Module| siblings.get(m).map(|s| s.public.clone());
    // The chat links into these as origins (`/api/endpoint/{slug}`, the guide's pages): a module
    // under a path prefix leaves its key unset.
    let origin = |m: Module| public(m).filter(|url| crate::siblings::origin(url) == *url);
    let internal = |m: Module| siblings.get(m).map(|s| s.internal.clone());
    put("JC_ASSISTANT_GATEWAY_URL", internal(Module::Gateway));
    put("JC_ASSISTANT_ENDPOINT_URL", origin(Module::Gateway));
    // The issuer's public token endpoint, as the gateway's own token: kaisatsu names the issuer
    // by the host a request reached it on, and the proxy introspects at the same address.
    put(
        "JC_ASSISTANT_TOKEN_URL",
        public(Module::Kaisatsu).map(|id| token_url(&id, &setup.realm)),
    );
    put(
        "JC_ASSISTANT_CLIENT_ID",
        Some(setup::ASSISTANT_CLIENT.to_owned()),
    );
    put(
        "JC_ASSISTANT_CLIENT_SECRET_FILE",
        setup
            .path(Secret::AssistantClientSecret)
            .map(|p| p.display().to_string()),
    );
    put("JC_ASSISTANT_PORTAL_URL", origin(Module::Portal));
    put(
        "JC_ASSISTANT_ORG_DOMAIN",
        siblings
            .portal_base()
            .map(|base| crate::provision::domain(Some(&base))),
    );
    put("JC_ASSISTANT_PUBLIC_ORIGIN", origin(Module::Assistant));
    if siblings.get(Module::Gateway).is_some() {
        let work = super::gateway::work_dir(setup);
        put(
            "JC_ASSISTANT_REPO_DIR",
            Some(work.join("repo/current").display().to_string()),
        );
        put(
            "JC_ASSISTANT_PROJECTS_DIR",
            Some(work.join("projects").display().to_string()),
        );
    }
    put(
        "JC_ASSISTANT_ASSEMBLY_DIR",
        Some(setup.dir.join("assistant/assembly").display().to_string()),
    );
    put(
        "JC_ASSISTANT_PROXY_URL",
        proxy.map(|at| format!("http://{at}")),
    );
    put("JC_ASSISTANT_LLM", Some(DEFAULT_LLM.to_owned()));
    // The shared embedder asks this realm for scope `embedder` as the assistant's own client.
    if let Some(url) = embedder {
        put("JC_ASSISTANT_EMBEDDER_URL", Some(url.to_owned()));
        put(
            "JC_ASSISTANT_EMBEDDER_TOKEN_URL",
            public(Module::Kaisatsu).map(|id| token_url(&id, &setup.realm)),
        );
        put(
            "JC_ASSISTANT_EMBEDDER_CLIENT_ID",
            Some(setup::ASSISTANT_CLIENT.to_owned()),
        );
        put(
            "JC_ASSISTANT_EMBEDDER_CLIENT_SECRET_FILE",
            setup
                .path(Secret::AssistantClientSecret)
                .map(|p| p.display().to_string()),
        );
    }
    out
}

/// The realm's token endpoint under kaisatsu's base `id`.
fn token_url(id: &str, realm: &setup::Realm) -> String {
    format!("{id}/realms/{realm}/protocol/openid-connect/token")
}

/// One assistant key: the environment's value (`env`) first, else the sibling's (`vars`).
fn lookup<'a>(
    env: &'a impl Fn(&str) -> Option<String>,
    vars: &'a BTreeMap<String, String>,
) -> impl Fn(&str) -> Option<String> + 'a {
    move |key| {
        env(key)
            .filter(|v| !v.trim().is_empty())
            .or_else(|| vars.get(key).cloned())
    }
}

/// The proxy's settings as its `Config::from_lookup` reads them: a fixed table, so neither a
/// secret nor an operator's `JC_MODEL_KEY` reaches it this way (the key and the client secret
/// are set on the config afterwards). The model's base and provider follow the environment.
fn proxy_lookup(
    siblings: &Siblings,
    realm: &setup::Realm,
    env: &impl Fn(&str) -> Option<String>,
) -> BTreeMap<&'static str, String> {
    let mut out = BTreeMap::new();
    out.insert("JC_PROXY_BIND", format!("{}:0", Ipv4Addr::LOCALHOST));
    if let Some(id) = siblings.get(Module::Kaisatsu) {
        out.insert("JC_OIDC_ISSUER", format!("{}/realms/{realm}", id.public));
    }
    out.insert("JC_OIDC_CLIENT_ID", setup::PROXY_CLIENT.to_owned());
    out.insert("JC_ASSISTANT_CLIENT_ID", setup::ASSISTANT_CLIENT.to_owned());
    out.insert("JC_PORTAL_CLIENT_ID", setup::PORTAL_CLIENT.to_owned());
    for (key, module) in [
        ("JC_PORTAL_BASE", Module::Portal),
        ("JC_GATEWAY_BASE", Module::Gateway),
        ("JC_FORGE_BASE", Module::Shako),
    ] {
        if let Some(s) = siblings.get(module) {
            out.insert(key, s.internal.clone());
        }
    }
    // The listener is loopback only: no mesh in front of it to name a caller.
    out.insert("JC_REQUIRE_MESH_IDENTITY", "false".to_owned());
    out.insert(
        "JC_MODEL_BASE",
        env("JC_MODEL_BASE").unwrap_or_else(|| DEFAULT_MODEL_BASE.to_owned()),
    );
    out.insert(
        "JC_MODEL_PROVIDER",
        env("JC_MODEL_PROVIDER").unwrap_or_else(|| DEFAULT_MODEL_PROVIDER.to_owned()),
    );
    out
}

/// What the module needs before anything is built, or why it is off: the siblings, the client
/// secrets, the embedding model's directory, ONNX Runtime's library and the database section.
fn missing(
    cfg: &Config,
    siblings: &Siblings,
    setup: &Setup,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Option<String> {
    for module in [
        Module::Kaisatsu,
        Module::Gateway,
        Module::Portal,
        Module::Assistant,
    ] {
        if siblings.get(module).is_none() {
            return Some(format!("{module} is not mounted in this jc"));
        }
    }
    for secret in [Secret::AssistantClientSecret, Secret::ProxyClientSecret] {
        if setup.value(secret).is_none() {
            return Some(format!("setup holds no {}", secret.file_name()));
        }
    }
    // The shared embedder service needs neither the model nor ONNX Runtime in this process.
    let local = lookup("JC_ASSISTANT_EMBEDDER_URL").is_none();
    match lookup("JC_ASSISTANT_MODEL_DIR").filter(|_| local) {
        None if !local => {}
        None => return Some("JC_ASSISTANT_MODEL_DIR (the embedding model) is unset".into()),
        Some(dir) if !Path::new(&dir).is_dir() => {
            return Some(format!(
                "the embedding model directory {dir} does not exist"
            ));
        }
        Some(_) => {}
    }
    match lookup("ORT_DYLIB_PATH").filter(|_| local) {
        None if !local => {}
        None => return Some("ORT_DYLIB_PATH (ONNX Runtime) is unset".into()),
        Some(file) if !Path::new(&file).is_file() => {
            return Some(format!("ONNX Runtime {file} does not exist"));
        }
        Some(_) => {}
    }
    if cfg.assistant.database.is_none() {
        return Some("[assistant.database] is not configured".into());
    }
    None
}

/// The embedder: the shared service when `JC_ASSISTANT_EMBEDDER_URL` names one (a token of
/// audience `embedder` for the `jc-assistant` client, its secret from setup), else the model in
/// this process over ONNX Runtime. A failing service later is an error, never the local model.
async fn embedder(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<assistant::embed::Embedder, String> {
    if lookup("JC_ASSISTANT_EMBEDDER_URL").is_some() {
        return assistant::embed::Embedder::from_lookup(lookup, 1)
            .map_err(|e| format!("the embedder service is not usable: {e}"));
    }
    let (Some(model_dir), Some(ort)) = (lookup("JC_ASSISTANT_MODEL_DIR"), lookup("ORT_DYLIB_PATH"))
    else {
        return Err("JC_ASSISTANT_MODEL_DIR or ORT_DYLIB_PATH is unset".into());
    };
    let threads = assistant::host::embed_threads(lookup)?;
    tokio::task::spawn_blocking(move || {
        ort::init_from(&ort)
            .map_err(|e| format!("ONNX Runtime {ort} is not usable: {e}"))?
            .commit();
        assistant::embed::Embedder::load(Path::new(&model_dir), threads)
            .map_err(|e| format!("the embedding model is not usable: {e}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The model proxy `jc` runs, bound but not yet serving.
struct Proxy {
    listener: tokio::net::TcpListener,
    router: Router,
}

/// Builds the proxy over `key` on a free loopback port.
async fn proxy(
    siblings: &Siblings,
    setup: &Setup,
    env: &impl Fn(&str) -> Option<String>,
    key: &jc_secret::Secret,
) -> Result<Proxy, String> {
    let table = proxy_lookup(siblings, &setup.realm, env);
    let mut config = agent_proxy::config::Config::from_lookup(|k| table.get(k).cloned())
        .map_err(|e| format!("the model proxy is not configured: {e}"))?;
    config.oidc_client_secret = setup
        .value(Secret::ProxyClientSecret)
        .unwrap_or_default()
        .to_owned();
    config.model_key = key.expose().to_owned();
    if let Some(token) = setup.value(Secret::ShakoToken) {
        config.forge_token = token.to_owned();
    }
    let listener = tokio::net::TcpListener::bind(config.bind)
        .await
        .map_err(|e| format!("the model proxy cannot listen: {e}"))?;
    let config = Arc::new(config);
    let credentials = agent_proxy::inject::CredentialManager::new(Arc::clone(&config));
    let runs = agent_proxy::runs::RunResolver::new(config.portal_base.clone(), credentials.clone());
    let state = Arc::new(agent_proxy::ProxyState::new(
        config,
        runs,
        credentials,
        agent_proxy::limits::LimitManager::default(),
    ));
    Ok(Proxy {
        listener,
        router: agent_proxy::router(state),
    })
}

/// Answers 404 for every path outside `/d/...` and `/api/v1/d/...`, the two dev's edge publishes.
async fn public_only(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let published = ["/d", "/api/v1/d"].iter().any(|p| {
        path.strip_prefix(p)
            .is_some_and(|rest| rest.starts_with('/'))
    });
    if published {
        next.run(req).await
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// The public face of `router`: only the published paths.
pub fn public(router: Router) -> Router {
    router.layer(axum::middleware::from_fn(public_only))
}

/// Starts the assistant, or leaves it off with one warning: answers the router its public mount
/// serves, `None` when off (the mount then answers 404 and the sibling is removed).
/// `env` is the assistant's environment ([`crate::config::assistant_env`]).
pub async fn start(
    cfg: &Config,
    setup: &Setup,
    siblings: &mut Siblings,
    secrets: &crate::config::Secrets,
    env: &impl Fn(&str) -> Option<String>,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) -> Option<Router> {
    match try_start(cfg, setup, siblings, secrets, env, shutdown, tasks).await {
        Ok((router, internal, proxy)) => {
            tracing::info!(%internal, %proxy, "assistant: on");
            siblings.set_internal(Module::Assistant, internal);
            Some(router)
        }
        Err(why) => {
            tracing::warn!(%why, "assistant: off");
            siblings.remove(Module::Assistant);
            None
        }
    }
}

async fn try_start(
    cfg: &Config,
    setup: &Setup,
    siblings: &Siblings,
    secrets: &crate::config::Secrets,
    env: &impl Fn(&str) -> Option<String>,
    shutdown: &CancellationToken,
    tasks: &TaskTracker,
) -> Result<(Router, SocketAddr, String), String> {
    // The proxy: an operator's own (`JC_ASSISTANT_PROXY_URL`), else `jc`'s over the key file.
    let proxy = match (env("JC_ASSISTANT_PROXY_URL"), &secrets.model_key) {
        (Some(_), _) => None,
        (None, Some(key)) => Some(proxy(siblings, setup, env, &key.secret).await?),
        (None, None) => {
            return Err(
                "no model proxy: give the model key file (JCBIN_MODEL_KEY_FILE) or JC_ASSISTANT_PROXY_URL"
                    .into(),
            );
        }
    };
    let proxy_at = match &proxy {
        Some(p) => Some(p.listener.local_addr().map_err(|e| e.to_string())?),
        None => None,
    };
    let filled = vars(
        siblings,
        setup,
        proxy_at,
        cfg.assistant.embedder_url.as_deref(),
    );
    let lookup = lookup(env, &filled);
    if let Some(why) = missing(cfg, siblings, setup, &lookup) {
        return Err(why);
    }
    let (Some(organization), Some(checkout), Some(db)) = (
        lookup("JC_ASSISTANT_ORG_DOMAIN"),
        assistant::host::checkout(&lookup),
        cfg.assistant.database.as_ref(),
    ) else {
        return Err("the organization or its checkout is unknown".into());
    };
    let embedder = embedder(&lookup).await?;
    let pool = assistant::host::pool_options()
        .max_connections(db.max_connections())
        .connect_with(db.app_options()?)
        .await
        .map_err(|e| format!("the assistant's database is not reachable: {e}"))?;
    // As its own role, not a superuser, so the forced row-level security holds (T-3050).
    assistant::MIGRATOR
        .run(&pool)
        .await
        .map_err(|e| format!("the assistant's migrations did not run: {e}"))?;
    let crawler = assistant::crawl::Crawler::public().map_err(|e| e.to_string())?;
    let secret = setup
        .value(Secret::AssistantClientSecret)
        .unwrap_or_default()
        .to_owned();
    let credential = jc_core::ClientCredential::Secret(jc_core::Secret::new(secret));
    let config = assistant::host::chat_config(&lookup, credential)
        .map_err(|(what, why)| format!("{what}: {why}"))?;
    let (chat_state, reader) =
        assistant::service::chat_state(config, &pool, &embedder, &organization)
            .map_err(|e| e.to_string())?;
    let ready = Arc::new(AtomicBool::new(false));
    let router = assistant::host::probe(Arc::clone(&chat_state), Arc::clone(&ready));
    let internal = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|e| format!("the assistant cannot listen: {e}"))?;
    let internal_at = internal.local_addr().map_err(|e| e.to_string())?;

    // Built: from here on nothing fails, so the listeners serve and the worker turns.
    let mut minute = assistant::host::Minute::new(
        pool,
        embedder,
        crawler,
        reader,
        organization,
        "jc".into(),
        checkout,
        chat_state,
        ready,
    );
    minute.extra_origins = catalogue_origin(cfg, siblings);
    if let Some(p) = proxy {
        serve(tasks, shutdown, p.listener, p.router, "the model proxy");
    }
    serve(tasks, shutdown, internal, router.clone(), "the assistant");
    let stop = shutdown.clone();
    tasks.spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = stop.cancelled() => return,
                _ = tick.tick() => {}
            }
            minute.work().await;
        }
    });
    let proxy = lookup("JC_ASSISTANT_PROXY_URL").unwrap_or_default();
    Ok((public(router), internal_at, proxy))
}

/// The dataroom's public origin for the deployment `[assistant] catalogue_deployment` names, so
/// that deployment's widget may be framed by this `jc`'s catalogue: https only.
fn catalogue_origin(cfg: &Config, siblings: &Siblings) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    if let Some(origin) = siblings
        .get(Module::Dataroom)
        .map(|d| crate::siblings::origin(&d.public))
        .filter(|o| o.starts_with("https://"))
    {
        out.insert(cfg.assistant.catalogue_deployment.clone(), vec![origin]);
    }
    out
}

fn serve(
    tasks: &TaskTracker,
    shutdown: &CancellationToken,
    listener: tokio::net::TcpListener,
    router: Router,
    what: &'static str,
) {
    let stop = shutdown.clone();
    tasks.spawn(async move {
        if let Err(e) = axum::serve(listener, router)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await
        {
            tracing::error!(error = %e, "{what} stopped");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Cli, FileConfig};
    use tower::ServiceExt;

    fn config() -> Config {
        Config::merge(
            Cli {
                role: Some("all".into()),
                base_host: Some("city.example".into()),
                ..Cli::default()
            },
            FileConfig::default(),
        )
        .unwrap()
    }

    fn fixture() -> (Config, Siblings, Setup, tempfile::TempDir) {
        let cfg = config();
        let plans = crate::app::plan_mounts(&cfg).unwrap();
        let siblings = Siblings::new(&plans, "0.0.0.0:8443".parse().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let setup = Setup::prepare(dir.path(), &cfg.roles, &BTreeMap::new()).unwrap();
        (cfg, siblings, setup, dir)
    }

    #[tokio::test]
    async fn only_the_published_paths_answer_publicly() {
        let inner = Router::new().fallback(|| async { "inner" });
        let app = public(inner);
        for (path, status) in [
            ("/d/data/widget", StatusCode::OK),
            ("/d/widget.js", StatusCode::OK),
            ("/api/v1/d/data/chat", StatusCode::OK),
            ("/internal/v1/search", StatusCode::NOT_FOUND),
            (
                "/internal/v1/projects/p/knowledge/sources",
                StatusCode::NOT_FOUND,
            ),
            ("/healthz", StatusCode::NOT_FOUND),
            ("/dx/widget", StatusCode::NOT_FOUND),
            ("/d", StatusCode::NOT_FOUND),
            ("/api/v1/dd/x", StatusCode::NOT_FOUND),
        ] {
            let req = Request::builder()
                .uri(path)
                .body(axum::body::Body::empty())
                .unwrap();
            let got = app.clone().oneshot(req).await.unwrap().status();
            assert_eq!(got, status, "{path}");
        }
    }

    #[test]
    fn the_siblings_fill_the_settings_and_the_environment_wins() {
        let (_cfg, siblings, setup, _d) = fixture();
        let filled = vars(
            &siblings,
            &setup,
            Some("127.0.0.1:4100".parse().unwrap()),
            Some("http://127.0.0.1:4200"),
        );
        assert_eq!(filled["JC_ASSISTANT_EMBEDDER_URL"], "http://127.0.0.1:4200");
        assert_eq!(
            filled["JC_ASSISTANT_EMBEDDER_TOKEN_URL"],
            filled["JC_ASSISTANT_TOKEN_URL"]
        );
        assert!(!vars(&siblings, &setup, None, None).contains_key("JC_ASSISTANT_EMBEDDER_URL"));
        assert_eq!(filled["JC_ASSISTANT_PROXY_URL"], "http://127.0.0.1:4100");
        assert_eq!(filled["JC_ASSISTANT_GATEWAY_URL"], "http://127.0.0.1:8443");
        assert_eq!(
            filled["JC_ASSISTANT_TOKEN_URL"],
            "https://city.example/id/realms/jc/protocol/openid-connect/token"
        );
        assert_eq!(filled["JC_ASSISTANT_PUBLIC_ORIGIN"], "https://city.example");
        assert_eq!(filled["JC_ASSISTANT_ORG_DOMAIN"], "city.example");
        assert!(filled["JC_ASSISTANT_REPO_DIR"].ends_with("gateway/repo/current"));
        assert_eq!(filled["JC_ASSISTANT_LLM"], DEFAULT_LLM);
        // A file path for the client secret, never its value.
        let secret = setup.value(Secret::AssistantClientSecret).unwrap();
        assert!(filled.values().all(|v| !v.contains(secret)));
        let env = |k: &str| (k == "JC_ASSISTANT_LLM").then(|| "other/model".to_owned());
        let read = lookup(&env, &filled);
        assert_eq!(read("JC_ASSISTANT_LLM").as_deref(), Some("other/model"));
        assert_eq!(
            read("JC_ASSISTANT_ORG_DOMAIN").as_deref(),
            Some("city.example")
        );
    }

    #[test]
    fn the_proxy_table_names_no_secret_and_follows_the_models_base() {
        let (_cfg, siblings, _setup, _d) = fixture();
        let env = |k: &str| match k {
            "JC_MODEL_BASE" => Some("https://models.example/v1".to_owned()),
            "JC_MODEL_KEY" => Some("sk-or-v1-from-env".to_owned()),
            _ => None,
        };
        let table = proxy_lookup(&siblings, &setup::Realm::default(), &env);
        assert_eq!(table["JC_MODEL_BASE"], "https://models.example/v1");
        assert_eq!(table["JC_PROXY_BIND"], "127.0.0.1:0");
        for key in table.keys() {
            assert!(
                !(key.contains("KEY") || key.contains("SECRET") || key.contains("TOKEN")),
                "{key}"
            );
        }
        assert!(table.values().all(|v| !v.contains("sk-or-v1")));
    }

    #[tokio::test]
    async fn without_a_model_key_the_module_is_off_and_leaves_the_siblings() {
        let (cfg, mut siblings, setup, _d) = fixture();
        let secrets = crate::config::Secrets::load_from(&cfg, &BTreeMap::new()).unwrap();
        let (shutdown, tasks) = (CancellationToken::new(), TaskTracker::new());
        // No key file, no proxy address, no model, no database: off, and nothing points at it.
        let started = start(
            &cfg,
            &setup,
            &mut siblings,
            &secrets,
            &|_| None,
            &shutdown,
            &tasks,
        )
        .await;
        assert!(started.is_none());
        assert!(siblings.get(Module::Assistant).is_none());
        let portal = crate::siblings::portal_vars(&siblings, &setup, |_| None);
        assert!(!portal.contains_key("JC_PORTAL_KNOWLEDGE_URL"));
        assert_eq!(
            crate::modules::dataroom::assistant_widget(&siblings, "data"),
            None
        );
        assert!(tasks.is_empty(), "nothing was spawned");
    }

    #[tokio::test]
    async fn each_missing_piece_is_named() {
        let (mut cfg, siblings, setup, dir) = fixture();
        let none = |_: &str| None;
        let why = missing(&cfg, &siblings, &setup, &none).unwrap();
        assert!(why.contains("JC_ASSISTANT_MODEL_DIR"), "{why}");
        let model = dir.path().join("model");
        std::fs::create_dir(&model).unwrap();
        let ort = dir.path().join("libonnxruntime.so");
        let (m, o) = (model.display().to_string(), ort.display().to_string());
        let found = move |k: &str| match k {
            "JC_ASSISTANT_MODEL_DIR" => Some(m.clone()),
            "ORT_DYLIB_PATH" => Some(o.clone()),
            _ => None,
        };
        let why = missing(&cfg, &siblings, &setup, &found).unwrap();
        assert!(why.contains("ONNX Runtime"), "{why}");
        std::fs::write(&ort, b"").unwrap();
        let why = missing(&cfg, &siblings, &setup, &found).unwrap();
        assert!(why.contains("[assistant.database]"), "{why}");
        cfg.assistant.database = Some(crate::pg::DatabaseSection {
            url: "postgres://assistant@127.0.0.1:1/assistant".into(),
            password_file: dir.path().join("pw"),
            max_connections: None,
            migrate_url: None,
            migrate_password_file: None,
        });
        assert_eq!(missing(&cfg, &siblings, &setup, &found), None);
        let mut alone = siblings.clone();
        alone.remove(Module::Gateway);
        let why = missing(&cfg, &alone, &setup, &found).unwrap();
        assert!(why.contains("gateway"), "{why}");
    }

    /// The catalogue's origin joins the named deployment's allowedOrigins over https only: the
    /// widget's frame-ancestors never names a plain-http page.
    #[test]
    fn the_catalogue_origin_joins_its_deployment_over_https_only() {
        let (mut cfg, siblings, _setup, _dir) = fixture();
        let origins = catalogue_origin(&cfg, &siblings);
        assert_eq!(
            origins,
            BTreeMap::from([("data".to_owned(), vec!["https://city.example".to_owned()])])
        );
        cfg.assistant.catalogue_deployment = "visitors".into();
        assert!(catalogue_origin(&cfg, &siblings).contains_key("visitors"));
        let http = Config::merge(
            Cli {
                role: Some("all".into()),
                base_host: Some("city.example".into()),
                scheme: Some("http".into()),
                ..Cli::default()
            },
            FileConfig::default(),
        )
        .unwrap();
        let plans = crate::app::plan_mounts(&http).unwrap();
        let plain = Siblings::new(&plans, "0.0.0.0:8080".parse().unwrap());
        assert!(catalogue_origin(&http, &plain).is_empty());
    }

    /// With the shared embedder named, neither the model nor ONNX Runtime is asked for, and the
    /// embedder is the service, with the token URL and the client's secret file setup gave.
    #[tokio::test]
    async fn the_embedder_service_needs_no_model_and_is_the_one_used() {
        let (mut cfg, siblings, setup, dir) = fixture();
        cfg.assistant.database = Some(crate::pg::DatabaseSection {
            url: "postgres://assistant@127.0.0.1:1/assistant".into(),
            password_file: dir.path().join("pw"),
            max_connections: None,
            migrate_url: None,
            migrate_password_file: None,
        });
        let filled = vars(&siblings, &setup, None, Some("http://127.0.0.1:4200"));
        let none = |_: &str| None;
        let remote = lookup(&none, &filled);
        assert_eq!(missing(&cfg, &siblings, &setup, &remote), None);
        assert!(embedder(&remote).await.unwrap().is_remote());
        // Without the service the in-process model stays the one asked for.
        let local = vars(&siblings, &setup, None, None);
        let why = missing(&cfg, &siblings, &setup, &lookup(&none, &local)).unwrap();
        assert!(why.contains("JC_ASSISTANT_MODEL_DIR"), "{why}");
        // A service named without its secret file does not start.
        let mut broken = filled.clone();
        broken.remove("JC_ASSISTANT_EMBEDDER_CLIENT_SECRET_FILE");
        let Err(why) = embedder(&lookup(&none, &broken)).await else {
            panic!("a service without its secret file started");
        };
        assert!(why.contains("CLIENT_SECRET_FILE"), "{why}");
    }

    /// A writer the test's tracing subscriber appends to, read after the module started.
    #[derive(Clone, Default)]
    struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The assistant's test inputs (the same as the assistant crate's tests): the embedding model,
    /// ONNX Runtime and a PostgreSQL with pgvector whose user may create roles and databases.
    #[allow(clippy::disallowed_methods)] // The test's own inputs, not jc's configuration.
    fn test_inputs() -> Option<(String, String, String)> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Some((
            var("JC_ASSISTANT_TEST_MODEL_DIR")?,
            var("ORT_DYLIB_PATH")?,
            var("JC_ASSISTANT_TEST_DATABASE_URL")?,
        ))
    }

    #[tokio::test]
    async fn with_its_model_key_and_database_the_module_is_on() {
        let Some((model_dir, ort, admin_url)) = test_inputs() else {
            eprintln!(
                "SKIP: set JC_ASSISTANT_TEST_MODEL_DIR, ORT_DYLIB_PATH and JC_ASSISTANT_TEST_DATABASE_URL"
            );
            return;
        };
        let (mut cfg, mut siblings, setup, dir) = fixture();
        // A database of the test's own, owned by a login role as the cluster's `assistant` is,
        // with pgvector made by the administrator (CloudNativePG does it on the cluster).
        let id = std::process::id();
        let (role, db) = (
            format!("jc_assistant_on_{id}"),
            format!("jc_assistant_on_{id}"),
        );
        let password = format!("pw-{id}");
        let admin = sqlx::PgPool::connect(&admin_url).await.unwrap();
        for sql in [
            format!("DROP DATABASE IF EXISTS {db}"),
            format!("DROP ROLE IF EXISTS {role}"),
            format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'"),
            format!("CREATE DATABASE {db} OWNER {role}"),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(&admin)
                .await
                .unwrap();
        }
        let mut in_db: sqlx::postgres::PgConnectOptions = admin_url.parse().unwrap();
        in_db = in_db.database(&db);
        let owner_admin = sqlx::PgPool::connect_with(in_db).await.unwrap();
        sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
            .execute(&owner_admin)
            .await
            .unwrap();
        owner_admin.close().await;
        let mut url = url::Url::parse(&admin_url).unwrap();
        url.set_username(&role).unwrap();
        url.set_password(None).unwrap();
        url.set_path(&format!("/{db}"));
        let password_file = dir.path().join("assistant-password");
        std::fs::write(&password_file, &password).unwrap();
        cfg.assistant.database = Some(crate::pg::DatabaseSection {
            url: url.to_string(),
            password_file,
            max_connections: Some(2),
            migrate_url: None,
            migrate_password_file: None,
        });
        // A model key of the test's own, from its file; it must show up in no log line.
        let marker = format!("{}-{id}", ["jc", "test", "model", "value"].join("-"));
        let key_file = dir.path().join("model-key");
        std::fs::write(&key_file, &marker).unwrap();
        cfg.assistant.model_key_file = Some(key_file);
        let secrets = crate::config::Secrets::load_from(&cfg, &BTreeMap::new()).unwrap();
        let env = move |k: &str| match k {
            "JC_ASSISTANT_MODEL_DIR" => Some(model_dir.clone()),
            "ORT_DYLIB_PATH" => Some(ort.clone()),
            _ => None,
        };

        let logs = Captured::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let (shutdown, tasks) = (CancellationToken::new(), TaskTracker::new());
        let public = start(
            &cfg,
            &setup,
            &mut siblings,
            &secrets,
            &env,
            &shutdown,
            &tasks,
        )
        .await
        .unwrap_or_else(|| {
            let logged = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
            panic!("off: {logged}")
        });

        // The Portal and the dataroom see it.
        let internal = siblings.get(Module::Assistant).unwrap().internal.clone();
        assert!(internal.starts_with("http://127.0.0.1:"), "{internal}");
        let portal = crate::siblings::portal_vars(&siblings, &setup, |_| None);
        assert_eq!(portal["JC_PORTAL_KNOWLEDGE_URL"], internal);
        assert_eq!(
            crate::modules::dataroom::assistant_widget(&siblings, "data").as_deref(),
            Some("https://city.example/d/data/widget")
        );

        // Publicly only the widget's and the chat's paths.
        let call = |path: &str| {
            let req = Request::builder()
                .uri(path)
                .body(axum::body::Body::empty())
                .unwrap();
            public.clone().oneshot(req)
        };
        assert_eq!(call("/d/widget.js").await.unwrap().status(), StatusCode::OK);
        for closed in [
            "/internal/v1/search",
            "/healthz",
            "/internal/v1/projects/p/knowledge/sources",
        ] {
            assert_eq!(
                call(closed).await.unwrap().status(),
                StatusCode::NOT_FOUND,
                "{closed}"
            );
        }

        // On loopback: the probe, and the Portal's paths refuse a caller without the Portal's token.
        let http = reqwest::Client::new();
        let health = http
            .get(format!("{internal}/healthz"))
            .send()
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        let admin_path = http
            .get(format!(
                "{internal}/internal/v1/projects/p/knowledge/sources"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(admin_path.status(), StatusCode::UNAUTHORIZED);

        // The proxy runs on loopback and refuses a completion without the assistant's token
        // before any provider is asked.
        let logged = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        assert!(logged.contains("assistant: on"), "{logged}");
        let proxy = logged
            .split("proxy=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .expect("the proxy's address is logged");
        assert!(proxy.starts_with("http://127.0.0.1:"), "{proxy}");
        let health = http.get(format!("{proxy}/healthz")).send().await.unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        let refused = http
            .post(format!("{proxy}/v1/llm/chat/completions"))
            .header("x-jc-assistant-deployment", "bbsk/data")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);

        shutdown.cancel();
        tasks.close();
        tokio::time::timeout(Duration::from_secs(30), tasks.wait())
            .await
            .expect("every task stops");
        let logged = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        assert!(
            !logged.contains(&marker),
            "the model key reached a log line"
        );
        let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {db} WITH (FORCE)"
        )))
        .execute(&admin)
        .await;
        let _ = sqlx::query(sqlx::AssertSqlSafe(format!("DROP ROLE IF EXISTS {role}")))
            .execute(&admin)
            .await;
    }
}
