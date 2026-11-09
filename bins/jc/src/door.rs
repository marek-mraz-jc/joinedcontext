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
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// ponytail: the forwarder holds a body whole; a Portal call carries manifests, not packs. Raise
/// it or stream when a call outgrows it.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

pub use jc_door::REFETCH_AFTER;
use jc_door::{bearer, capped, http_client};

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

/// The verifier of a module container's door ([`jc_door::Verifier`] over `[door]`).
#[derive(Debug)]
pub struct Door(jc_door::Verifier);

impl Door {
    /// `jwks_url` is fetched by `jc` itself, so it may name a listener on loopback or the
    /// cluster network.
    pub fn new(cfg: &crate::config::DoorSection) -> Result<Self, String> {
        jc_door::Verifier::new(&cfg.issuer, &cfg.jwks_url, &cfg.clients).map(Self)
    }

    /// Whether `token` is a valid token of an admitted client for `audience`.
    #[must_use]
    pub fn verify(&self, token: &str, audience: &str) -> bool {
        self.0.verify(token, audience)
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
                        door.0.prepare(&token).await;
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
        let (token, life) = jc_door::fetch_token(
            &self.http,
            &self.token_url,
            &self.client_id,
            &self.secret,
            module.name(),
        )
        .await?;
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
