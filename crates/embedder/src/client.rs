//! The client a deployment calls the embedder service with: a kaisatsu client-credentials token
//! of audience [`AUDIENCE`](crate::AUDIENCE), [`BATCH`] texts per request, and every answer
//! checked (count, order, length, finite numbers, the pinned model's SHA-256) before a vector is
//! handed on. A failing service is a refusal: nothing falls back to another model.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::{DIMENSIONS, EmbedAnswer, EmbedRequest, Error, Kind, MODEL, MODEL_SHA256};

/// Texts per request: the assistant's indexing batch (`embed::BATCH`).
pub const BATCH: usize = 16;
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ANSWER_BYTES: usize = 4 * 1024 * 1024;

/// Where the service is and how this deployment signs in to it.
pub struct RemoteConfig {
    /// The service's base URL (`https://embedder.example`); requests go to `{url}/v1/embeddings`.
    pub url: String,
    /// This deployment's kaisatsu token endpoint.
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
}

/// The embedder service, as one deployment calls it.
pub struct Remote {
    endpoint: String,
    token_url: String,
    client_id: String,
    secret: jc_secret::Secret,
    http: reqwest::Client,
    token: Mutex<Option<(jc_secret::Secret, Instant)>>,
}

impl std::fmt::Debug for Remote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Remote")
            .field("endpoint", &self.endpoint)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl Remote {
    pub fn new(cfg: RemoteConfig) -> Result<Self, Error> {
        let url = cfg.url.trim_end_matches('/');
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(Error::Refused(format!(
                "the embedder URL {url} is not an http(s) URL"
            )));
        }
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Error::Unreachable(format!("cannot build an HTTP client: {e}")))?;
        Ok(Self {
            endpoint: format!("{url}/v1/embeddings"),
            token_url: cfg.token_url,
            client_id: cfg.client_id,
            secret: jc_secret::Secret::new(cfg.client_secret),
            http,
            token: Mutex::new(None),
        })
    }

    async fn token(&self) -> Result<jc_secret::Secret, Error> {
        if let Some((token, until)) = self
            .token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            && Instant::now() < *until
        {
            return Ok(token.clone());
        }
        let (token, life) = jc_door::fetch_token(
            &self.http,
            &self.token_url,
            &self.client_id,
            &self.secret,
            crate::AUDIENCE,
        )
        .await
        .map_err(|e| Error::Refused(format!("no token for the embedder: {e}")))?;
        *self
            .token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((token.clone(), Instant::now() + life));
        Ok(token)
    }

    /// The embeddings of `texts` read as `kind`, in order.
    pub async fn embed(&self, kind: Kind, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH) {
            vectors.extend(self.batch(kind, batch).await?);
        }
        Ok(vectors)
    }

    async fn batch(&self, kind: Kind, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        let token = self.token().await?;
        let resp = self
            .http
            .post(&self.endpoint)
            .bearer_auth(token.expose())
            .json(&EmbedRequest {
                model: MODEL.to_owned(),
                kind,
                input: texts.to_vec(),
            })
            .send()
            .await
            // reqwest's error names the URL, never the body.
            .map_err(|e| Error::Unreachable(e.without_url().to_string()))?;
        let status = resp.status();
        let body = jc_door::capped(resp, MAX_ANSWER_BYTES)
            .await
            .map_err(|e| Error::Unreachable(format!("the answer could not be read: {e}")))?;
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                // A token the service no longer takes (a key rotated): fetch a new one next time.
                *self
                    .token
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }
            let why = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_owned))
                .unwrap_or_default();
            return Err(Error::Refused(format!("{status} {why}")));
        }
        let answer: EmbedAnswer = serde_json::from_slice(&body)
            .map_err(|_| Error::Refused("the answer is not an embedding answer".into()))?;
        checked(answer, texts.len())
    }
}

/// The vectors of `answer` once it holds `count` of them, in order, from the pinned model.
fn checked(answer: EmbedAnswer, count: usize) -> Result<Vec<Vec<f32>>, Error> {
    if answer.model_sha256 != MODEL_SHA256 || answer.model != MODEL {
        return Err(Error::Refused(format!(
            "the embedder runs model {} ({}), not the pinned {MODEL} ({MODEL_SHA256}); its vectors would not match the index",
            answer.model, answer.model_sha256
        )));
    }
    if answer.data.len() != count {
        return Err(Error::Refused(format!(
            "{} vectors for {count} texts",
            answer.data.len()
        )));
    }
    answer
        .data
        .into_iter()
        .enumerate()
        .map(|(at, e)| {
            if e.index != at
                || e.embedding.len() != DIMENSIONS
                || !e.embedding.iter().all(|v| v.is_finite())
            {
                return Err(Error::Refused(format!(
                    "vector {at} is not {DIMENSIONS} finite numbers in order"
                )));
            }
            Ok(e.embedding)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Embedding;

    fn answer(sha: &str, n: usize) -> EmbedAnswer {
        EmbedAnswer {
            model: MODEL.into(),
            model_sha256: sha.into(),
            data: (0..n)
                .map(|index| Embedding {
                    index,
                    embedding: vec![0.1; DIMENSIONS],
                })
                .collect(),
        }
    }

    #[test]
    fn an_answer_of_another_model_is_refused() {
        assert_eq!(
            checked(answer(MODEL_SHA256, 2), 2).expect("pinned").len(),
            2
        );
        let other = checked(answer(&"0".repeat(64), 2), 2).err();
        assert!(
            matches!(&other, Some(Error::Refused(why)) if why.contains("not the pinned")),
            "{other:?}"
        );
    }

    #[test]
    fn a_short_unordered_or_broken_answer_is_refused() {
        assert!(checked(answer(MODEL_SHA256, 1), 2).is_err());
        let mut swapped = answer(MODEL_SHA256, 2);
        swapped.data.swap(0, 1);
        assert!(checked(swapped, 2).is_err());
        let mut nan = answer(MODEL_SHA256, 1);
        nan.data[0].embedding[3] = f32::NAN;
        assert!(checked(nan, 1).is_err());
        let mut short = answer(MODEL_SHA256, 1);
        short.data[0].embedding.pop();
        assert!(checked(short, 1).is_err());
    }
}
