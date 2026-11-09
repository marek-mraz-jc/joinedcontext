//! `jc-assistant`: the knowledge assistant's crawl worker and chat route (Architecture/22 §1,
//! T-3052, T-3055).
//!
//! Environment: `JC_ASSISTANT_DATABASE_URL` (the `assistant` role's own database),
//! `JC_ASSISTANT_REPO_DIR` (the organization's checkout), `JC_ASSISTANT_PROJECTS_DIR` and
//! `JC_ASSISTANT_ASSEMBLY_DIR` (layout 2), `JC_ASSISTANT_ORG_DOMAIN`, `JC_ASSISTANT_MODEL_DIR`
//! (the embedding model's files, `scripts/ci/e5-small.sh`), `JC_ASSISTANT_EMBED_THREADS`
//! (default 2) or `JC_ASSISTANT_EMBEDDER_URL` and its client (the shared embedder service,
//! [`embedder_from_env`]), `JC_ASSISTANT_BIND` (the chat route and the health probe, default
//! `0.0.0.0:8080`), `HOSTNAME` (the worker's name in the queue), and for the chat:
//! `JC_ASSISTANT_PROXY_URL` (`jc-agent-proxy`), `JC_ASSISTANT_GATEWAY_URL` (the connectors' MCP
//! surfaces), `JC_ASSISTANT_TOKEN_URL`, `JC_ASSISTANT_CLIENT_ID` (default `jc-assistant`) and
//! `JC_ASSISTANT_CLIENT_ASSERTION_FILE` (the pod's projected token, PF-47) or
//! `JC_ASSISTANT_CLIENT_SECRET_FILE` (the service's own Keycloak client), `JC_ASSISTANT_LLM` (the
//! model the completions name) and `JC_ASSISTANT_FUNCTIONS_URL` (`jc-functions`, for the scripts
//! of a deployment with `sandbox: true`; none without it) and `JC_ASSISTANT_PORTAL_CLIENT` (the
//! Portal's client, the one caller of the administration paths; default `portal-api`) and
//! `JC_ASSISTANT_PUBLIC_ORIGIN` (`https://assistant.{domain}`, whose widget may ask the chat) and
//! `JC_ASSISTANT_ENDPOINT_URL` (the public origin `/api/endpoint/{slug}` is served under, which a
//! `catalogue` source's pages cite when the Endpoint publishes no CKAN dataset, AG-116) and
//! `JC_ASSISTANT_PORTAL_URL` (`https://portal.{domain}`, which a `guide` source's sections link
//! into, AG-118) and `JC_ASSISTANT_VIZ_URL` (`https://viz.{domain}`, the renderer a data answer's
//! maps and charts are drawn on, T-4657; none without it).

use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use assistant::chat::ChatState;
use assistant::crawl::Crawler;
use assistant::embed::Embedder;
use assistant::host::{self, embed_threads, pool_options, probe, Minute};
use assistant::service;
use sqlx::PgPool;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    match run().await {
        Ok(()) => {
            tracing::info!("stopping");
            ExitCode::SUCCESS
        }
        Err(Startup { what, why }) => {
            tracing::error!(%why, "{what}");
            ExitCode::FAILURE
        }
    }
}

/// Why the service did not start: what failed, and the error underneath.
#[derive(Debug)]
struct Startup {
    what: &'static str,
    why: String,
}

/// Turns an error into the [`Startup`] failure `what` names.
fn fail<E: std::fmt::Display>(what: &'static str) -> impl FnOnce(E) -> Startup {
    move |err| Startup {
        what,
        why: err.to_string(),
    }
}

/// Starts the chat and the probe, then works the queue once a minute until SIGTERM or Ctrl-C.
async fn run() -> Result<(), Startup> {
    let env = |key: &str| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
    };

    let (Some(url), Some(checkout), Some(organization)) = (
        env("JC_ASSISTANT_DATABASE_URL"),
        host::checkout(env),
        env("JC_ASSISTANT_ORG_DOMAIN"),
    ) else {
        return Err(Startup {
            what: "JC_ASSISTANT_DATABASE_URL, JC_ASSISTANT_REPO_DIR and JC_ASSISTANT_ORG_DOMAIN are required",
            why: "one of them is unset or empty".into(),
        });
    };
    let embedder = embedder_from_env(&env).map_err(fail("the embedding model is not usable"))?;
    let pool = pool_options()
        .connect(&url)
        .await
        .map_err(fail("the assistant's database is not reachable"))?;
    // As the `assistant` role, not a superuser, so the forced row-level security holds (T-3050).
    assistant::MIGRATOR
        .run(&pool)
        .await
        .map_err(fail("the assistant's migrations did not run"))?;
    let crawler = Crawler::public().map_err(fail("the crawler could not be built"))?;
    let (chat_state, reader) = chat_state(&env, &pool, &embedder, &organization)?;

    // Ready once the first look at the manifests went through; alive as long as the loop turns.
    let ready = Arc::new(AtomicBool::new(false));
    let bind = env("JC_ASSISTANT_BIND").unwrap_or_else(|| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .map_err(|err| Startup {
            what: "the chat and the health probe cannot listen",
            why: format!("{bind}: {err}"),
        })?;
    let probe = probe(Arc::clone(&chat_state), Arc::clone(&ready));
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, probe).await {
            tracing::error!(%err, "the chat and the health probe stopped");
        }
    });

    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(fail("SIGTERM cannot be watched"))?;
    let mut minute = Minute::new(
        pool,
        embedder,
        crawler,
        reader,
        organization,
        env("HOSTNAME").unwrap_or_else(|| "jc-assistant".into()),
        checkout,
        chat_state,
        ready,
    );
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            _ = terminate.recv() => return Ok(()),
            _ = tick.tick() => {}
        }
        minute.work().await;
    }
}

/// The chat's shared state and the catalogue reader the crawl worker shares its client with,
/// from the environment (the library's [`service::chat_state`]).
fn chat_state(
    env: &impl Fn(&str) -> Option<String>,
    pool: &PgPool,
    embedder: &Embedder,
    organization: &str,
) -> Result<(Arc<ChatState>, assistant::catalogue::Reader), Startup> {
    let credential = host::credential(env).map_err(|why| Startup {
        what: "the chat is not configured",
        why,
    })?;
    let config = host::chat_config(env, credential).map_err(|(what, why)| Startup { what, why })?;
    service::chat_state(config, pool, embedder, organization)
        .map_err(fail("the HTTP client could not be built"))
}

/// The embedder [`Embedder::from_lookup`] names, over the process environment.
fn embedder_from_env(env: &impl Fn(&str) -> Option<String>) -> Result<Embedder, String> {
    if env("JC_ASSISTANT_EMBEDDER_URL").is_some() {
        return Embedder::from_lookup(env, 1);
    }
    Embedder::from_lookup(env, embed_threads(env)?)
}
