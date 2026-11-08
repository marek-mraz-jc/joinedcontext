//! Bearer tokens an OIDC provider (kaisatsu inside `jc`) signs for ekiben: a JWT is admitted as
//! the admin caller when its signature verifies against the provider's JWKS, its `iss` is the
//! configured issuer, its `aud` holds the configured audience, it has not expired, and its
//! `azp` (or `client_id`) is one of the configured clients. No client configured admits nobody.
//!
//! Verification is synchronous against the cached key set; [`OidcAuth::prepare`] fetches the
//! JWKS before it when a token names a key the cache lacks, at most once per
//! [`REFETCH_AFTER`], so unknown `kid`s cannot make ekiben hammer the provider. Neither the
//! token nor its claims are logged.

use std::sync::RwLock;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

/// The least time between two JWKS fetches.
pub const REFETCH_AFTER: Duration = Duration::from_secs(30);
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_JWKS_BYTES: usize = 256 * 1024;
/// The algorithms kaisatsu signs with (`kaisatsu-crypto` keys.rs); no HMAC, no `none`.
const ALGORITHMS: [Algorithm; 3] = [Algorithm::ES256, Algorithm::RS256, Algorithm::EdDSA];

struct Keys {
    set: JwkSet,
    fetched: Option<Instant>,
}

/// The OIDC provider whose tokens ekiben admits.
pub struct OidcAuth {
    jwks_url: String,
    issuer: String,
    audience: String,
    clients: Vec<String>,
    http: reqwest::Client,
    keys: RwLock<Keys>,
}

impl std::fmt::Debug for OidcAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcAuth")
            .field("jwks_url", &self.jwks_url)
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("clients", &self.clients)
            .finish()
    }
}

#[derive(Deserialize)]
struct Claims {
    azp: Option<String>,
    client_id: Option<String>,
}

impl OidcAuth {
    /// `jwks_url` is fetched by ekiben itself (the provider's `.../protocol/openid-connect/certs`),
    /// not by any stream, so it may name a loopback listener.
    pub fn new(
        jwks_url: impl Into<String>,
        issuer: impl Into<String>,
        audience: impl Into<String>,
        clients: Vec<String>,
    ) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("cannot build the JWKS client: {e}"))?;
        Ok(Self {
            jwks_url: jwks_url.into(),
            issuer: issuer.into(),
            audience: audience.into(),
            clients: clients
                .into_iter()
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect(),
            http,
            keys: RwLock::new(Keys {
                set: JwkSet { keys: Vec::new() },
                fetched: None,
            }),
        })
    }

    fn knows(&self, kid: &str) -> bool {
        self.keys
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set
            .find(kid)
            .is_some()
    }

    /// Fetches the JWKS when `token` is a JWT naming a key the cache lacks and the last fetch is
    /// older than [`REFETCH_AFTER`]. A failed fetch keeps the old keys.
    pub async fn prepare(&self, token: &str) {
        let Some(kid) = decode_header(token).ok().and_then(|h| h.kid) else {
            return;
        };
        if self.knows(&kid) {
            return;
        }
        let due = self
            .keys
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fetched
            .is_none_or(|at| at.elapsed() >= REFETCH_AFTER);
        if !due {
            return;
        }
        // Stamp first, so concurrent requests with unknown keys wait out the period too.
        self.keys
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fetched = Some(Instant::now());
        match self.fetch().await {
            Ok(set) => {
                self.keys
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .set = set;
            }
            Err(e) => tracing::warn!(jwks = %self.jwks_url, error = %e, "JWKS fetch failed"),
        }
    }

    async fn fetch(&self) -> Result<JwkSet, String> {
        let mut resp = self
            .http
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("answered {}", resp.status()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            if body.len() + chunk.len() > MAX_JWKS_BYTES {
                return Err(format!("larger than {MAX_JWKS_BYTES} bytes"));
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|e| format!("not a JWKS: {e}"))
    }

    /// Whether `token` is a valid token of an admitted client.
    pub fn verify(&self, token: &str) -> bool {
        if self.clients.is_empty() {
            return false;
        }
        let Ok(header) = decode_header(token) else {
            return false;
        };
        if !ALGORITHMS.contains(&header.alg) {
            return false;
        }
        let Some(kid) = header.kid else {
            return false;
        };
        let key = {
            let keys = self
                .keys
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(jwk) = keys.set.find(&kid) else {
                return false;
            };
            // A key published for one algorithm verifies only that one.
            if jwk
                .common
                .key_algorithm
                .is_some_and(|a| Algorithm::try_from(a).ok() != Some(header.alg))
            {
                return false;
            }
            match DecodingKey::from_jwk(jwk) {
                Ok(k) => k,
                Err(_) => return false,
            }
        };
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
        validation.leeway = 30;
        let Ok(data) = decode::<Claims>(token, &key, &validation) else {
            return false;
        };
        let client = data.claims.azp.or(data.claims.client_id);
        client.is_some_and(|c| self.clients.iter().any(|allowed| *allowed == c))
    }
}
