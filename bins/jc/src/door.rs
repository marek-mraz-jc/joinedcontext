//! Role split, both sides of a module call that leaves the container.
//!
//! - **The module's container** ([`Door`], `[door]`): a request whose bearer token kaisatsu
//!   signed for an admitted client, with the module's name as audience, reaches the module as
//!   its service caller: the door swaps the token for the module's per-boot credential, which
//!   never leaves this process. The door checks the signature against kaisatsu's JWKS, `iss`,
//!   `aud`, `exp` and `azp`. Any other request (a person's own token, a token for another
//!   module) reaches the module unchanged, and the module answers it as it always does, so the
//!   door opens nothing that was closed.
//! - **The Portal's container** ([`Remote`], `[remote]`): the Portal calls a remote module on a
//!   loopback forwarder with the per-boot credential `jc` gave it. The forwarder swaps exactly
//!   that credential for a client-credentials token of the module's audience and passes the
//!   request on; any other credential goes on unchanged.
//!
//! Neither side logs a token, a credential or a claim.

use crate::module::Module;
use crate::setup::{DASHBOARDS_USER, Secret, Setup};
use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// The least time between two JWKS fetches, so unknown `kid`s cannot make the door hammer
/// kaisatsu.
pub const REFETCH_AFTER: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_JWKS_BYTES: usize = 256 * 1024;
const MAX_TOKEN_BYTES: usize = 64 * 1024;
/// ponytail: the forwarder holds a body whole; a Portal call carries manifests, not packs. Raise
/// it or stream when a call outgrows it.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
/// The algorithms kaisatsu signs with (`kaisatsu-crypto` keys.rs); no HMAC, no `none`.
const ALGORITHMS: [Algorithm; 3] = [Algorithm::ES256, Algorithm::RS256, Algorithm::EdDSA];

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("cannot build an HTTP client: {e}"))
}

/// Reads at most `cap` bytes of an answer.
async fn capped(mut resp: reqwest::Response, cap: usize) -> Result<Vec<u8>, String> {
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
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| rest.trim())
        .filter(|t| !t.is_empty() && t.len() <= MAX_TOKEN_BYTES)
}

/// The `Authorization` value a module takes its service credential in: HTTP Basic for daiya
/// (as Grafana's admin user), a bearer token for the others.
#[must_use]
pub fn credential_header(module: Module, setup: &Setup) -> Option<HeaderValue> {
    let value = setup.value(Secret::service_credential(module)?)?;
    let text = match module {
        Module::Daiya => format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{DASHBOARDS_USER}:{value}"))
        ),
        _ => format!("Bearer {value}"),
    };
    let mut header = HeaderValue::from_str(&text).ok()?;
    header.set_sensitive(true);
    Some(header)
}

/// Whether `got` is `module`'s service credential `value` in a form a Portal client sends it:
/// HTTP Basic for daiya, else the bare value (CKAN) or after `Bearer`/`token` (Gitea). Compared
/// in constant time.
#[must_use]
pub fn is_credential(module: Module, got: &HeaderValue, value: &str) -> bool {
    let Ok(got) = got.to_str() else {
        return false;
    };
    let got = got.trim();
    if module == Module::Daiya {
        let Some(encoded) = got.strip_prefix("Basic ") else {
            return false;
        };
        let expected = format!("{DASHBOARDS_USER}:{value}");
        return base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .is_ok_and(|raw| jc_secret::constant_time_eq(&raw, expected.as_bytes()));
    }
    let bare = match got.split_once(' ') {
        Some((scheme, rest))
            if scheme.eq_ignore_ascii_case("bearer") || scheme.eq_ignore_ascii_case("token") =>
        {
            rest.trim()
        }
        _ => got,
    };
    jc_secret::constant_time_eq(bare.as_bytes(), value.as_bytes())
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

/// The verifier of a module container's door.
pub struct Door {
    issuer: String,
    jwks_url: String,
    clients: Vec<String>,
    http: reqwest::Client,
    keys: RwLock<Keys>,
}

impl std::fmt::Debug for Door {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Door")
            .field("issuer", &self.issuer)
            .field("jwks_url", &self.jwks_url)
            .field("clients", &self.clients)
            .finish()
    }
}

impl Door {
    /// `jwks_url` is fetched by `jc` itself, so it may name a listener on loopback or the
    /// cluster network.
    pub fn new(cfg: &crate::config::DoorSection) -> Result<Self, String> {
        Ok(Self {
            issuer: cfg.issuer.clone(),
            jwks_url: cfg.jwks_url.clone(),
            clients: cfg
                .clients
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

    fn keys(&self) -> std::sync::RwLockReadGuard<'_, Keys> {
        self.keys
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Fetches the JWKS when `token` names a key the cache lacks and the last fetch is older
    /// than [`REFETCH_AFTER`]. A failed fetch keeps the old keys.
    async fn prepare(&self, token: &str) {
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

    /// Whether `token` is a valid token of an admitted client for `audience`.
    #[must_use]
    pub fn verify(&self, token: &str, audience: &str) -> bool {
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
            let keys = self.keys();
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
        validation.set_audience(&[audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
        validation.leeway = 30;
        let Ok(data) = decode::<Claims>(token, &key, &validation) else {
            return false;
        };
        data.claims
            .azp
            .or(data.claims.client_id)
            .is_some_and(|c| self.clients.contains(&c))
    }

    /// `router` behind the door: a verified token of `module`'s audience becomes the module's
    /// service credential. A module without one in `setup` is returned as it is.
    pub fn guard(self: &Arc<Self>, module: Module, router: Router, setup: &Setup) -> Router {
        let Some(credential) = credential_header(module, setup) else {
            return router;
        };
        let door = Arc::clone(self);
        router.layer(axum::middleware::from_fn(
            move |mut req: Request, next: Next| {
                let door = Arc::clone(&door);
                let credential = credential.clone();
                async move {
                    if let Some(token) = bearer(req.headers()).map(str::to_owned) {
                        door.prepare(&token).await;
                        if door.verify(&token, module.name()) {
                            req.headers_mut().insert(header::AUTHORIZATION, credential);
                        }
                    }
                    next.run(req).await
                }
            },
        ))
    }
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    expires_in: Option<u64>,
}

/// The Portal container's client of kaisatsu's token endpoint, one cached token per module.
pub struct Remote {
    token_url: String,
    client_id: String,
    secret: jc_secret::Secret,
    http: reqwest::Client,
    cache: Mutex<BTreeMap<Module, (jc_secret::Secret, Instant)>>,
}

impl Remote {
    pub fn new(
        cfg: &crate::config::RemoteSection,
        secret: jc_secret::Secret,
    ) -> Result<Self, String> {
        Ok(Self {
            token_url: cfg.token_url.clone(),
            client_id: cfg.client_id.clone(),
            secret,
            http: http_client()?,
            cache: Mutex::new(BTreeMap::new()),
        })
    }

    /// A client-credentials token with `module`'s name as scope (kaisatsu maps it to the
    /// audience), reused until 30 s before it expires.
    pub async fn token(&self, module: Module) -> Result<jc_secret::Secret, String> {
        if let Some((token, until)) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&module)
            && Instant::now() < *until
        {
            return Ok(token.clone());
        }
        let resp = self
            .http
            .post(&self.token_url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.secret.expose()),
                ("scope", module.name()),
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
        let token = jc_secret::Secret::new(answer.access_token);
        let life = Duration::from_secs(answer.expires_in.unwrap_or(60).saturating_sub(30));
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(module, (token.clone(), Instant::now() + life));
        Ok(token)
    }

    /// Starts the loopback forwarder to `module` at `base` (the public base of its mount in the
    /// other container). `local` is the credential the Portal was given for it.
    pub async fn forward(
        self: &Arc<Self>,
        module: Module,
        base: String,
        local: jc_secret::Secret,
    ) -> std::io::Result<(tokio::net::TcpListener, Router)> {
        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
        let remote = Arc::clone(self);
        let router = Router::new().fallback(move |req: Request| {
            let remote = Arc::clone(&remote);
            let base = base.clone();
            let local = local.clone();
            async move { remote.pass(module, &base, &local, req).await }
        });
        Ok((listener, router))
    }

    async fn pass(
        &self,
        module: Module,
        base: &str,
        local: &jc_secret::Secret,
        req: Request,
    ) -> Response {
        let (parts, body) = req.into_parts();
        let path = parts.uri.path_and_query().map_or("/", |p| p.as_str());
        let Ok(body) = axum::body::to_bytes(body, MAX_BODY_BYTES).await else {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        };
        let mut headers = parts.headers;
        for hop in [
            header::HOST,
            header::CONNECTION,
            header::TRANSFER_ENCODING,
            header::CONTENT_LENGTH,
        ] {
            headers.remove(hop);
        }
        let is_local = headers
            .get(header::AUTHORIZATION)
            .is_some_and(|got| is_credential(module, got, local.expose()));
        if is_local {
            match self.token(module).await {
                Ok(token) => {
                    let Ok(mut value) =
                        HeaderValue::from_str(&format!("Bearer {}", token.expose()))
                    else {
                        return StatusCode::BAD_GATEWAY.into_response();
                    };
                    value.set_sensitive(true);
                    headers.insert(header::AUTHORIZATION, value);
                }
                Err(e) => {
                    tracing::warn!(module = module.name(), error = %e, "no token for the remote module");
                    return (StatusCode::BAD_GATEWAY, "no token for the remote module")
                        .into_response();
                }
            }
        }
        let sent = self
            .http
            .request(parts.method, format!("{base}{path}"))
            .headers(headers)
            .body(body)
            .send()
            .await;
        let resp = match sent {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!(module = module.name(), error = %e, "the remote module is unreachable");
                return StatusCode::BAD_GATEWAY.into_response();
            }
        };
        let status = resp.status();
        let mut answer_headers = resp.headers().clone();
        for hop in [
            header::CONNECTION,
            header::TRANSFER_ENCODING,
            header::CONTENT_LENGTH,
        ] {
            answer_headers.remove(hop);
        }
        match capped(resp, MAX_BODY_BYTES).await {
            Ok(bytes) => {
                let mut out = Response::new(Body::from(bytes));
                *out.status_mut() = status;
                *out.headers_mut() = answer_headers;
                out
            }
            Err(_) => StatusCode::BAD_GATEWAY.into_response(),
        }
    }
}
