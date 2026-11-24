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
pub struct Door(Arc<jc_door::Verifier>);

impl Door {
    /// `jwks_url` is fetched by `jc` itself, so it may name a listener on loopback or the
    /// cluster network.
    pub fn new(cfg: &crate::config::DoorSection) -> Result<Self, String> {
        jc_door::Verifier::new(&cfg.issuer, &cfg.jwks_url, &cfg.clients).map(|v| Self(Arc::new(v)))
    }

    /// Whether `token` is a valid token of an admitted client for `audience`.
    #[must_use]
    pub fn verify(&self, token: &str, audience: &str) -> bool {
        self.0.verify(token, audience)
    }

    /// The verifier itself, for a module that names its callers by their tokens (dataroom's
    /// publisher, S11-23).
    #[must_use]
    pub fn verifier(&self) -> Arc<jc_door::Verifier> {
        Arc::clone(&self.0)
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

/// The client the Portal publishes into a jc catalogue as (S11-23): kaisatsu's client in a role
/// split names its own id (`[remote] client_id`); all in one, jc's in-process issuer names this.
pub const PUBLISHER_CLIENT: &str = "portal-publisher";

/// How long a token the in-process issuer signs is valid.
const LOCAL_TOKEN_SECONDS: i64 = 300;

/// The process's one in-process issuer of publisher tokens (all in one: the Portal and dataroom
/// beside each other), started on first use; a failed start is retried on the next.
pub fn local_issuer() -> Result<Arc<jc_door::issuer::LocalIssuer>, String> {
    static ISSUER: Mutex<Option<Arc<jc_door::issuer::LocalIssuer>>> = Mutex::new(None);
    let mut held = ISSUER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(issuer) = held.as_ref() {
        return Ok(Arc::clone(issuer));
    }
    let issuer = Arc::new(jc_door::issuer::LocalIssuer::start("", "jc-publisher")?);
    *held = Some(Arc::clone(&issuer));
    Ok(issuer)
}

/// A token of [`PUBLISHER_CLIENT`] for dataroom from the in-process issuer.
pub fn local_publisher_token() -> Result<jc_secret::Secret, String> {
    local_issuer()?
        .token(
            PUBLISHER_CLIENT,
            Module::Dataroom.name(),
            LOCAL_TOKEN_SECONDS,
        )
        .map(jc_secret::Secret::new)
}

/// The verifier of the in-process issuer's publisher tokens.
pub fn local_verifier() -> Result<jc_door::Verifier, String> {
    let issuer = local_issuer()?;
    jc_door::Verifier::new(
        issuer.issuer(),
        issuer.jwks_url(),
        &[PUBLISHER_CLIENT.to_owned()],
    )
}

/// The Portal container's [`Remote`], set when it starts the forwarders (role split): the
/// Portal takes its publisher tokens for a dataroom in another container from it.
pub static REMOTE: std::sync::OnceLock<Arc<Remote>> = std::sync::OnceLock::new();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DoorSection, RemoteSection};
    use std::collections::BTreeMap as Map;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt as _;

    fn setup(roles: &[Module]) -> (Setup, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let setup = Setup::prepare(dir.path(), roles, &Map::new()).unwrap();
        (setup, dir)
    }

    /// A module that answers with the `Authorization` header it got, or `none`.
    fn echo() -> Router {
        Router::new().fallback(|req: Request| async move {
            req.headers()
                .get(header::AUTHORIZATION)
                .map_or_else(|| "none".to_owned(), |v| v.to_str().unwrap().to_owned())
        })
    }

    async fn text(router: Router, auth: Option<&str>) -> String {
        let mut req = Request::builder().uri("/x");
        if let Some(auth) = auth {
            req = req.header(header::AUTHORIZATION, auth);
        }
        let resp = router
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    #[test]
    fn the_credential_header_is_basic_for_daiya_and_bearer_for_the_others() {
        let (setup, _dir) = setup(&Module::ALL);
        let daiya = credential_header(Module::Daiya, &setup).unwrap();
        assert!(daiya.is_sensitive());
        let password = setup.value(Secret::DashboardsPassword).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(format!("{DASHBOARDS_USER}:{password}"));
        assert_eq!(daiya.to_str().unwrap(), format!("Basic {encoded}"));
        let shako = credential_header(Module::Shako, &setup).unwrap();
        let token = setup.value(Secret::ShakoToken).unwrap();
        assert_eq!(shako.to_str().unwrap(), format!("Bearer {token}"));
        // A module without a service credential, and one whose secret is not held.
        assert!(credential_header(Module::Dataroom, &setup).is_none());
        let (portal_only, _dir) = self::setup(&[Module::Portal]);
        assert!(credential_header(Module::Ekiben, &portal_only).is_none());
    }

    #[test]
    fn a_credential_with_a_line_break_gives_no_header() {
        let dir = tempfile::tempdir().unwrap();
        let mut given = Map::new();
        given.insert(
            Secret::ShakoToken,
            crate::setup::Given {
                path: None,
                secret: jc_secret::Secret::new("bad\nvalue".to_owned()),
                source: jc_secret::Source::Env,
            },
        );
        let setup = Setup::prepare(dir.path(), &Module::ALL, &given).unwrap();
        assert!(credential_header(Module::Shako, &setup).is_none());
    }

    #[test]
    fn is_credential_takes_each_form_a_client_sends() {
        let h = |s: &str| HeaderValue::from_str(s).unwrap();
        // CKAN: the bare value; Gitea: `token` or `Bearer`, any case, padded.
        assert!(is_credential(Module::Shako, &h("s3cret"), "s3cret"));
        assert!(is_credential(Module::Shako, &h(" token s3cret "), "s3cret"));
        assert!(is_credential(Module::Shako, &h("BEARER  s3cret"), "s3cret"));
        assert!(!is_credential(Module::Shako, &h("Bearer other"), "s3cret"));
        assert!(!is_credential(Module::Shako, &h("Basic s3cret"), "s3cret"));
        assert!(!is_credential(Module::Shako, &h(""), "s3cret"));
        // Not visible ASCII: never equal.
        let opaque = HeaderValue::from_bytes(b"\xffs3cret").unwrap();
        assert!(!is_credential(Module::Shako, &opaque, "s3cret"));
        // daiya: HTTP Basic of the dashboards user only.
        let basic = |user: &str, pw: &str| {
            let enc = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pw}"));
            h(&format!("Basic {enc}"))
        };
        assert!(is_credential(
            Module::Daiya,
            &basic(DASHBOARDS_USER, "pw"),
            "pw"
        ));
        assert!(!is_credential(
            Module::Daiya,
            &basic(DASHBOARDS_USER, "no"),
            "pw"
        ));
        assert!(!is_credential(Module::Daiya, &basic("someone", "pw"), "pw"));
        assert!(!is_credential(Module::Daiya, &h("Bearer pw"), "pw"));
        assert!(!is_credential(Module::Daiya, &h("Basic !!!"), "pw"));
    }

    #[tokio::test]
    async fn the_door_swaps_only_a_verified_token_of_the_modules_audience() {
        let issuer = jc_door::testing::Issuer::start("", "k1").await;
        let door = Arc::new(
            Door::new(&DoorSection {
                issuer: issuer.issuer.clone(),
                jwks_url: issuer.jwks_url.clone(),
                clients: vec!["portal".to_owned()],
            })
            .unwrap(),
        );
        assert_eq!(door.verifier().issuer(), issuer.issuer);
        let (setup, _dir) = setup(&Module::ALL);
        let credential = format!("Bearer {}", setup.value(Secret::ShakoToken).unwrap());
        let guarded = door.guard(Module::Shako, echo(), &setup);

        let good = issuer.token("portal", "shako", 60);
        assert_eq!(
            text(guarded.clone(), Some(&format!("Bearer {good}"))).await,
            credential
        );
        // The guard fetched the issuer's keys on that first call.
        assert!(door.verify(&good, "shako"));
        // Another audience, another client, an expired token, a person's own credential, none.
        for token in [
            issuer.token("portal", "ekiben", 60),
            issuer.token("stranger", "shako", 60),
            issuer.token("portal", "shako", -120),
        ] {
            let sent = format!("Bearer {token}");
            assert_eq!(text(guarded.clone(), Some(&sent)).await, sent);
        }
        assert_eq!(
            text(guarded.clone(), Some("Basic eDp5")).await,
            "Basic eDp5"
        );
        assert_eq!(text(guarded, None).await, "none");
    }

    #[tokio::test]
    async fn a_module_without_a_credential_is_not_guarded() {
        let issuer = jc_door::testing::Issuer::start("", "k1").await;
        let door = Arc::new(
            Door::new(&DoorSection {
                issuer: issuer.issuer.clone(),
                jwks_url: issuer.jwks_url.clone(),
                clients: vec!["portal".to_owned()],
            })
            .unwrap(),
        );
        let (setup, _dir) = setup(&Module::ALL);
        let token = issuer.token("portal", "dataroom", 60);
        let sent = format!("Bearer {token}");
        let router = door.guard(Module::Dataroom, echo(), &setup);
        assert_eq!(text(router, Some(&sent)).await, sent);
    }

    /// kaisatsu's token endpoint: answers `status` with a token naming the call's number.
    async fn token_endpoint(status: StatusCode, calls: Arc<AtomicUsize>) -> String {
        let router = Router::new().route(
            "/token",
            axum::routing::post(move |body: String| {
                let calls = Arc::clone(&calls);
                async move {
                    let n = calls.fetch_add(1, Ordering::SeqCst);
                    assert!(body.contains("grant_type=client_credentials"));
                    assert!(body.contains("client_id=portal"));
                    let scope = body.split('&').find_map(|kv| kv.strip_prefix("scope="));
                    let answer = serde_json::json!({
                        "access_token": format!("t{n}-{}", scope.unwrap_or_default()),
                        "expires_in": 300,
                    });
                    (status, axum::Json(answer))
                }
            }),
        );
        format!("http://{}/token", serve(router).await)
    }

    async fn serve(router: Router) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        addr
    }

    fn remote(token_url: String) -> Arc<Remote> {
        let cfg = RemoteSection {
            token_url,
            client_id: "portal".to_owned(),
            ..RemoteSection::default()
        };
        Arc::new(Remote::new(&cfg, jc_secret::Secret::new("client-secret".to_owned())).unwrap())
    }

    #[tokio::test]
    async fn a_remote_token_is_cached_per_module_until_it_expires() {
        let calls = Arc::new(AtomicUsize::new(0));
        let remote = remote(token_endpoint(StatusCode::OK, Arc::clone(&calls)).await);
        assert_eq!(
            remote.token(Module::Shako).await.unwrap().expose(),
            "t0-shako"
        );
        assert_eq!(
            remote.token(Module::Shako).await.unwrap().expose(),
            "t0-shako"
        );
        assert_eq!(
            remote.token(Module::Ekiben).await.unwrap().expose(),
            "t1-ekiben"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // An entry past its time is fetched again.
        remote.cache.lock().unwrap().insert(
            Module::Shako,
            (jc_secret::Secret::new("old".into()), Instant::now()),
        );
        assert_eq!(
            remote.token(Module::Shako).await.unwrap().expose(),
            "t2-shako"
        );
    }

    #[tokio::test]
    async fn a_refused_token_request_is_an_error_and_nothing_is_cached() {
        let calls = Arc::new(AtomicUsize::new(0));
        let remote = remote(token_endpoint(StatusCode::UNAUTHORIZED, Arc::clone(&calls)).await);
        let e = remote.token(Module::Shako).await.unwrap_err();
        assert!(e.contains("401"), "{e}");
        assert!(!e.contains("client-secret"));
        assert!(remote.cache.lock().unwrap().is_empty());
    }

    /// The other container's module: echoes method, path, the `Authorization` and `Host` it
    /// got and the body length.
    async fn upstream() -> String {
        let router = Router::new().fallback(|req: Request| async move {
            let auth = req
                .headers()
                .get(header::AUTHORIZATION)
                .map_or("none", |v| v.to_str().unwrap())
                .to_owned();
            let line = format!(
                "{} {} {auth} {}",
                req.method(),
                req.uri(),
                req.headers().contains_key("x-kept")
            );
            let body = axum::body::to_bytes(req.into_body(), 1 << 20)
                .await
                .unwrap();
            (
                StatusCode::CREATED,
                [("x-answer", "1")],
                format!("{line} {}", body.len()),
            )
        });
        format!("http://{}/base", serve(router).await)
    }

    async fn call(router: Router, auth: Option<&str>) -> (StatusCode, String, bool) {
        let mut req = Request::builder()
            .method("PUT")
            .uri("/api/v1/x?y=1")
            .header("x-kept", "1")
            .header(header::HOST, "elsewhere");
        if let Some(auth) = auth {
            req = req.header(header::AUTHORIZATION, auth);
        }
        let resp = router
            .oneshot(req.body(Body::from("abc")).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let answered = resp.headers().contains_key("x-answer");
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap(), answered)
    }

    #[tokio::test]
    async fn the_forwarder_swaps_only_the_local_credential_for_a_token() {
        let calls = Arc::new(AtomicUsize::new(0));
        let remote = remote(token_endpoint(StatusCode::OK, Arc::clone(&calls)).await);
        let base = upstream().await;
        let (listener, router) = remote
            .forward(Module::Shako, base, jc_secret::Secret::new("local".into()))
            .await
            .unwrap();
        assert!(listener.local_addr().unwrap().ip().is_loopback());

        let (status, body, answered) = call(router.clone(), Some("token local")).await;
        assert_eq!(status, StatusCode::CREATED);
        assert!(answered);
        assert_eq!(body, "PUT /base/api/v1/x?y=1 Bearer t0-shako true 3");
        // Any other credential, or none, goes on unchanged and fetches no token.
        let (_, body, _) = call(router.clone(), Some("Bearer someone")).await;
        assert_eq!(body, "PUT /base/api/v1/x?y=1 Bearer someone true 3");
        let (_, body, _) = call(router, None).await;
        assert_eq!(body, "PUT /base/api/v1/x?y=1 none true 3");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn the_forwarder_answers_502_without_a_token_or_a_reachable_module() {
        let refused =
            remote(token_endpoint(StatusCode::FORBIDDEN, Arc::new(AtomicUsize::new(0))).await);
        let (_l, router) = refused
            .forward(
                Module::Ekiben,
                upstream().await,
                jc_secret::Secret::new("local".into()),
            )
            .await
            .unwrap();
        let (status, body, _) = call(router, Some("Bearer local")).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body, "no token for the remote module");

        // A module whose listener is gone.
        let gone = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", gone.local_addr().unwrap());
        drop(gone);
        let ok = remote(token_endpoint(StatusCode::OK, Arc::new(AtomicUsize::new(0))).await);
        let (_l, router) = ok
            .forward(Module::Ekiben, base, jc_secret::Secret::new("local".into()))
            .await
            .unwrap();
        let (status, _, _) = call(router, None).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn a_token_that_is_no_header_value_answers_502() {
        let router = Router::new().route(
            "/token",
            axum::routing::post(|| async {
                axum::Json(serde_json::json!({"access_token": "a\nb", "expires_in": 300}))
            }),
        );
        let remote = remote(format!("http://{}/token", serve(router).await));
        let (_l, router) = remote
            .forward(
                Module::Shako,
                upstream().await,
                jc_secret::Secret::new("local".into()),
            )
            .await
            .unwrap();
        let (status, _, _) = call(router, Some("local")).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn the_local_issuer_signs_publisher_tokens_its_verifier_admits() {
        let a = local_issuer().unwrap();
        let b = local_issuer().unwrap();
        assert!(Arc::ptr_eq(&a, &b), "one issuer per process");
        let token = local_publisher_token().unwrap();
        let verifier = local_verifier().unwrap();
        assert_eq!(verifier.issuer(), a.issuer());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(verifier.prepare(token.expose()));
        assert_eq!(
            verifier
                .client(token.expose(), Module::Dataroom.name())
                .as_deref(),
            Some(PUBLISHER_CLIENT)
        );
        assert!(!verifier.verify(token.expose(), Module::Shako.name()));
    }
}
