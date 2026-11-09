//! kaisatsu client-credentials, both sides of a call between two services.
//!
//! - **The service's door** ([`Verifier`]): a bearer token kaisatsu signed for an admitted client,
//!   with the service's name as audience, names its caller ([`Verifier::client`]). The signature
//!   is checked against kaisatsu's JWKS, with `iss`, `aud`, `exp` and `azp`.
//! - **The caller** ([`fetch_token`]): a client-credentials token with the service's name as
//!   scope, which kaisatsu maps to the audience.
//!
//! Moved out of `bins/jc/src/door.rs`, which keeps the jc wiring (the credential swap, the
//! Portal's forwarder) over these, so the embedder service (`crates/embedder`) checks its callers
//! with the same code. Nothing here logs a token, a credential or a claim.

use http::{HeaderMap, header};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// The least time between two JWKS fetches, so unknown `kid`s cannot make the door hammer
/// kaisatsu.
pub const REFETCH_AFTER: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_JWKS_BYTES: usize = 256 * 1024;
/// The longest bearer token taken from a request or a token answer.
pub const MAX_TOKEN_BYTES: usize = 64 * 1024;
/// The algorithms kaisatsu signs with (`kaisatsu-crypto` keys.rs); no HMAC, no `none`.
const ALGORITHMS: [Algorithm; 3] = [Algorithm::ES256, Algorithm::RS256, Algorithm::EdDSA];

/// An HTTP client with a timeout that follows no redirect.
pub fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("cannot build an HTTP client: {e}"))
}

/// Reads at most `cap` bytes of an answer.
pub async fn capped(mut resp: reqwest::Response, cap: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > cap {
            return Err(format!("larger than {cap} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// The value of `Authorization: Bearer <value>`.
#[must_use]
pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| rest.trim())
        .filter(|t| !t.is_empty() && t.len() <= MAX_TOKEN_BYTES)
}

struct Keys {
    set: JwkSet,
    fetched: Option<Instant>,
}

#[derive(Deserialize)]
struct Claims {
    azp: Option<String>,
    client_id: Option<String>,
}

/// The verifier of one issuer's tokens for a service's door.
pub struct Verifier {
    issuer: String,
    jwks_url: String,
    clients: Vec<String>,
    http: reqwest::Client,
    keys: RwLock<Keys>,
}

impl std::fmt::Debug for Verifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Verifier")
            .field("issuer", &self.issuer)
            .field("jwks_url", &self.jwks_url)
            .field("clients", &self.clients)
            .finish()
    }
}

impl Verifier {
    /// Tokens `issuer` signs with the keys at `jwks_url`, for the client ids `clients`.
    /// `jwks_url` is fetched by this process itself, so it may name a listener on loopback or
    /// the cluster network.
    pub fn new(issuer: &str, jwks_url: &str, clients: &[String]) -> Result<Self, String> {
        Ok(Self {
            issuer: issuer.to_owned(),
            jwks_url: jwks_url.to_owned(),
            clients: clients
                .iter()
                .map(|c| c.trim().to_owned())
                .filter(|c| !c.is_empty())
                .collect(),
            http: http_client()?,
            keys: RwLock::new(Keys {
                set: JwkSet { keys: Vec::new() },
                fetched: None,
            }),
        })
    }

    /// The issuer this verifier admits tokens of.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    fn keys(&self) -> std::sync::RwLockReadGuard<'_, Keys> {
        self.keys
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Fetches the JWKS when `token` names a key the cache lacks and the last fetch is older
    /// than [`REFETCH_AFTER`]. A failed fetch keeps the old keys.
    pub async fn prepare(&self, token: &str) {
        let Some(kid) = decode_header(token).ok().and_then(|h| h.kid) else {
            return;
        };
        let due = {
            let keys = self.keys();
            keys.set.find(&kid).is_none()
                && keys.fetched.is_none_or(|at| at.elapsed() >= REFETCH_AFTER)
        };
        if !due {
            return;
        }
        // Stamp first, so concurrent requests naming unknown keys wait out the period too.
        self.keys
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fetched = Some(Instant::now());
        let fetched = async {
            let resp = self
                .http
                .get(&self.jwks_url)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                return Err(format!("answered {}", resp.status()));
            }
            let body = capped(resp, MAX_JWKS_BYTES).await?;
            serde_json::from_slice::<JwkSet>(&body).map_err(|e| format!("not a JWKS: {e}"))
        };
        match fetched.await {
            Ok(set) => {
                self.keys
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set = set;
            }
            Err(e) => tracing::warn!(jwks = %self.jwks_url, error = %e, "JWKS fetch failed"),
        }
    }

    /// The admitted client (`azp`, else `client_id`) of `token`, when it is a valid token of
    /// this issuer for `audience`.
    #[must_use]
    pub fn client(&self, token: &str, audience: &str) -> Option<String> {
        if self.clients.is_empty() {
            return None;
        }
        let header = decode_header(token).ok()?;
        if !ALGORITHMS.contains(&header.alg) {
            return None;
        }
        let kid = header.kid?;
        let key = {
            let keys = self.keys();
            let jwk = keys.set.find(&kid)?;
            // A key published for one algorithm verifies only that one.
            if jwk
                .common
                .key_algorithm
                .is_some_and(|a| Algorithm::try_from(a).ok() != Some(header.alg))
            {
                return None;
            }
            DecodingKey::from_jwk(jwk).ok()?
        };
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
        validation.leeway = 30;
        let data = decode::<Claims>(token, &key, &validation).ok()?;
        data.claims
            .azp
            .or(data.claims.client_id)
            .filter(|c| self.clients.contains(c))
    }

    /// Whether `token` is a valid token of an admitted client for `audience`.
    #[must_use]
    pub fn verify(&self, token: &str, audience: &str) -> bool {
        self.client(token, audience).is_some()
    }
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    expires_in: Option<u64>,
}

/// A client-credentials token of `client_id` from `token_url` with `scope` (kaisatsu maps it to
/// the audience), and how long to reuse it: until 30 s before it expires.
pub async fn fetch_token(
    http: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    secret: &jc_secret::Secret,
    scope: &str,
) -> Result<(jc_secret::Secret, Duration), String> {
    let resp = http
        .post(token_url)
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            ("client_secret", secret.expose()),
            ("scope", scope),
        ])
        .send()
        .await
        .map_err(|e| format!("the token endpoint is unreachable: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("the token endpoint answered {}", resp.status()));
    }
    let body = capped(resp, MAX_TOKEN_BYTES).await?;
    let answer: TokenAnswer =
        serde_json::from_slice(&body).map_err(|_| "the token endpoint answered no token")?;
    let life = Duration::from_secs(answer.expires_in.unwrap_or(60).saturating_sub(30));
    Ok((jc_secret::Secret::new(answer.access_token), life))
}

#[cfg(test)]
mod tests;

#[cfg(feature = "testing")]
pub mod testing;
