//! Passage and question embeddings (T-3053, ADR-N-040 §3.1): `multilingual-e5-small` int8,
//! either loaded in this process from `JC_ASSISTANT_MODEL_DIR` or asked of the embedder service
//! several deployments share (`JC_ASSISTANT_EMBEDDER_URL`, `crates/embedder`). Both run the same
//! model code (`jc_embedder`), one text at a time, a question before every passage. The service
//! answers with the SHA-256 of the model it ran and the client refuses any other, so a failing or
//! different embedder is an error here, never a silent switch to another model whose vectors
//! would not match the index.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jc_embedder::client::{Remote, RemoteConfig};
use jc_embedder::Kind;
use sqlx::PgPool;

use crate::{vector_literal, Error};

pub use jc_embedder::{Took, MODEL_FILES};

/// How many passages one round of [`embed_missing`] takes from the store and writes back. The
/// model reads them one at a time: the int8 export quantizes its activations with a scale taken
/// from the whole input, so a passage embedded beside others would get a vector that depends on
/// its neighbours, and a question (always alone) would be measured against it.
pub const BATCH: usize = jc_embedder::client::BATCH;

/// The model, in this process or behind the embedder service. Cloning shares it.
#[derive(Clone)]
pub struct Embedder(Engine);

#[derive(Clone)]
enum Engine {
    Local(jc_embedder::Embedder),
    Remote(Arc<Remote>),
}

fn model(err: jc_embedder::Error) -> Error {
    Error::Model(err.to_string())
}

impl Embedder {
    /// Loads the model from `dir` after checking every file's hash.
    pub fn load(dir: &Path, threads: usize) -> Result<Self, Error> {
        jc_embedder::Embedder::load(dir, threads)
            .map(|e| Self(Engine::Local(e)))
            .map_err(model)
    }

    /// The embedder service at `cfg.url`, signed in to with this deployment's client.
    pub fn remote(cfg: RemoteConfig) -> Result<Self, Error> {
        Remote::new(cfg)
            .map(|r| Self(Engine::Remote(Arc::new(r))))
            .map_err(model)
    }

    /// The embeddings of passages to index, in order, one text per turn of the model.
    pub async fn passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        match &self.0 {
            Engine::Local(e) => e.passages(texts).await,
            Engine::Remote(r) => r.embed(Kind::Passage, texts).await,
        }
        .map_err(model)
    }

    /// The embedding of a question.
    pub async fn query(&self, text: &str) -> Result<Vec<f32>, Error> {
        Ok(self.query_timed(text).await?.0)
    }

    /// The embedding of a question, and how long it waited for the model and ran on it (for
    /// the service: the whole call, as run).
    pub async fn query_timed(&self, text: &str) -> Result<(Vec<f32>, Took), Error> {
        match &self.0 {
            Engine::Local(e) => e.query_timed(text).await.map_err(model),
            Engine::Remote(r) => {
                let began = Instant::now();
                let mut vectors = r
                    .embed(Kind::Query, &[text.to_owned()])
                    .await
                    .map_err(model)?;
                let vector = vectors
                    .pop()
                    .ok_or_else(|| Error::Model("the embedder answered no vector".into()))?;
                Ok((
                    vector,
                    Took {
                        waited: Duration::ZERO,
                        ran: began.elapsed(),
                    },
                ))
            }
        }
    }
}

/// The claim of up to `$1` chunks of the session's project still without a vector, oldest
/// first; index `chunks_embedding_pending` answers it (T-6547).
pub const CLAIM_UNEMBEDDED: &str =
    "SELECT id, text FROM chunks WHERE embedding IS NULL ORDER BY id LIMIT $1 FOR UPDATE SKIP LOCKED";

/// Embeds up to `limit` chunks of `project` that have none yet, [`BATCH`] per round, and
/// returns how many it embedded. A chunk gets one when it is stored or replaced (its page or
/// document changed), so this is the whole of "re-embed on change"; a batch the model fails on
/// stays without and is tried again on the next call.
pub async fn embed_missing(
    pool: &PgPool,
    embedder: &Embedder,
    project: &str,
    limit: i64,
) -> Result<usize, Error> {
    let mut done = 0;
    while (done as i64) < limit {
        let mut tx = crate::project_scope(pool, project).await?;
        let rows: Vec<(i64, String)> = sqlx::query_as(CLAIM_UNEMBEDDED)
            .bind((limit - done as i64).min(BATCH as i64))
            .fetch_all(&mut *tx)
            .await?;
        if rows.is_empty() {
            tx.rollback().await?;
            break;
        }
        let (ids, texts): (Vec<i64>, Vec<String>) = rows.into_iter().unzip();
        // A text the project already has a vector for (a footer, a how-to on every page) takes
        // that vector, and one the batch holds twice is embedded once (T-4749): the model's time
        // goes to text it has not read.
        let mut literal: HashMap<String, String> = sqlx::query_as(
            "SELECT DISTINCT ON (text) text, embedding::text FROM chunks \
             WHERE embedding IS NOT NULL AND text = ANY($1)",
        )
        .bind(&texts)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
        let mut fresh: Vec<String> = Vec::new();
        for text in &texts {
            if !literal.contains_key(text) && !fresh.contains(text) {
                fresh.push(text.clone());
            }
        }
        let vectors = embedder.passages(&fresh).await?;
        for (text, vector) in fresh.into_iter().zip(&vectors) {
            literal.insert(text, vector_literal(vector)?);
        }
        for (id, text) in ids.iter().zip(&texts) {
            sqlx::query("UPDATE chunks SET embedding = $2::vector WHERE id = $1")
                .bind(id)
                .bind(literal.get(text))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        done += ids.len();
    }
    Ok(done)
}

/// Embeds the chunks of `project` that have none, [`BATCH`] at a time, until none is left or
/// `until` has passed; how many it embedded. A batch already begun is finished, so the budget
/// is overrun by at most one batch (T-4749).
pub async fn embed_until(
    pool: &PgPool,
    embedder: &Embedder,
    project: &str,
    until: Instant,
) -> Result<usize, Error> {
    let mut done = 0;
    while Instant::now() < until {
        match embed_missing(pool, embedder, project, BATCH as i64).await? {
            0 => break,
            embedded => done += embedded,
        }
    }
    Ok(done)
}
