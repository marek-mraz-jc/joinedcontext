//! `jc-embedder`: the embedding model one deployment loads in process ([`Embedder`]), the
//! service that serves it to several deployments ([`server`], binary `jc-embedder`), and the
//! client a deployment calls it with ([`client::Remote`]).
//!
//! One API: `POST /v1/embeddings` with `{model, kind, input}` answers `{model, model_sha256,
//! data}`, a vector per text in order. `kind` is `query` (a question: ahead of every passage)
//! or `passage` (a text to index); the service prefixes each as e5 expects. The answer names the
//! SHA-256 of the model file it ran, and the client refuses any other, so vectors of two models
//! never meet in one index.

pub mod client;
mod model;
pub mod server;

pub use model::{Embedder, MODEL_FILES, Took};
use serde::{Deserialize, Serialize};

/// The numbers in one embedding.
pub const DIMENSIONS: usize = 384;

/// The one model the service runs, by name.
pub const MODEL: &str = "multilingual-e5-small-int8";

/// The SHA-256 of the ONNX file [`MODEL`] names, which every answer carries.
pub const MODEL_SHA256: &str = MODEL_FILES[0].1;

/// The service's audience: the scope a calling deployment asks kaisatsu for.
pub const AUDIENCE: &str = "embedder";

/// What a text is to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A question, embedded ahead of every passage.
    Query,
    /// A text to index.
    Passage,
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Query => "query: ",
            Self::Passage => "passage: ",
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Passage => "passage",
        }
    }
}

/// `POST /v1/embeddings`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbedRequest {
    pub model: String,
    pub kind: Kind,
    pub input: Vec<String>,
}

/// One text's vector.
#[derive(Debug, Serialize, Deserialize)]
pub struct Embedding {
    pub index: usize,
    pub embedding: Vec<f32>,
}

/// The answer to an [`EmbedRequest`].
#[derive(Debug, Serialize, Deserialize)]
pub struct EmbedAnswer {
    pub model: String,
    pub model_sha256: String,
    pub data: Vec<Embedding>,
}

/// Why an embedding is not there. None carries a text that was sent.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The model could not load or run.
    #[error("{0}")]
    Model(String),
    /// The embedder service answered no vectors.
    #[error("the embedder refused: {0}")]
    Refused(String),
    /// The embedder service could not be reached.
    #[error("the embedder is unreachable: {0}")]
    Unreachable(String),
}
