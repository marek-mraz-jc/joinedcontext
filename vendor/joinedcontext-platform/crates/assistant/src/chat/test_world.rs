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
            pool: sqlx::PgPool::connect_lazy("postgres://nobody@127.0.0.1:9/none")
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
