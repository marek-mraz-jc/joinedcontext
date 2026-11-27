//! A test's [`Ask`] (T-7853): every part it borrows, each pointed at nothing the test did not
//! start, so a read path runs against a local gateway without a database or a model.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;

use super::agent::{Ask, Connected};
use super::mcp::{PersonToken, Surface, Tool};

/// What an [`Ask`] borrows.
pub(crate) struct Parts {
    pool: sqlx::PgPool,
    embedder: crate::embed::Embedder,
    model: super::model::Model,
    http: reqwest::Client,
    cache: super::routed::AnswerCache,
    trace: super::trace::Trace,
    suggestions: BTreeMap<String, Vec<String>>,
}

impl Parts {
    /// Parts whose database, embedder and model are a closed local port.
    pub(crate) fn new() -> Self {
        let http = reqwest::Client::new();
        let closed = "http://127.0.0.1:9";
        Self {
            // A store that is down: every query fails within a fifth of a second.
            pool: jc_pool::options::<sqlx::Postgres>()
                .acquire_timeout(Duration::from_millis(200))
                .connect_lazy("postgres://nobody@127.0.0.1:9/none")
                .expect("a lazy pool"),
            embedder: crate::embed::Embedder::remote(jc_embedder::client::RemoteConfig {
                url: closed.into(),
                token_url: format!("{closed}/token"),
                client_id: "jc-assistant".into(),
                client_secret: "s".into(),
            })
            .expect("a remote embedder"),
            model: super::model::Model::new(
                super::model::ModelConfig {
                    proxy: closed.into(),
                    token_url: format!("{closed}/token"),
                    client_id: "jc-assistant".into(),
                    credential: jc_core::ClientCredential::Secret(jc_core::Secret::new("s")),
                    model: "test/model".into(),
                },
                http.clone(),
            ),
            http,
            cache: super::routed::AnswerCache::default(),
            trace: super::trace::Trace::default(),
            suggestions: BTreeMap::new(),
        }
    }

    /// An anonymous public question of project `hronov` over `connectors`.
    pub(crate) fn ask<'a>(&'a self, connectors: &'a [Connected]) -> Ask<'a> {
        Ask {
            pool: &self.pool,
            embedder: &self.embedder,
            model: &self.model,
            http: &self.http,
            project: "hronov",
            deployment: "hronov/obcania",
            system_prompt: None,
            sources: &[],
            websites: &[],
            public_only: true,
            connectors,
            tokens_per_day: 1_000_000,
            tokens_per_conversation: 100_000,
            spent_before: 0,
            functions: None,
            router_model: None,
            answer_model: None,
            provider: None,
            reasoning: Default::default(),
            output_tokens: 1_000,
            cache: &self.cache,
            endpoint_base: None,
            viz: None,
            data_steps: 4,
            data_time: Duration::from_secs(10),
            now: time::OffsetDateTime::now_utc(),
            zone: None,
            trace: &self.trace,
            languages: &[],
            suggestions: &self.suggestions,
            title: None,
        }
    }
}

/// An Endpoint `slug` on the gateway at `gateway` offering `tools` (name, input schema), read as
/// the person of `token` or as anyone.
pub(crate) fn connector(
    gateway: &str,
    slug: &str,
    tools: &[(&str, Value)],
    token: Option<&str>,
) -> Connected {
    Connected {
        endpoint: format!("hronov/{slug}"),
        title: None,
        titles: BTreeMap::new(),
        surface: Surface {
            gateway: gateway.into(),
            slug: slug.into(),
            timeout: Duration::from_secs(2),
            token: token.map(|t| PersonToken(t.into())),
        },
        tools: tools
            .iter()
            .map(|(name, schema)| Tool {
                name: (*name).into(),
                description: String::new(),
                input_schema: schema.clone(),
            })
            .collect(),
        label: None,
        siblings: Vec::new(),
        dataset: None,
    }
}

/// A chat state over `snapshot` and the gateway at `gateway`, whose store, embedder and model are
/// closed local ports: a path that reads the manifests and the gateway runs, a query fails fast.
pub(crate) fn state(snapshot: crate::worker::Snapshot, gateway: &str) -> super::ChatState {
    let parts = Parts::new();
    super::ChatState {
        pool: parts.pool,
        embedder: parts.embedder,
        model: parts.model,
        http: parts.http,
        gateway: gateway.into(),
        functions: None,
        portal_client: "portal-api".into(),
        public_origin: Some("https://assistant.example".into()),
        snapshot: std::sync::RwLock::new(std::sync::Arc::new(snapshot)),
        limits: Default::default(),
        questions: std::sync::Arc::new(tokio::sync::Semaphore::new(super::MAX_QUESTIONS)),
        answers: Default::default(),
        endpoint_base: None,
        viz: None,
        organization: "hronov".into(),
        contexts: Default::default(),
        turns: Default::default(),
        map_sources: Default::default(),
    }
}

/// An enabled public deployment `name` of project `hronov` (publicId `hronov-{name}`) over the
/// connectors named in `endpoints`, each with `query_entities`, and no source.
pub(crate) fn public_deployment(name: &str, endpoints: &[&str]) -> crate::worker::Deployment {
    let connectors: Vec<Value> = endpoints
        .iter()
        .map(|e| serde_json::json!({"endpoint": e, "tools": ["query_entities"]}))
        .collect();
    crate::worker::Deployment {
        project: "hronov".into(),
        name: name.into(),
        spec: serde_json::from_value(serde_json::json!({
            "publicId": format!("hronov-{name}"),
            "channel": "public",
            "sources": [],
            "connectors": connectors,
            "allowedOrigins": ["https://www.hronov.example"],
            "rateLimit": {"requestsPerMinute": 60, "perClientPerMinute": 10, "perClientPerHour": 40},
            "budget": {"tokensPerDay": 1_000_000, "tokensPerConversation": 20_000, "costPerDayUsd": 2.5},
            "languages": ["sk"],
        }))
        .expect("a deployment spec"),
    }
}

/// The snapshot's entry for a public Endpoint `name` of project `hronov` with `slug`, and its page
/// over the entity types `types`.
pub(crate) fn publish(
    snapshot: &mut crate::worker::Snapshot,
    name: &str,
    slug: &str,
    types: &[&str],
) {
    snapshot.endpoints.insert(
        ("hronov".into(), name.into()),
        crate::worker::EndpointRef {
            titles: BTreeMap::new(),
            slug: slug.into(),
            audience: "public".into(),
            publisher: None,
            space: String::new(),
            title: None,
            dataset: None,
        },
    );
    snapshot.pages.insert(
        slug.into(),
        crate::worker::Page {
            project: "hronov".into(),
            page: crate::catalogue::EndpointPage {
                name: name.into(),
                title: None,
                description: None,
                space: String::new(),
                slug: slug.into(),
                audience: "public".into(),
                ckan_dataset: None,
                types: types
                    .iter()
                    .map(|class| crate::catalogue::EntityType {
                        class: (*class).into(),
                        description: None,
                        attributes: Vec::new(),
                    })
                    .collect(),
                datasets: Vec::new(),
            },
            themes: Vec::new(),
        },
    );
}

/// The log lines of this thread, kept by a subscriber over them (see `Log::capture` of the model's
/// tests): a line is only written when a subscriber wants it, so a test of a log line holds one.
#[derive(Clone, Default)]
pub(crate) struct Log(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("the log").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Log {
    /// The log, and the guard that keeps this thread's subscriber alive.
    pub(crate) fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
        let log = Self::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        (log, tracing::subscriber::set_default(subscriber))
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("the log")).into_owned()
    }
}
