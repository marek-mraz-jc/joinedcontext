//! The App hosts `{name}.apps.{apex}` answered by the Portal itself (AP-133, edge-session-design
//! S4a), where no edge composes per-App routes: jc routes every App host to the Portal, and this
//! layer does what the Portal's edge file has APISIX do on dev (`reconciler::edge_file`).
//!
//! On an App host nothing of the Portal answers but that App:
//! - a page or file goes to the static host under `/apps/{name}/`, the old `/apps/{name}/` prefix
//!   kept optional as on the edge;
//! - `/api/endpoint/{slug}/…` goes to the gateway for the App's own slugs alone, never the egress
//!   path, without the browser's cookies;
//! - `/api/services/…` and a `ui` App's `/api/functions/…` go to their routes under `/apps/{name}/`;
//! - a `wasm` App's own server (`/api/…` otherwise) goes to its shard
//!   `jc-wasm-host-{status.shard}.{JC_PORTAL_WASM_HOST_NAMESPACE}` under `/apps/{name}/api/…`, as
//!   the edge sent it (AP-143), after the App's visibility admits the caller: the shard never asks;
//! - a `ui-rust` App's pod named in `JC_PORTAL_APP_PODS` gets every other path, streamed, with the
//!   App's Content-Security-Policy, as the edge file routes Service `app-{name}`, after the App's
//!   visibility admits the caller;
//! - a wasm App without a shard, a shard the host does not run, or no WASM host configured, and
//!   any other `ui-rust` App's pod, which this Portal does not run: a `503` naming the App, never a
//!   raw error.
//!
//! A host one label under the apex that names no published App gets the static host's own `404`.
//! Any other host passes untouched.
//!
//! The login is the App's own, as the edge's `openid-connect` was (ADR-N-030, AP-29, AP-92): the
//! App's client `app-{name}` with its secret from the realm, PKCE, `redirect_uri`
//! `https://{host}/callback`, `/logout`, and a session cookie `__Host-jc_app_{name}`, host-only on
//! the App's host, holding a random id; the tokens stay in this process. A non-public App's page
//! asked for without a session goes to the realm, and its `/api/…` without a session or a bearer
//! answers `401` before anything is handed on; every request with one carries the App's access
//! token as its bearer, renewed by the refresh token when it expired (the client's lifespan is a
//! minute, T-4914). Another App's cookie never reaches this host, and a session id is bound to its
//! App, so one App's session never opens another.
//!
//! The realm is the App's (T-7165): its project's Organization's own when that Organization signs
//! in through one (`auth::realms::of_app`), else the installation's. Sign-in, code, refresh, logout
//! and the token check all go there, so a token of another realm never opens the App.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jc_core::kinds::AppClass;

use crate::state::AppState;

/// The App a request's host names: `{name}` of `{name}.apps.{apex}`, compared without case and
/// port. `None` on any other host, also when no apps origin is configured or this Portal does not
/// serve the App hosts (`serve_app_hosts`).
pub fn app_of_host(
    state: &AppState,
    headers: &HeaderMap,
    request_host: Option<&str>,
) -> Option<String> {
    if !state.config.serve_app_hosts {
        return None;
    }
    let apex = state
        .config
        .apps_url
        .as_ref()?
        .host_str()?
        .to_ascii_lowercase();
    let raw = request_host.or_else(|| headers.get(header::HOST)?.to_str().ok())?;
    let host = raw
        .parse::<axum::http::uri::Authority>()
        .ok()?
        .host()
        .to_ascii_lowercase();
    let name = host.strip_suffix(&format!(".apps.{apex}"))?;
    (crate::resource::is_dns1123(name) && !name.contains('.')).then(|| name.to_owned())
}

/// Headers a hop never hands on (RFC 9110 §7.6.1), with the ones this layer sets or drops itself.
const DROPPED: [&str; 11] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "cookie",
    "set-cookie",
    "x-access-token",
];

/// The layer in front of every Portal route (`server::app`).
pub async fn middleware(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let request_host = req
        .extensions()
        .get::<crate::organization_host::RequestHost>()
        .map(|host| host.0.clone());
    let Some(name) = app_of_host(&state, req.headers(), request_host.as_deref()) else {
        return next.run(req).await;
    };
    let path = req.uri().path();
    // The old path under the App's own host lands on the same file, as on the edge.
    let rest = path
        .strip_prefix(&format!("/apps/{name}"))
        .filter(|rest| rest.is_empty() || rest.starts_with('/'))
        .unwrap_or(path)
        .to_owned();
    let rest = if rest.is_empty() {
        "/".to_owned()
    } else {
        rest
    };
    let app = super::static_host::published_app(&state, &name);
    if app.is_none() && state.mirror.is_empty() {
        return crate::organization_host::starting(
            super::roles::unavailable_page(
                req.headers(),
                &name,
                None,
                super::static_host::portal_origin(&state).as_deref(),
            )
            .unwrap_or_else(|| {
                problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    crate::organization_host::STARTING,
                )
            }),
        );
    }
    let kind = app.as_ref().map(|(_, spec, _)| spec.class);

    if let Some(after) = rest.strip_prefix("/api/endpoint/") {
        let slug = after.split('/').next().unwrap_or_default();
        let egress = after.split('/').nth(1) == Some("egress");
        let own = app.as_ref().is_some_and(|(project, spec, _)| {
            super::static_host::served_endpoints(&state.mirror, project, &name, spec)
                .iter()
                .any(|endpoint| endpoint.slug == slug)
        });
        if slug.is_empty() || egress || !own {
            return problem(StatusCode::NOT_FOUND, "not one of this App's endpoints");
        }
    }
    if let Some((_, spec, _)) = &app {
        match rest.as_str() {
            "/callback" => return login::callback(&state, req.headers(), req.uri(), &name).await,
            "/logout" => return login::logout(&state, &req, &name),
            "/login" => return login::start(&state, &name, "/"),
            _ => {}
        }
        match login::session(&state, req.headers(), &name).await {
            Some(token) => {
                if let Ok(bearer) = HeaderValue::from_str(&format!("Bearer {token}")) {
                    req.headers_mut().insert(header::AUTHORIZATION, bearer);
                }
            }
            None if spec.visibility != jc_core::kinds::AppVisibility::Public
                && req.method() == axum::http::Method::GET
                && !rest.starts_with("/api/")
                && wants_page(req.headers()) =>
            {
                let back = req.uri().path_and_query().map_or("/", |pq| pq.as_str());
                let back = back
                    .strip_prefix(&format!("/apps/{name}"))
                    .filter(|b| b.starts_with('/'))
                    .unwrap_or(back);
                return login::start(&state, &name, back);
            }
            // A call of a non-public App nobody signed in to is handed to nothing (AP-29).
            None if spec.visibility != jc_core::kinds::AppVisibility::Public
                && rest.starts_with("/api/")
                && super::roles::presented(&state, req.headers()).is_none() =>
            {
                return problem(
                    StatusCode::UNAUTHORIZED,
                    &format!("sign in to App '{name}' first"),
                );
            }
            None => {}
        }
    }
    if rest.starts_with("/api/endpoint/") {
        return to_gateway(&state, req, &rest).await;
    }
    let services = rest.starts_with("/api/services/");
    match (&app, kind) {
        (Some((project, spec, _)), Some(AppClass::Wasm))
            if rest.starts_with("/api/") && !services =>
        {
            if let Some(refused) =
                super::static_host::refused(&state, req.headers(), project, &name, spec).await
            {
                return refused;
            }
            let Some((base, client)) = wasm_server(&state, project, &name) else {
                return unavailable(&state, &req, &name, "server");
            };
            return forward(
                req,
                &base,
                &format!("/apps/{name}{rest}"),
                &client,
                "App server",
            )
            .await;
        }
        (Some((project, spec, _)), Some(AppClass::UiRust)) if !services => {
            // The pod never asks who calls: the App's visibility is judged here, as for a shard,
            // and before the `503` of a pod this installation lacks, which would name a gated
            // App to a caller its visibility answers `404` (T-7229).
            if let Some(refused) =
                super::static_host::refused(&state, req.headers(), project, &name, spec).await
            {
                return refused;
            }
            let Some(pod) = state.config.app_pods.get(&name) else {
                return unavailable(&state, &req, &name, "pod");
            };
            return to_pod(&state, req, pod, &rest, project, spec).await;
        }
        _ => {}
    }
    let query = req
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    match Uri::try_from(format!("/apps/{name}{rest}{query}")) {
        Ok(uri) => *req.uri_mut() = uri,
        Err(_) => return problem(StatusCode::NOT_FOUND, "no such path"),
    }
    next.run(req).await
}

fn wants_page(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

/// A JSON problem for a fetch, as the gateway and the static host answer one.
fn problem(status: StatusCode, detail: &str) -> Response {
    let body = serde_json::json!({ "status": status.as_u16(), "title": status.canonical_reason(), "detail": detail });
    let mut response = (status, body.to_string()).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    response
}

/// The `503` of an App half this Portal does not run: a page naming the App for a browser, a
/// problem for a fetch.
fn unavailable(state: &AppState, req: &Request, name: &str, half: &str) -> Response {
    let detail = match half {
        "server" => format!("the server of App '{name}' does not run on this installation yet"),
        _ => format!(
            "App '{name}' runs as its own service, which this installation does not run yet"
        ),
    };
    let title = super::static_host::published_app(state, name).and_then(|(project, _, _)| {
        state
            .mirror
            .get(&project, "App", name)
            .and_then(|env| env.metadata.title)
    });
    super::roles::unavailable_page(
        req.headers(),
        name,
        title.as_ref(),
        super::static_host::portal_origin(state).as_deref(),
    )
    .unwrap_or_else(|| problem(StatusCode::SERVICE_UNAVAILABLE, &detail))
}

fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        crate::reach::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(6))
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default()
    })
}

/// The client of an App's pod: no deadline on the whole answer, whose `api/stream` is an event
/// stream that stays open, but 30 s at most between two reads, as the edge file's upstream
/// (`read: 30`); hsl-transport's stream sends a keep-alive every 15 s.
fn pod_http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(6))
            .read_timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default()
    })
}

/// Hands an App's endpoint call to the gateway (`JC_PORTAL_GATEWAY_URL`).
async fn to_gateway(state: &AppState, req: Request, rest: &str) -> Response {
    let Some(gateway) = state.config.gateway_url.as_deref() else {
        return problem(StatusCode::SERVICE_UNAVAILABLE, "no gateway is configured");
    };
    forward(req, gateway, rest, http(), "gateway").await
}

/// Hands a request on a `ui-rust` App's host to its pod (`JC_PORTAL_APP_PODS`) at the same path,
/// the App's base path being `/` there, and the answer back with the App's Content-Security-Policy
/// in place of any the pod sent, as the edge file's route sets it (AP-12, AP-122).
async fn to_pod(
    state: &AppState,
    req: Request,
    pod: &str,
    rest: &str,
    project: &str,
    spec: &jc_core::kinds::AppSpec,
) -> Response {
    let mut response = forward(req, pod, rest, pod_http(), "App's server").await;
    let basemap = crate::api::basemap::route_prefix(&state.config, project);
    let policy = super::static_host::content_security_policy(
        spec,
        super::static_host::portal_origin(state).as_deref(),
        basemap.as_deref(),
    );
    let headers = response.headers_mut();
    headers.remove(header::X_FRAME_OPTIONS);
    match HeaderValue::from_str(&policy) {
        Ok(csp) => {
            headers.insert(header::CONTENT_SECURITY_POLICY, csp);
        }
        Err(_) => return problem(StatusCode::BAD_GATEWAY, "the App's policy is no header"),
    }
    response
}

/// The base URL of App `name`'s shard and the client that reaches it, `None` when it has none:
/// the shard only from the App's `status.shard`, below the shards the host runs
/// (`JC_PORTAL_WASM_SHARDS`), and the namespace only from `JC_PORTAL_WASM_HOST_NAMESPACE`, so no
/// part of the host comes from the request.
fn wasm_server(state: &AppState, project: &str, name: &str) -> Option<(String, reqwest::Client)> {
    let settings = state.config.apps_db.as_ref()?;
    let namespace = settings
        .host_namespace
        .as_deref()
        .filter(|ns| crate::resource::is_dns1123(ns) && !ns.contains('.'))?;
    let shard = state
        .mirror
        .get(project, "App", name)?
        .status?
        .shard
        .filter(|shard| *shard < settings.shards)?;
    let host = format!("jc-wasm-host-{shard}.{namespace}.svc.cluster.local");
    let port = crate::reconciler::edge_file::WASM_HOST_PORT;
    let client = match settings.shards_through {
        None => http().clone(),
        Some(stub) => reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .proxy(reqwest::Proxy::http(format!("http://{stub}")).ok()?)
            .build()
            .ok()?,
    };
    Some((format!("http://{host}:{port}"), client))
}

/// Hands a call to `base` + `path` with the request's query and its answer back, both streamed.
/// The browser's cookies and the hop headers stay here both ways (`DROPPED`); the upstream sees
/// the bearer the call carries. A path with a dot segment is refused: the URL would resolve it
/// and leave the prefix the caller was checked for (another slug, another App on the shard).
async fn forward(
    req: Request,
    base: &str,
    path: &str,
    client: &reqwest::Client,
    upstream: &str,
) -> Response {
    let dotted = path.split('/').any(|segment| {
        let segment = segment.to_ascii_lowercase().replace("%2e", ".");
        segment == "." || segment == ".."
    });
    // A `\` is a `/` to the URL of an `http` address.
    if dotted || path.contains('\\') {
        return problem(StatusCode::NOT_FOUND, "no such path");
    }
    let query = req
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let url = format!("{}{path}{query}", base.trim_end_matches('/'));
    let (parts, body) = req.into_parts();
    let mut out = client.request(parts.method.clone(), url);
    for (key, value) in &parts.headers {
        if !DROPPED.contains(&key.as_str()) {
            out = out.header(key, value);
        }
    }
    let out =
        crate::request_id::forward(out).body(reqwest::Body::wrap_stream(body.into_data_stream()));
    let answer = match out.send().await {
        Ok(answer) => answer,
        Err(err) => {
            tracing::warn!(error = %err, upstream, "an App's call did not reach its upstream");
            return problem(
                StatusCode::BAD_GATEWAY,
                &format!("the {upstream} did not answer"),
            );
        }
    };
    // reqwest answers with the `http` types axum uses, so the upstream's headers are handed on
    // as they are, the hop-by-hop ones and its cookies dropped.
    let status = answer.status();
    let headers: HeaderMap = answer
        .headers()
        .iter()
        .filter(|(key, _)| !DROPPED.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let mut response = Response::new(Body::from_stream(answer.bytes_stream()));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

/// The App's own login on its host.
mod login {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    use axum::extract::Request;
    use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
    use axum::response::{IntoResponse, Response};
    use openidconnect::{CsrfToken, PkceCodeChallenge};
    use serde::Deserialize;

    use super::problem;
    use crate::auth::realms::app_oidc;
    use crate::auth::session::now_unix;
    use crate::config::OidcConfig;
    use crate::reconciler::app_clients::client_id;
    use crate::state::AppState;

    /// How long a login may take between the redirect and the callback.
    const LOGIN_SECS: i64 = 600;
    /// The longest a session lives, whatever the refresh token says.
    const SESSION_SECS: i64 = 12 * 3600;
    /// How long a client secret read from the realm is used before it is read again.
    const SECRET_SECS: i64 = 300;
    /// Sessions and logins held at most; the expired ones are dropped first.
    // ponytail: in memory, one jc replica (bins/jc/STATUS.md); a restart signs everyone in again
    // silently through the realm's own session. A second replica needs a shared store.
    const MAX_HELD: usize = 20_000;

    struct Session {
        app: String,
        access: String,
        refresh: Option<String>,
        access_until: i64,
        until: i64,
    }

    struct Pending {
        app: String,
        verifier: String,
        back: String,
        until: i64,
    }

    static SESSIONS: LazyLock<Mutex<HashMap<String, Session>>> = LazyLock::new(Default::default);
    static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Default::default);
    static SECRETS: LazyLock<Mutex<HashMap<String, (String, i64)>>> =
        LazyLock::new(Default::default);

    /// The session cookie of App `name`, as the edge named it.
    pub(super) fn cookie_name(name: &str) -> String {
        format!("__Host-jc_app_{}", name.replace('-', "_"))
    }

    fn login_cookie(name: &str) -> String {
        format!("__Host-jc_app_login_{}", name.replace('-', "_"))
    }

    fn cookie_of(headers: &HeaderMap, wanted: &str) -> Option<String> {
        headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(key, _)| *key == wanted)
            .map(|(_, value)| value.to_owned())
    }

    fn set_cookie(response: &mut Response, name: &str, value: &str, max_age: i64) {
        let cookie = format!(
            "{name}={value}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age={}",
            max_age.max(0)
        );
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }

    /// `with` applied to what `store` holds; `None` once a panic poisoned it.
    fn held<T, R>(store: &Mutex<T>, with: impl FnOnce(&mut T) -> R) -> Option<R> {
        store.lock().ok().map(|mut guard| with(&mut guard))
    }

    thread_local! {
        /// The logins and sessions held at most, as this thread's calls see it: MAX_HELD, lower
        /// only while a test fills the store.
        static HELD_AT_MOST: std::cell::Cell<usize> = const { std::cell::Cell::new(MAX_HELD) };
    }

    fn keep<T>(
        map: &mut HashMap<String, T>,
        until: impl Fn(&T) -> i64,
        key: String,
        value: T,
    ) -> bool {
        let now = now_unix();
        let most = HELD_AT_MOST.with(std::cell::Cell::get);
        if map.len() >= most {
            map.retain(|_, held| until(held) > now);
        }
        if map.len() >= most {
            return false;
        }
        map.insert(key, value);
        true
    }

    /// The App's origin, `https://{name}.apps.{apex}/`.
    fn origin(state: &AppState, name: &str) -> Option<url::Url> {
        crate::apps::static_host::app_origin(state.config.apps_url.as_ref()?, name)
    }

    /// `{issuer}/protocol/openid-connect/{page}`, with or without a slash after the realm.
    fn realm_page(oidc: &OidcConfig, page: &str) -> url::Url {
        let mut url = oidc.issuer.clone();
        let path = format!(
            "{}/protocol/openid-connect/{page}",
            oidc.issuer.path().trim_end_matches('/')
        );
        url.set_path(&path);
        url
    }

    fn redirect(location: &str) -> Response {
        match HeaderValue::from_str(location) {
            Ok(location) => (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response(),
            Err(_) => problem(StatusCode::BAD_GATEWAY, "no sign-in address"),
        }
    }

    /// `302`-like redirect to the realm with the App's client, PKCE and a state bound to this
    /// browser by a short cookie. `back` is a path on the App's host, never another origin.
    pub(super) fn start(state: &AppState, name: &str, back: &str) -> Response {
        let (Some(oidc), Some(origin)) = (app_oidc(state, name), origin(state, name)) else {
            return problem(StatusCode::SERVICE_UNAVAILABLE, "sign-in is not configured");
        };
        let back = if back.starts_with('/') && !back.starts_with("//") && !back.contains('\\') {
            back
        } else {
            "/"
        };
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let csrf = CsrfToken::new_random().secret().clone();
        let pending = Pending {
            app: name.to_owned(),
            verifier: verifier.secret().clone(),
            back: back.to_owned(),
            until: now_unix() + LOGIN_SECS,
        };
        let held = PENDING
            .lock()
            .map(|mut map| keep(&mut map, |p: &Pending| p.until, csrf.clone(), pending))
            .unwrap_or(false);
        if !held {
            return problem(StatusCode::SERVICE_UNAVAILABLE, "too many sign-ins at once");
        }
        let mut url = realm_page(&oidc, "auth");
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id(name))
            .append_pair("redirect_uri", &format!("{origin}callback"))
            .append_pair("scope", "openid")
            .append_pair("state", &csrf)
            .append_pair("code_challenge", challenge.as_str())
            .append_pair("code_challenge_method", "S256");
        let mut response = redirect(url.as_str());
        set_cookie(&mut response, &login_cookie(name), &csrf, LOGIN_SECS);
        response
    }

    #[derive(Deserialize)]
    struct Tokens {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
        #[serde(default)]
        expires_in: Option<i64>,
    }

    /// The App client's secret, read from the realm with the Portal's own admin client (the
    /// identity that writes the App clients, AP-111) and kept for a few minutes.
    async fn secret(oidc: &OidcConfig, name: &str) -> Result<String, String> {
        let now = now_unix();
        // Keyed by realm too: an App moved to its Organization's realm never reuses the old secret.
        let key = format!("{} {name}", oidc.issuer);
        if let Some((value, until)) = SECRETS.lock().ok().and_then(|map| map.get(&key).cloned()) {
            if until > now {
                return Ok(value);
            }
        }
        let admin = crate::reconciler::app_clients::Admin::new(
            &oidc.backchannel(),
            oidc.client_id.clone(),
            oidc.auth.clone(),
        )
        .ok_or("the issuer is not a realm URL")?;
        let token = admin.token().await?;
        let client = admin
            .find(&token, &client_id(name))
            .await?
            .ok_or_else(|| format!("the realm has no client {}", client_id(name)))?;
        let uuid = client["id"].as_str().ok_or("the client has no id")?;
        let value = admin
            .send(
                &token,
                reqwest::Method::GET,
                &format!("/clients/{uuid}/client-secret"),
                None,
            )
            .await?
            .and_then(|v| v["value"].as_str().map(str::to_owned))
            .filter(|v| !v.is_empty())
            .ok_or("the realm answered no secret for the client")?;
        held(&SECRETS, |map| {
            map.insert(key, (value.clone(), now + SECRET_SECS))
        });
        Ok(value)
    }

    /// One token request of App `name`'s client: the code, or a refresh.
    async fn tokens(
        state: &AppState,
        name: &str,
        grant: &[(&str, &str)],
    ) -> Result<Tokens, String> {
        let oidc = app_oidc(state, name).ok_or("no realm")?;
        let secret = secret(&oidc, name).await?;
        let client = client_id(name);
        let mut form: Vec<(&str, &str)> = grant.to_vec();
        form.push(("client_id", &client));
        form.push(("client_secret", &secret));
        let http = crate::auth::client_auth::realm_http(std::time::Duration::from_secs(10))
            .ok_or("no http client")?;
        let answer = http
            .post(format!(
                "{}/protocol/openid-connect/token",
                oidc.backchannel()
            ))
            .form(&form)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if !answer.status().is_success() {
            return Err(format!(
                "the realm refused the token request: {}",
                answer.status()
            ));
        }
        answer.json::<Tokens>().await.map_err(|err| err.to_string())
    }

    /// Whether `token` is an access token of App `name`'s own client in the App's realm (AP-92).
    async fn verified(state: &AppState, name: &str, token: &str) -> bool {
        crate::auth::realms::verify_app_token(state, name, token)
            .await
            .is_ok()
    }

    fn session_of(name: &str, tokens: Tokens, now: i64, until: i64) -> Session {
        Session {
            app: name.to_owned(),
            access: tokens.access_token,
            refresh: tokens.refresh_token,
            access_until: now + tokens.expires_in.unwrap_or(60).clamp(1, 3600) - 5,
            until,
        }
    }

    struct Callback {
        code: Option<String>,
        state: Option<String>,
    }

    /// `/callback`: the state must be this browser's (its login cookie) and this App's; the code
    /// is redeemed with the App's secret and the PKCE verifier, the token verified as the App's.
    pub(super) async fn callback(
        state: &AppState,
        headers: &HeaderMap,
        uri: &axum::http::Uri,
        name: &str,
    ) -> Response {
        let mut query = Callback {
            code: None,
            state: None,
        };
        for (key, value) in url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
        {
            match key.as_ref() {
                "code" => query.code = Some(value.into_owned()),
                "state" => query.state = Some(value.into_owned()),
                _ => {}
            }
        }
        let (Some(code), Some(csrf)) = (query.code, query.state) else {
            return problem(StatusCode::BAD_REQUEST, "the sign-in answer is incomplete");
        };
        let ours = cookie_of(headers, &login_cookie(name));
        let pending = PENDING.lock().ok().and_then(|mut map| map.remove(&csrf));
        let Some(pending) = pending.filter(|p| {
            p.app == name
                && p.until > now_unix()
                && ours
                    .as_deref()
                    .is_some_and(|ours| crate::auth::csrf::constant_time_eq(ours, &csrf))
        }) else {
            return problem(StatusCode::BAD_REQUEST, "this sign-in was not started here");
        };
        let Some(origin) = origin(state, name) else {
            return problem(StatusCode::NOT_FOUND, "no such App");
        };
        let redirect_uri = format!("{origin}callback");
        let grant = [
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("code_verifier", pending.verifier.as_str()),
        ];
        let tokens = match tokens(state, name, &grant).await {
            Ok(tokens) => tokens,
            Err(err) => {
                tracing::warn!(app = %name, error = %err, "an App sign-in failed at the realm");
                return problem(StatusCode::BAD_GATEWAY, "the sign-in did not complete");
            }
        };
        if !verified(state, name, &tokens.access_token).await {
            tracing::warn!(app = %name, "the realm answered a token that is not the App's own");
            return problem(StatusCode::BAD_GATEWAY, "the sign-in did not complete");
        }
        let now = now_unix();
        let id = CsrfToken::new_random().secret().clone();
        let held = SESSIONS
            .lock()
            .map(|mut map| {
                keep(
                    &mut map,
                    |s: &Session| s.until,
                    id.clone(),
                    session_of(name, tokens, now, now + SESSION_SECS),
                )
            })
            .unwrap_or(false);
        if !held {
            return problem(StatusCode::SERVICE_UNAVAILABLE, "too many sessions at once");
        }
        let mut response = redirect(&pending.back);
        set_cookie(&mut response, &cookie_name(name), &id, SESSION_SECS);
        set_cookie(&mut response, &login_cookie(name), "", 0);
        response
    }

    /// The access token of this browser's session on App `name`, renewed when it expired; `None`
    /// without one, or once the realm refuses the renewal (the session or a role ended).
    pub(super) async fn session(
        state: &AppState,
        headers: &HeaderMap,
        name: &str,
    ) -> Option<String> {
        let id = cookie_of(headers, &cookie_name(name))?;
        let now = now_unix();
        let refresh = {
            let map = SESSIONS.lock().ok()?;
            let held = map.get(&id).filter(|s| s.app == name && s.until > now)?;
            if held.access_until > now {
                return Some(held.access.clone());
            }
            held.refresh.clone()
        };
        let renewed = match refresh {
            Some(refresh) => tokens(
                state,
                name,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh.as_str()),
                ],
            )
            .await
            .ok(),
            None => None,
        };
        let mut map = SESSIONS.lock().ok()?;
        match renewed {
            Some(tokens) if !tokens.access_token.is_empty() => {
                let until = map.get(&id).map_or(now, |s| s.until);
                let session = session_of(name, tokens, now, until);
                let access = session.access.clone();
                map.insert(id, session);
                Some(access)
            }
            _ => {
                map.remove(&id);
                None
            }
        }
    }

    /// `/logout`: the session ends here and at the realm, and the browser lands on the App's start.
    pub(super) fn logout(state: &AppState, req: &Request, name: &str) -> Response {
        if let Some(id) = cookie_of(req.headers(), &cookie_name(name)) {
            held(&SESSIONS, |map| {
                if map.get(&id).is_some_and(|s| s.app == name) {
                    map.remove(&id);
                }
            });
        }
        let target = match (app_oidc(state, name), origin(state, name)) {
            (Some(oidc), Some(origin)) => {
                let mut url = realm_page(&oidc, "logout");
                url.query_pairs_mut()
                    .append_pair("client_id", &client_id(name))
                    .append_pair("post_logout_redirect_uri", origin.as_str());
                url.to_string()
            }
            _ => "/".to_owned(),
        };
        let mut response = redirect(&target);
        set_cookie(&mut response, &cookie_name(name), "", 0);
        response
    }

    #[cfg(test)]
    pub(super) fn insert_for_test(id: &str, app: &str, access: &str) {
        insert_expiring_for_test(id, app, access, None, 60);
    }

    /// A session whose access token is valid `access_secs` more (expired when not positive).
    #[cfg(test)]
    pub(super) fn insert_expiring_for_test(
        id: &str,
        app: &str,
        access: &str,
        refresh: Option<&str>,
        access_secs: i64,
    ) {
        let now = now_unix();
        SESSIONS.lock().unwrap().insert(
            id.to_owned(),
            Session {
                app: app.to_owned(),
                access: access.to_owned(),
                refresh: refresh.map(str::to_owned),
                access_until: now + access_secs,
                until: now + 600,
            },
        );
    }

    /// A client secret of App `name` in the realm `issuer`, kept until `until`.
    #[cfg(test)]
    pub(super) fn keep_secret_for_test(issuer: &str, name: &str, value: &str, until: i64) {
        SECRETS
            .lock()
            .unwrap()
            .insert(format!("{issuer} {name}"), (value.to_owned(), until));
    }

    #[cfg(test)]
    pub(super) fn held_for_test(id: &str) -> bool {
        SESSIONS.lock().unwrap().contains_key(id)
    }

    /// Holds at most `most` logins and sessions on this thread until the guard drops.
    #[cfg(test)]
    pub(super) fn held_at_most_for_test(most: usize) -> HeldAtMost {
        HELD_AT_MOST.with(|held| held.set(most));
        HeldAtMost
    }

    #[cfg(test)]
    pub(super) struct HeldAtMost;

    #[cfg(test)]
    impl Drop for HeldAtMost {
        fn drop(&mut self) {
            HELD_AT_MOST.with(|held| held.set(MAX_HELD));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn oidc(issuer: &str) -> OidcConfig {
            OidcConfig {
                issuer: issuer.parse().unwrap(),
                client_id: "portal".into(),
                auth: crate::auth::client_auth::ClientAuth::Secret("unused".into()),
                extra_ca_pem: None,
                admin_url: None,
            }
        }

        #[test]
        fn a_realm_page_is_under_the_realm_with_or_without_its_trailing_slash() {
            for issuer in [
                "https://idm.example.org/realms/city",
                "https://idm.example.org/realms/city/",
            ] {
                assert_eq!(
                    realm_page(&oidc(issuer), "logout").as_str(),
                    "https://idm.example.org/realms/city/protocol/openid-connect/logout"
                );
            }
        }

        /// MAX_HELD bounds the logins and sessions held: the expired go first, and a map full of
        /// live ones refuses the next instead of growing.
        #[test]
        fn a_full_map_drops_the_expired_first_and_refuses_when_all_are_live() {
            let now = now_unix();
            let until = |held: &i64| *held;
            let mut map: HashMap<String, i64> =
                (0..MAX_HELD).map(|i| (format!("k{i}"), now - 1)).collect();
            assert!(keep(&mut map, until, "new".into(), now + 60));
            assert_eq!(map.len(), 1, "the expired were dropped");
            let mut live: HashMap<String, i64> =
                (0..MAX_HELD).map(|i| (format!("k{i}"), now + 60)).collect();
            assert!(!keep(&mut live, until, "new".into(), now + 60));
            assert_eq!(live.len(), MAX_HELD);
            assert!(!live.contains_key("new"));
            let mut room: HashMap<String, i64> = HashMap::new();
            assert!(keep(&mut room, until, "a".into(), now));
        }

        #[test]
        fn a_location_no_header_can_carry_is_a_bad_gateway_and_a_bad_cookie_is_not_set() {
            assert_eq!(redirect("/a\nb").status(), StatusCode::BAD_GATEWAY);
            let mut response = redirect("/");
            set_cookie(&mut response, "c", "bad\nvalue", 60);
            assert!(response.headers().get(header::SET_COOKIE).is_none());
            set_cookie(&mut response, "c", "v", -5);
            assert_eq!(
                response.headers()[header::SET_COOKIE],
                "c=v; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0"
            );
        }

        /// The access token's lifetime is the realm's, one second to an hour, five seconds early.
        #[test]
        fn a_sessions_access_lifetime_is_bounded_and_defaults_to_a_minute() {
            let tokens = |expires_in| Tokens {
                access_token: "a".into(),
                refresh_token: None,
                expires_in,
            };
            assert_eq!(session_of("x", tokens(None), 1000, 2000).access_until, 1055);
            assert_eq!(
                session_of("x", tokens(Some(999_999)), 1000, 2000).access_until,
                4595
            );
            assert_eq!(
                session_of("x", tokens(Some(-3)), 1000, 2000).access_until,
                996
            );
            let session = session_of("x", tokens(Some(30)), 1000, 2000);
            assert_eq!((session.app.as_str(), session.until), ("x", 2000));
        }

        #[test]
        fn a_cookie_is_found_among_several_and_a_missing_one_is_none() {
            let mut headers = HeaderMap::new();
            headers.append(
                header::COOKIE,
                HeaderValue::from_static("a=1; __Host-jc_app_x=sid"),
            );
            headers.append(header::COOKIE, HeaderValue::from_static("b=2"));
            assert_eq!(
                cookie_of(&headers, "__Host-jc_app_x").as_deref(),
                Some("sid")
            );
            assert_eq!(cookie_of(&headers, "b").as_deref(), Some("2"));
            assert_eq!(cookie_of(&headers, "__Host-jc_app"), None);
            assert_eq!(cookie_name("my-app"), "__Host-jc_app_my_app");
            assert_eq!(login_cookie("my-app"), "__Host-jc_app_login_my_app");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
    use tower::ServiceExt;

    use crate::config::{Config, OidcConfig};
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use crate::state::AppState;
    use crate::store::Mirror;

    fn state() -> AppState {
        let mirror = Arc::new(Mirror::new());
        for (name, visibility) in [
            ("bikes", "project"),
            ("alerts", "roles"),
            ("open", "public"),
        ] {
            mirror.upsert(ResourceEnvelope {
                api_version: API_VERSION.into(),
                kind: "App".into(),
                metadata: ObjectMeta {
                    name: name.into(),
                    namespace: Some("helsinki".into()),
                    ..Default::default()
                },
                spec: serde_json::json!({
                    "kind": "ui", "source": { "path": "." }, "build": {},
                    "visibility": visibility, "lifecycle": "published", "dataNeeds": []
                }),
                status: None,
            });
        }
        let config = Config {
            public_base_url: "https://portal.example.org".parse().unwrap(),
            apps_url: Some("https://example.org".parse().unwrap()),
            serve_app_hosts: true,
            oidc: Some(OidcConfig {
                issuer: "https://idm.example.org/realms/city".parse().unwrap(),
                client_id: "portal".into(),
                auth: crate::auth::client_auth::ClientAuth::Secret("unused".into()),
                extra_ca_pem: None,
                admin_url: None,
            }),
            ..Config::for_tests()
        };
        AppState::new(config, None).with_mirror(mirror)
    }

    async fn get(
        state: &AppState,
        host: &str,
        uri: &str,
        extra: &[(&str, &str)],
    ) -> axum::response::Response {
        let mut req = Request::builder().uri(uri).header(header::HOST, host);
        for (key, value) in extra {
            req = req.header(*key, *value);
        }
        crate::server::app(state.clone())
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_non_public_apps_page_without_a_session_goes_to_the_realm_with_the_apps_own_client() {
        let state = state();
        for name in ["bikes", "alerts"] {
            let host = format!("{name}.apps.example.org");
            let response = get(&state, &host, "/stations?x=1", &[("accept", "text/html")]).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER, "{name}");
            let location =
                url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
            assert_eq!(
                location.as_str().split('?').next(),
                Some("https://idm.example.org/realms/city/protocol/openid-connect/auth")
            );
            let query: std::collections::HashMap<_, _> =
                location.query_pairs().into_owned().collect();
            assert_eq!(query["client_id"], format!("app-{name}"));
            assert_eq!(query["redirect_uri"], format!("https://{host}/callback"));
            assert_eq!(query["code_challenge_method"], "S256");
            assert_eq!(query["response_type"], "code");
            // The state is bound to this browser by a cookie host-only on the App's host (AP-29).
            let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
            let want = format!("__Host-jc_app_login_{name}={}; ", query["state"]);
            assert!(cookie.starts_with(&want), "{cookie}");
            for part in ["Path=/", "Secure", "HttpOnly", "SameSite=Lax"] {
                assert!(cookie.contains(part), "{cookie}");
            }
            assert!(!cookie.to_ascii_lowercase().contains("domain"), "{cookie}");
        }
        // A public App, a fetch and a call are never sent to sign in.
        for (host, uri, accept) in [
            ("open.apps.example.org", "/", "text/html"),
            ("bikes.apps.example.org", "/data.json", "application/json"),
            ("bikes.apps.example.org", "/api/services/jobs", "text/html"),
        ] {
            let response = get(&state, host, uri, &[("accept", accept)]).await;
            assert_ne!(response.status(), StatusCode::SEE_OTHER, "{host}{uri}");
        }
    }

    #[tokio::test]
    async fn a_callback_this_browser_did_not_start_for_this_app_is_refused() {
        let state = state();
        let started = get(
            &state,
            "bikes.apps.example.org",
            "/",
            &[("accept", "text/html")],
        )
        .await;
        let location =
            url::Url::parse(started.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let csrf = location
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        let callback = format!("/callback?code=c&state={csrf}");
        // Without the login cookie (another browser), and on another App's host with that App's
        // cookie name: refused before any token request.
        let response = get(&state, "bikes.apps.example.org", &callback, &[]).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let cookie = format!("__Host-jc_app_login_alerts={csrf}");
        let response = get(
            &state,
            "alerts.apps.example.org",
            &callback,
            &[("cookie", &cookie)],
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_session_opens_its_own_app_alone() {
        let state = state();
        super::login::insert_for_test("sid-of-bikes", "bikes", "bikes-token");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-jc_app_bikes=sid-of-bikes"),
        );
        assert_eq!(
            super::login::session(&state, &headers, "bikes")
                .await
                .as_deref(),
            Some("bikes-token")
        );
        // The same id under another App's cookie name (a cookie copied by hand): not a session.
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-jc_app_alerts=sid-of-bikes"),
        );
        assert_eq!(
            super::login::session(&state, &headers, "alerts").await,
            None
        );
        // And on another App's host the bikes cookie is not that App's cookie at all.
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-jc_app_bikes=sid-of-bikes"),
        );
        assert_eq!(
            super::login::session(&state, &headers, "alerts").await,
            None
        );
        let response = get(
            &state,
            "alerts.apps.example.org",
            "/",
            &[
                ("accept", "text/html"),
                ("cookie", "__Host-jc_app_alerts=sid-of-bikes"),
            ],
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::SEE_OTHER,
            "still signed out on alerts"
        );
    }

    /// An App's session is the bearer of every call it makes: to its shard and to its own
    /// endpoints through the gateway, the browser's cookie never. (A public App: the shard's
    /// visibility check verifies the token, which a test session's is not.)
    #[tokio::test]
    async fn a_session_is_the_bearer_its_apps_shard_and_endpoints_see() {
        type Seen = Arc<std::sync::Mutex<Vec<(String, HeaderMap)>>>;
        let seen: Seen = Arc::default();
        let record = seen.clone();
        let stub = axum::Router::new().fallback(move |req: Request<Body>| {
            let record = record.clone();
            async move {
                let seen = (req.uri().to_string(), req.headers().clone());
                record.lock().unwrap().push(seen);
                "{}"
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, stub).await });

        let state = state();
        let spec = serde_json::json!({
            "kind": "wasm", "source": { "path": "." }, "build": {},
            "visibility": "public", "lifecycle": "published",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
                "types": ["AirQualityObserved"], "operations": ["queryEntity"]
            }]
        });
        let mut app = ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "App".into(),
            metadata: ObjectMeta {
                name: "counts".into(),
                namespace: Some("helsinki".into()),
                ..Default::default()
            },
            spec,
            status: None,
        };
        app.status = Some(serde_json::from_value(serde_json::json!({ "shard": 0 })).unwrap());
        state.mirror.upsert(app);
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "Endpoint".into(),
            metadata: ObjectMeta {
                name: "air".into(),
                namespace: Some("helsinki".into()),
                ..Default::default()
            },
            spec: serde_json::json!({ "contextSpaceRef": "air", "slug": "airslug" }),
            status: None,
        });
        let mut config = (*state.config).clone();
        config.gateway_url = Some(format!("http://{addr}"));
        config.apps_db = Some(crate::config::AppsDbSettings {
            url: "postgres://apps@db/apps".into(),
            shards: 1,
            bucket: "apps".into(),
            host_namespace: Some("wasm-host".into()),
            shards_through: Some(addr),
        });
        let state = AppState::new(config, None).with_mirror(state.mirror.clone());
        super::login::insert_for_test("sid-of-counts", "counts", "counts-token");
        let cookie = ("cookie", "__Host-jc_app_counts=sid-of-counts");
        for uri in ["/api/totals", "/api/endpoint/airslug/ngsi-ld/v1/entities"] {
            let response = get(&state, "counts.apps.example.org", uri, &[cookie]).await;
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
        }
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2);
        assert_eq!(
            seen[0].0,
            "http://jc-wasm-host-0.wasm-host.svc.cluster.local:8080/apps/counts/api/totals"
        );
        assert_eq!(seen[1].0, "/api/endpoint/airslug/ngsi-ld/v1/entities");
        for (_, headers) in &seen {
            assert_eq!(headers[header::AUTHORIZATION], "Bearer counts-token");
            assert!(headers.get(header::COOKIE).is_none());
        }
    }

    /// A realm on wiremock that knows App `bikes`' client and its secret, and a verifier of its
    /// tokens; the state's sign-in goes there.
    struct Realm {
        server: wiremock::MockServer,
        state: AppState,
        sign: Box<dyn Fn(&serde_json::Value) -> String + Send + Sync>,
        issuer: String,
        /// Of this realm alone: the client secret is kept per issuer, and a port of a dropped
        /// server is handed out again.
        realm: String,
    }

    impl Realm {
        async fn start() -> Self {
            static REALMS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let server = wiremock::MockServer::start().await;
            let realm = format!(
                "city{}",
                REALMS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let issuer = format!("{}/realms/{realm}", server.uri());
            let (verifier, sign) = crate::auth::bearer::test_verifier(&issuer, "app-bikes", "k1");
            let base = state();
            let mut config = (*base.config).clone();
            config.oidc = Some(OidcConfig {
                issuer: issuer.parse().unwrap(),
                client_id: "portal".into(),
                auth: crate::auth::client_auth::ClientAuth::Secret("portal-secret".into()),
                extra_ca_pem: None,
                admin_url: None,
            });
            let mut state = AppState::new(config, None).with_mirror(base.mirror.clone());
            state.bearer = Some(Arc::new(verifier));
            Self {
                server,
                state,
                sign: Box::new(sign),
                issuer,
                realm,
            }
        }

        fn token(&self, azp: &str) -> String {
            let now = crate::auth::session::now_unix();
            (self.sign)(&serde_json::json!({
                "iss": self.issuer, "sub": "ana", "aud": "app-bikes", "azp": azp,
                "typ": "Bearer", "iat": now, "exp": now + 300,
            }))
        }

        /// The admin token, App `bikes`' client `u1` and its `secret`, each asked for `times`.
        async fn knows_the_client(&self, secret: serde_json::Value, times: u64) {
            use wiremock::matchers::{body_string_contains, method, path, query_param};
            use wiremock::{Mock, ResponseTemplate};
            Mock::given(method("POST"))
                .and(path(format!(
                    "/realms/{}/protocol/openid-connect/token",
                    self.realm
                )))
                .and(body_string_contains("grant_type=client_credentials"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({ "access_token": "admin" })),
                )
                .expect(times)
                .mount(&self.server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/admin/realms/{}/clients", self.realm)))
                .and(query_param("clientId", "app-bikes"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(serde_json::json!([{ "id": "u1" }])),
                )
                .expect(times)
                .mount(&self.server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!(
                    "/admin/realms/{}/clients/u1/client-secret",
                    self.realm
                )))
                .respond_with(ResponseTemplate::new(200).set_body_json(secret))
                .expect(times)
                .mount(&self.server)
                .await;
        }

        /// The token endpoint's answer to a `grant` of App `bikes`' client with secret `s3`.
        async fn answers(&self, grant: &str, status: u16, body: serde_json::Value) {
            use wiremock::matchers::{body_string_contains, method, path};
            use wiremock::{Mock, ResponseTemplate};
            Mock::given(method("POST"))
                .and(path(format!(
                    "/realms/{}/protocol/openid-connect/token",
                    self.realm
                )))
                .and(body_string_contains(format!("grant_type={grant}")))
                .and(body_string_contains("client_id=app-bikes"))
                .and(body_string_contains("client_secret=s3"))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .mount(&self.server)
                .await;
        }

        /// A sign-in started on `bikes` for `back`: its state and its login cookie.
        async fn started(&self, back: &str) -> (String, String) {
            let response = get(
                &self.state,
                "bikes.apps.example.org",
                back,
                &[("accept", "text/html")],
            )
            .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let location =
                url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
            let csrf = location
                .query_pairs()
                .find(|(k, _)| k == "state")
                .unwrap()
                .1
                .into_owned();
            (csrf.clone(), format!("__Host-jc_app_login_bikes={csrf}"))
        }
    }

    fn set_cookies(response: &axum::response::Response) -> Vec<String> {
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_owned())
            .collect()
    }

    /// AP-29, AP-92: the code is redeemed with the App's own secret read from the realm and the
    /// PKCE verifier; the browser goes back where it started with a session cookie, the login
    /// cookie cleared, and that session is the App's bearer. A second sign-in reuses the secret.
    #[tokio::test]
    async fn a_completed_sign_in_opens_a_session_on_the_apps_host_and_the_secret_is_kept() {
        let realm = Realm::start().await;
        realm
            .knows_the_client(serde_json::json!({ "value": "s3" }), 1)
            .await;
        let token = realm.token("app-bikes");
        realm
            .answers(
                "authorization_code",
                200,
                serde_json::json!({ "access_token": token, "refresh_token": "r1", "expires_in": 60 }),
            )
            .await;
        for _ in 0..2 {
            let (csrf, cookie) = realm.started("/stations?x=1").await;
            let callback = format!("/callback?code=the-code&state={csrf}&session_state=s");
            let response = get(
                &realm.state,
                "bikes.apps.example.org",
                &callback,
                &[("cookie", &cookie)],
            )
            .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert_eq!(response.headers()[header::LOCATION], "/stations?x=1");
            let cookies = set_cookies(&response);
            let session = cookies
                .iter()
                .find(|c| c.starts_with("__Host-jc_app_bikes="))
                .expect("a session cookie");
            assert!(session.contains("Max-Age=43200"), "{session}");
            assert!(
                cookies.iter().any(
                    |c| c.starts_with("__Host-jc_app_login_bikes=;") && c.contains("Max-Age=0")
                ),
                "{cookies:?}"
            );
            let sid = session
                .trim_start_matches("__Host-jc_app_bikes=")
                .split(';')
                .next()
                .unwrap()
                .to_owned();
            let mut headers = HeaderMap::new();
            headers.insert(
                header::COOKIE,
                HeaderValue::from_str(&format!("__Host-jc_app_bikes={sid}")).unwrap(),
            );
            assert_eq!(
                super::login::session(&realm.state, &headers, "bikes")
                    .await
                    .as_deref(),
                Some(token.as_str())
            );
            // The state is spent: the same answer again is not a sign-in started here.
            let again = get(
                &realm.state,
                "bikes.apps.example.org",
                &callback,
                &[("cookie", &cookie)],
            )
            .await;
            assert_eq!(again.status(), StatusCode::BAD_REQUEST);
        }
        let requests = realm.server.received_requests().await.unwrap();
        let redeemed = requests
            .iter()
            .find(|r| String::from_utf8_lossy(&r.body).contains("authorization_code"))
            .expect("the code was redeemed");
        let form: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(&redeemed.body)
                .into_owned()
                .collect();
        assert_eq!(form["code"], "the-code");
        assert_eq!(
            form["redirect_uri"],
            "https://bikes.apps.example.org/callback"
        );
        assert!(form["code_verifier"].len() >= 43, "a PKCE verifier");
    }

    /// The realm refusing the code, a token another client obtained, and an App client the realm
    /// does not know or keeps no secret for each end the sign-in with a 502, no session.
    #[tokio::test]
    async fn a_sign_in_the_realm_does_not_complete_is_a_bad_gateway_without_a_session() {
        for case in [
            "refused",
            "foreign",
            "no-client",
            "no-id",
            "no-secret",
            "admin-refused",
        ] {
            let realm = Realm::start().await;
            match case {
                "no-client" | "no-id" | "admin-refused" => {
                    use wiremock::matchers::{body_string_contains, method, path};
                    use wiremock::{Mock, ResponseTemplate};
                    let status = if case == "admin-refused" { 401 } else { 200 };
                    Mock::given(method("POST"))
                        .and(path(format!(
                            "/realms/{}/protocol/openid-connect/token",
                            realm.realm
                        )))
                        .and(body_string_contains("grant_type=client_credentials"))
                        .respond_with(
                            ResponseTemplate::new(status)
                                .set_body_json(serde_json::json!({ "access_token": "admin" })),
                        )
                        .mount(&realm.server)
                        .await;
                    let found = if case == "no-id" {
                        serde_json::json!([{ "clientId": "app-bikes" }])
                    } else {
                        serde_json::json!([])
                    };
                    Mock::given(method("GET"))
                        .and(path(format!("/admin/realms/{}/clients", realm.realm)))
                        .respond_with(ResponseTemplate::new(200).set_body_json(found))
                        .mount(&realm.server)
                        .await;
                }
                "no-secret" => {
                    realm
                        .knows_the_client(serde_json::json!({ "value": "" }), 1)
                        .await
                }
                _ => {
                    realm
                        .knows_the_client(serde_json::json!({ "value": "s3" }), 1)
                        .await
                }
            }
            match case {
                "refused" => {
                    realm
                        .answers(
                            "authorization_code",
                            400,
                            serde_json::json!({ "error": "invalid_grant" }),
                        )
                        .await
                }
                "foreign" => {
                    let token = realm.token("another-client");
                    realm
                        .answers(
                            "authorization_code",
                            200,
                            serde_json::json!({ "access_token": token }),
                        )
                        .await
                }
                _ => {}
            }
            let (csrf, cookie) = realm.started("/").await;
            let response = get(
                &realm.state,
                "bikes.apps.example.org",
                &format!("/callback?code=c&state={csrf}"),
                &[("cookie", &cookie)],
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY, "{case}");
            assert!(
                !set_cookies(&response)
                    .join("; ")
                    .contains("__Host-jc_app_bikes="),
                "{case}"
            );
        }
    }

    /// A token answer that is no token JSON is a failed sign-in too.
    #[tokio::test]
    async fn a_token_answer_without_an_access_token_is_a_bad_gateway() {
        let realm = Realm::start().await;
        realm
            .knows_the_client(serde_json::json!({ "value": "s3" }), 1)
            .await;
        realm
            .answers(
                "authorization_code",
                200,
                serde_json::json!({ "id_token": "x" }),
            )
            .await;
        let (csrf, cookie) = realm.started("/").await;
        let response = get(
            &realm.state,
            "bikes.apps.example.org",
            &format!("/callback?code=c&state={csrf}"),
            &[("cookie", &cookie)],
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn a_callback_without_its_code_or_state_is_a_bad_request() {
        let state = state();
        for uri in [
            "/callback",
            "/callback?code=c",
            "/callback?state=s",
            "/apps/bikes/callback?error=x",
        ] {
            let response = get(&state, "bikes.apps.example.org", uri, &[]).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{uri}");
        }
    }

    /// The minute-long access token is renewed by the refresh token; a refusal of the renewal,
    /// an answer without a token, or a session without a refresh token ends the session.
    #[tokio::test]
    async fn an_expired_access_token_is_renewed_or_the_session_ends() {
        let realm = Realm::start().await;
        realm
            .knows_the_client(serde_json::json!({ "value": "s3" }), 1)
            .await;
        {
            use wiremock::matchers::{body_string_contains, method, path};
            use wiremock::{Mock, ResponseTemplate};
            for (refresh, status, body) in [
                (
                    "good",
                    200,
                    serde_json::json!({ "access_token": "renewed", "refresh_token": "good", "expires_in": 60 }),
                ),
                ("empty", 200, serde_json::json!({ "access_token": "" })),
                (
                    "revoked",
                    400,
                    serde_json::json!({ "error": "invalid_grant" }),
                ),
            ] {
                Mock::given(method("POST"))
                    .and(path(format!(
                        "/realms/{}/protocol/openid-connect/token",
                        realm.realm
                    )))
                    .and(body_string_contains("grant_type=refresh_token"))
                    .and(body_string_contains(format!("refresh_token={refresh}&")))
                    .respond_with(ResponseTemplate::new(status).set_body_json(body))
                    .mount(&realm.server)
                    .await;
            }
        }
        let cookie = |sid: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::COOKIE,
                HeaderValue::from_str(&format!("__Host-jc_app_bikes={sid}")).unwrap(),
            );
            headers
        };
        super::login::insert_expiring_for_test("sid-renew", "bikes", "old", Some("good"), -1);
        assert_eq!(
            super::login::session(&realm.state, &cookie("sid-renew"), "bikes")
                .await
                .as_deref(),
            Some("renewed")
        );
        // Renewed: the next call needs no realm.
        assert_eq!(
            super::login::session(&realm.state, &cookie("sid-renew"), "bikes")
                .await
                .as_deref(),
            Some("renewed")
        );
        for (sid, refresh) in [
            ("sid-empty", Some("empty")),
            ("sid-revoked", Some("revoked")),
            ("sid-none", None),
        ] {
            super::login::insert_expiring_for_test(sid, "bikes", "old", refresh, 0);
            assert_eq!(
                super::login::session(&realm.state, &cookie(sid), "bikes").await,
                None,
                "{sid}"
            );
            assert!(!super::login::held_for_test(sid), "{sid} ended");
        }
        // No session cookie, an unknown id: no session, nothing asked.
        assert_eq!(
            super::login::session(&realm.state, &HeaderMap::new(), "bikes").await,
            None
        );
        assert_eq!(
            super::login::session(&realm.state, &cookie("sid-unknown"), "bikes").await,
            None
        );
    }

    /// Without a realm a renewal cannot be asked for: the session ends.
    #[tokio::test]
    async fn a_renewal_without_a_realm_ends_the_session() {
        let base = state();
        let mut config = (*base.config).clone();
        config.oidc = None;
        let state = AppState::new(config, None).with_mirror(base.mirror.clone());
        super::login::insert_expiring_for_test("sid-no-realm", "bikes", "old", Some("r"), -1);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-jc_app_bikes=sid-no-realm"),
        );
        assert_eq!(super::login::session(&state, &headers, "bikes").await, None);
        assert!(!super::login::held_for_test("sid-no-realm"));
        // And a sign-in cannot start: 503, not a redirect to nowhere.
        let response = get(&state, "bikes.apps.example.org", "/login", &[]).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// A store full of live logins refuses the next sign-in, and one full of live sessions the
    /// session the realm just answered for: each a 503 with no cookie, the live session kept.
    #[tokio::test]
    async fn a_full_store_refuses_the_next_sign_in_and_the_next_session() {
        let realm = Realm::start().await;
        realm
            .knows_the_client(serde_json::json!({ "value": "s3" }), 1)
            .await;
        let token = realm.token("app-bikes");
        realm
            .answers(
                "authorization_code",
                200,
                serde_json::json!({ "access_token": token, "expires_in": 60 }),
            )
            .await;
        let (csrf, cookie) = realm.started("/stations").await;
        super::login::insert_for_test("sid-full-store", "bikes", "t");
        let full = super::login::held_at_most_for_test(0);
        let refused = get(&realm.state, "bikes.apps.example.org", "/login", &[]).await;
        assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(set_cookies(&refused).is_empty());
        let callback = format!("/callback?code=the-code&state={csrf}");
        let response = get(
            &realm.state,
            "bikes.apps.example.org",
            &callback,
            &[("cookie", &cookie)],
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(set_cookies(&response).is_empty());
        drop(full);
        assert!(
            super::login::held_for_test("sid-full-store"),
            "a live session is never dropped to make room"
        );
        // With room again the next sign-in starts.
        let (_, again) = realm.started("/").await;
        assert!(again.starts_with("__Host-jc_app_login_bikes="));
    }

    /// `/logout` ends this App's session alone, here and at the realm, and lands on the App.
    #[tokio::test]
    async fn a_logout_ends_this_apps_session_here_and_at_the_realm() {
        let state = state();
        super::login::insert_for_test("sid-out-bikes", "bikes", "t");
        super::login::insert_for_test("sid-out-alerts", "alerts", "t");
        let response = get(
            &state,
            "bikes.apps.example.org",
            "/logout",
            &[("cookie", "__Host-jc_app_bikes=sid-out-bikes")],
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location =
            url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        assert_eq!(
            location.as_str().split('?').next(),
            Some("https://idm.example.org/realms/city/protocol/openid-connect/logout")
        );
        let query: std::collections::HashMap<_, _> = location.query_pairs().into_owned().collect();
        assert_eq!(query["client_id"], "app-bikes");
        assert_eq!(
            query["post_logout_redirect_uri"],
            "https://bikes.apps.example.org/"
        );
        assert!(set_cookies(&response)
            .iter()
            .any(|c| c.starts_with("__Host-jc_app_bikes=;") && c.contains("Max-Age=0")));
        assert!(!super::login::held_for_test("sid-out-bikes"));
        // Another App's session id under this App's cookie name is not this App's to end.
        let _ = get(
            &state,
            "bikes.apps.example.org",
            "/logout",
            &[("cookie", "__Host-jc_app_bikes=sid-out-alerts")],
        )
        .await;
        assert!(super::login::held_for_test("sid-out-alerts"));
        // No realm configured: the browser lands on the App's start.
        let mut config = (*state.config).clone();
        config.oidc = None;
        let bare = AppState::new(config, None).with_mirror(state.mirror.clone());
        let response = get(&bare, "bikes.apps.example.org", "/logout", &[]).await;
        assert_eq!(response.headers()[header::LOCATION], "/");
    }

    /// `back` is a path on the App's host: another origin, a protocol-relative or a backslash
    /// path starts the App at `/` instead (an open redirect otherwise).
    #[tokio::test]
    async fn a_sign_in_never_returns_to_another_origin() {
        let realm = Realm::start().await;
        realm
            .knows_the_client(serde_json::json!({ "value": "s3" }), 1)
            .await;
        let token = realm.token("app-bikes");
        realm
            .answers(
                "authorization_code",
                200,
                serde_json::json!({ "access_token": token }),
            )
            .await;
        for (back, want) in [
            ("https://evil.example", "/"),
            ("//evil.example/x", "/"),
            ("/\\evil.example", "/"),
            ("/ok/path", "/ok/path"),
        ] {
            let response = super::login::start(&realm.state, "bikes", back);
            let location =
                url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
            let csrf = location
                .query_pairs()
                .find(|(k, _)| k == "state")
                .unwrap()
                .1
                .into_owned();
            let cookie = format!("__Host-jc_app_login_bikes={csrf}");
            let response = get(
                &realm.state,
                "bikes.apps.example.org",
                &format!("/callback?code=c&state={csrf}"),
                &[("cookie", &cookie)],
            )
            .await;
            assert_eq!(response.headers()[header::LOCATION], want, "{back}");
        }
    }

    /// An App host without an apps origin has no sign-in to start.
    #[tokio::test]
    async fn a_sign_in_without_the_apps_origin_is_unavailable() {
        let base = state();
        let mut config = (*base.config).clone();
        config.apps_url = None;
        let state = AppState::new(config, None).with_mirror(base.mirror.clone());
        let response = super::login::start(&state, "bikes", "/");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// A sign-in whose kept secret expired reads the secret again; a realm that stops answering
    /// at the code's redemption, or an App host gone between the start and the answer, ends it.
    #[tokio::test]
    async fn an_expired_secret_is_read_again_and_a_realm_gone_mid_sign_in_is_a_bad_gateway() {
        let realm = Realm::start().await;
        realm
            .knows_the_client(serde_json::json!({ "value": "s3" }), 1)
            .await;
        let token = realm.token("app-bikes");
        realm
            .answers(
                "authorization_code",
                200,
                serde_json::json!({ "access_token": token }),
            )
            .await;
        let now = crate::auth::session::now_unix();
        super::login::keep_secret_for_test(&realm.issuer, "bikes", "stale", now - 1);
        let (csrf, cookie) = realm.started("/apps/bikes/stations").await;
        let response = get(
            &realm.state,
            "bikes.apps.example.org",
            &format!("/callback?code=c&state={csrf}"),
            &[("cookie", &cookie)],
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/stations",
            "the old /apps/{{name}} prefix is no path on the App's host"
        );

        // The realm is gone when the code is redeemed (the secret is still kept).
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = held.local_addr().unwrap();
        drop(held);
        let issuer = format!("http://{addr}/realms/gone");
        let mut config = (*realm.state.config).clone();
        config.oidc = Some(OidcConfig {
            issuer: issuer.parse().unwrap(),
            client_id: "portal".into(),
            auth: crate::auth::client_auth::ClientAuth::Secret("s".into()),
            extra_ca_pem: None,
            admin_url: None,
        });
        let gone = AppState::new(config, None).with_mirror(realm.state.mirror.clone());
        super::login::keep_secret_for_test(&issuer, "bikes", "s3", now + 600);
        let started = super::login::start(&gone, "bikes", "/");
        let location =
            url::Url::parse(started.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        let csrf = location
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        let cookie = format!("__Host-jc_app_login_bikes={csrf}");
        let response = get(
            &gone,
            "bikes.apps.example.org",
            &format!("/callback?code=c&state={csrf}"),
            &[("cookie", &cookie)],
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

        // Started, and the apps origin is gone before the answer: no such App.
        let (csrf, cookie) = realm.started("/").await;
        let mut config = (*realm.state.config).clone();
        config.apps_url = None;
        let unhosted = AppState::new(config, None).with_mirror(realm.state.mirror.clone());
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_str(&cookie).unwrap());
        let uri: axum::http::Uri = format!("/callback?code=c&state={csrf}").parse().unwrap();
        let response = super::login::callback(&unhosted, &headers, &uri, "bikes").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// The host the organization layer resolved names the App, whatever `Host` says; an apps
    /// origin without a host names none.
    #[tokio::test]
    async fn the_resolved_request_host_names_the_app_and_a_hostless_apex_names_none() {
        let state = state();
        let mut req = Request::builder()
            .uri("/stations")
            .header(header::HOST, "portal.example.org")
            .header("accept", "text/html")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(crate::organization_host::RequestHost(
                "bikes.apps.example.org".into(),
            ));
        let response = crate::server::app(state.clone())
            .oneshot(req)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "bikes signs in");
        let mut config = (*state.config).clone();
        config.apps_url = Some("file:///srv/apps".parse().unwrap());
        let hostless = AppState::new(config, None).with_mirror(state.mirror.clone());
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            HeaderValue::from_static("bikes.apps.example.org"),
        );
        assert_eq!(super::app_of_host(&hostless, &headers, None), None);
    }

    /// `/apps/{name}` alone is the App's start, a query is kept on a file, and a session token no
    /// header can carry is not handed on as a bearer.
    #[tokio::test]
    async fn the_apps_old_prefix_alone_is_its_start_and_a_query_is_kept() {
        let state = state();
        for uri in ["/apps/open", "/data.json?x=1"] {
            let response = get(&state, "open.apps.example.org", uri, &[]).await;
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{uri}: no bundle here"
            );
        }
        super::login::insert_for_test("sid-bad-token", "open", "bad\ntoken");
        let response = get(
            &state,
            "open.apps.example.org",
            "/",
            &[("cookie", "__Host-jc_app_open=sid-bad-token")],
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// An App's own endpoint without a gateway configured is a 503 naming it, never a raw error.
    #[tokio::test]
    async fn an_endpoint_call_without_a_gateway_is_unavailable() {
        let state = state();
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "App".into(),
            metadata: ObjectMeta {
                name: "air".into(),
                namespace: Some("helsinki".into()),
                ..Default::default()
            },
            spec: serde_json::json!({
                "kind": "ui", "source": { "path": "." }, "build": {},
                "visibility": "public", "lifecycle": "published",
                "dataNeeds": [{
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
                    "types": ["AirQualityObserved"], "operations": ["queryEntity"]
                }]
            }),
            status: None,
        });
        state.mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "Endpoint".into(),
            metadata: ObjectMeta {
                name: "air".into(),
                namespace: Some("helsinki".into()),
                ..Default::default()
            },
            spec: serde_json::json!({ "contextSpaceRef": "air", "slug": "airslug" }),
            status: None,
        });
        let response = get(
            &state,
            "air.apps.example.org",
            "/api/endpoint/airslug/ngsi-ld/v1/entities",
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
