use axum::extract::{Request, State};
use axum::http::{header, HeaderName, HeaderValue};
use axum::middleware::{from_fn, from_fn_with_state, Next};
use axum::response::Response;
use axum::Router;
use tower_http::compression::CompressionLayer;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::api;
use crate::apps;
use crate::assets;
use crate::auth;
use crate::config::Config;
use crate::openapi;
use crate::state::AppState;
use crate::telemetry;

pub fn app(state: AppState) -> Router {
    // The recorder belongs to the surface rather than to `main`: without it every `metrics::`
    // call in the process is a no-op, and a Portal that counted nothing would look exactly
    // like one nobody used (OPS-16).
    telemetry::install();

    // `frame-src` names the apps origin and each App's own host under it, `{name}.apps.{apex}`
    // (AP-133): the Open page shows an App under the Portal's header (AP-122, T-2871). Without
    // it `default-src` refuses the frame before the App's own `frame-ancestors` is ever read.
    // It names the realm's origin too: an App host signs its visitor in with a redirect through
    // the realm, and a frame refused that hop stays blank until the App's host was opened in a
    // tab of its own (T-2911). The realm's pages still refuse the frame by their own headers.
    let frame_src = match state.config.apps_url.as_ref() {
        Some(apps) => match apps.host_str() {
            Some(host) => {
                let mut sources = format!(
                    "'self' {} {}://*.apps.{host}",
                    apps.origin().ascii_serialization(),
                    apps.scheme()
                );
                if let Some(oidc) = state.config.oidc.as_ref() {
                    sources.push(' ');
                    sources.push_str(&oidc.issuer.origin().ascii_serialization());
                }
                sources
            }
            None => "'self'".to_owned(),
        },
        None => "'self'".to_owned(),
    };
    // `style-src` takes a `<style>` element only with the answer's nonce (crate::csp, T-5828) or,
    // for the public docs pages, the hash of their one stylesheet. Style *attributes* stay
    // allowed (`style-src-attr`): Monaco writes `style="top:..."` through innerHTML and no nonce
    // or hash can cover an attribute; an attribute holds no selector, so it cannot read the page.
    let content_security_policy: std::sync::Arc<str> = format!(
        "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'self'; \
             frame-src {frame_src}; form-action 'self'; img-src 'self' data: blob:; \
             style-src 'self' 'nonce-{slot}' {docs_style}; style-src-attr 'unsafe-inline'; \
             font-src 'self' data:; connect-src {connect_src}; worker-src 'self' blob:",
        slot = crate::csp::NONCE_SLOT,
        docs_style = crate::public_docs::style_source(),
        connect_src = connect_src(&state.config)
    )
    .into();

    // A static app carries its own Content Security Policy and its own framing rule, built from
    // its manifest (AP-12); the Portal's would override them, so those two headers are set on
    // the Portal's own routes only. Everything below them applies to both.
    let portal = Router::new()
        .nest("/api/v1", api::router())
        .merge(openapi::router())
        // The API and User Guide pages, for a signed-in person only (T-6200).
        .merge(crate::public_docs::router())
        .route("/", axum::routing::get(assets::home))
        .fallback(assets::static_handler)
        // Every portal request, API or page, keeps the cookie session alive against the realm;
        // outermost of the portal's own layers so a rotated cookie reaches every response.
        .layer(from_fn_with_state(state.clone(), auth::refresh::middleware))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("SAMEORIGIN"),
        ))
        .layer(from_fn_with_state(
            content_security_policy,
            crate::csp::middleware,
        ))
        // Outside the two framing headers, so it is the one place that may relax them, and only
        // for a public form's page whose Endpoint names the sites (EP-101).
        .layer(from_fn_with_state(state.clone(), public_form_framing));

    Router::new()
        .merge(apps::static_host::router())
        // The App templates' demos (AP-141): their own CSP, which forbids every connection.
        .merge(apps::template_demos::router())
        // The kit preview is an application document too: its own CSP, outside the Portal's.
        .merge(api::agent_runs::preview_router())
        // The embed dialog's launcher preview: its own CSP names the assistant host (T-5328).
        .merge(api::assistant_preview::router())
        // OPS-16: what `components/monitoring` scrapes. Outside the Portal's own security
        // headers and outside `/api/v1`, so no session or CSRF guard stands in front of a
        // scrape; the edge refuses the path, which is what keeps it inside the cluster.
        .merge(telemetry::router())
        // Portal MCP server and RFC 9728 protected resource metadata (AG-60, AG-32).
        // Stateless Streamable HTTP with audience-bound Bearer authentication; exempt from
        // the session refresh and CSRF layers.
        .merge(crate::mcp::router())
        .merge(crate::mcp::connector::router())
        // The gateway's delivery of live-update notifications (API/01 §32): outside `/api/v1`, so
        // no session or CSRF guard stands in front of a broker that holds no token; the key in
        // the path is the credential. The body limit is the route's own, with room for the check.
        .route(
            "/live-notify/{key}",
            axum::routing::post(api::live::notify).layer(axum::extract::DefaultBodyLimit::max(
                api::live::MAX_NOTIFICATION + 1,
            )),
        )
        .merge(portal)
        .with_state(state)
        .layer(from_fn(telemetry::record))
        // Outermost of the logging layers, so every line of the request carries its id.
        .layer(from_fn(crate::request_id::middleware))
        .layer(from_fn(api_cache_control_middleware))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("geolocation=(self), camera=(), microphone=()"),
        ))
        // gzip for a client that asks (T-5598): a space's grid read 480 KB of JSON uncompressed,
        // seconds on a phone's network. tower-http leaves out event streams, images, gRPC, bodies
        // under 32 bytes and answers already encoded upstream (the proxied `/cs` reads included).
        // The session cookie is SameSite=Lax and no answer echoes the CSRF token, so a cross-site
        // page cannot time compressed lengths of a signed-in answer (BREACH).
        .layer(CompressionLayer::new().gzip(true))
}

/// The sites a public form's page `/f/{slug}` may be framed by: its Endpoint's
/// `spec.creates.embedOrigins` under the Endpoint's own rule (EP-101). `None` for any other path,
/// an unknown slug, an empty list, or a list that does not pass the rule: then the page keeps the
/// Portal's own framing headers.
/// `connect-src` of the Portal's pages: its own origin and the data host it hands out
/// (`api.{domain}`, [`Config::api_base_url`]), the one origin the public /connect page asks for a
/// named MCP server's tools (T-4949). Exactly that origin, never a wildcard; a Portal that is its
/// own data host adds nothing.
fn connect_src(config: &Config) -> String {
    let api = config.api_base_url().origin().ascii_serialization();
    if api == config.public_base_url.origin().ascii_serialization() {
        "'self'".to_owned()
    } else {
        format!("'self' {api}")
    }
}

fn embed_origins(state: &AppState, path: &str) -> Option<Vec<String>> {
    let slug = path.strip_prefix("/f/")?;
    if slug.is_empty() || slug.contains(['/', '.']) {
        return None;
    }
    // ponytail: a scan of the mirror per form page view; an index by slug if it ever shows up.
    let endpoint = state.mirror.find(|env| {
        env.kind == "Endpoint" && env.spec.get("slug").and_then(|s| s.as_str()) == Some(slug)
    })?;
    let creates: jc_core::kinds::Creates =
        serde_json::from_value(endpoint.spec.get("creates")?.clone()).ok()?;
    if let Err(e) = creates.validate() {
        // The reconciler refuses such a manifest; one in the mirror anyway keeps SAMEORIGIN.
        tracing::warn!(slug = ?slug, error = %e, "a form's embedOrigins break the Endpoint's rule");
        return None;
    }
    (!creates.embed_origins.is_empty()).then_some(creates.embed_origins)
}

/// Lets the sites a form's Endpoint names frame its page and nothing else (EP-101): the page's
/// `frame-ancestors` gains exactly those origins and `X-Frame-Options`, which cannot name one, is
/// dropped. Every other response passes untouched.
async fn public_form_framing(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let origins = embed_origins(&state, request.uri().path());
    let mut response = next.run(request).await;
    let Some(origins) = origins else {
        return response;
    };
    let headers = response.headers_mut();
    let Some(policy) = headers
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|value| value.to_str().ok())
    else {
        return response;
    };
    let framed = policy.replacen(
        "frame-ancestors 'self';",
        &format!("frame-ancestors 'self' {};", origins.join(" ")),
        1,
    );
    if let Ok(value) = HeaderValue::from_str(&framed) {
        headers.insert(header::CONTENT_SECURITY_POLICY, value);
        headers.remove("x-frame-options");
    }
    response
}

/// Whether the route answered that this is a public answer: its `Cache-Control` begins with the
/// `public` directive (AP-67, UI-30, SDK-16).
///
/// The test is one-directional on purpose. A route says *public* and keeps what it set; a route
/// that says nothing, or says `private`, `no-cache` or anything else, gets `no-store` — so no
/// answer becomes cacheable by omission, and the only way into a browser cache is a handler that
/// wrote the word. The three routes that do are the basemap style, the basemap tile and the
/// branding block and its assets, none of which reads a session (T-2295).
fn route_says_public(response: &Response) -> bool {
    response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .is_some_and(|directive| directive.trim().eq_ignore_ascii_case("public"))
}

async fn api_cache_control_middleware(request: Request, next: Next) -> Response {
    let is_api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    // An event stream sets its own `no-cache`, which is what the SSE contract asks for; it must
    // not be overwritten with `no-store`, because some intermediaries read that as a reason not
    // to stream at all (AG-45).
    let streaming = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    // An API answer is `no-store` unless the route itself said it is public. This used to be a
    // comparison against one exact path, which overwrote the `public, max-age=3600` the basemap
    // style and every tile set and the `public, max-age=300` of `/api/v1/branding/logo`: a pan
    // of any map refetched its whole grid from the single-node cluster, and the logo came back
    // on every page load (T-2295).
    if is_api && !streaming && !route_says_public(&response) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

/// Whether [`serve`] opens the internal listener: only a Portal that verifies tokens can answer
/// any route on it.
fn internal_listener_opens(state: &AppState) -> bool {
    state.bearer.is_some()
}

/// The listener the credential proxy calls back on (AG-52).
///
/// A second listener rather than a path on the first: the edge routes `config.bind` and routes
/// nothing here, so the two calls that carry a run's ticket hash are not on the public URL
/// scheme at all. It carries no session layer, no CSRF guard and no static host — the proxy's
/// bearer is the whole of its authentication, checked inside the handlers.
pub fn internal_app(state: AppState) -> Router {
    Router::new()
        .merge(api::internal::router())
        .with_state(state)
}

pub async fn serve(config: Config) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(bind = %local_addr, public_url = %config.public_base_url, "portal server listening");
    let state = AppState::from_config(config)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    // A workspace outlives the Portal that made it unless something finishes its TTL: the record
    // hides at expiry, the branch does not go by itself (CC-81, T-1260).
    crate::ops::workspaces::spawn_reaper(state.clone());
    crate::api::agent_access::spawn_grant_reaper(state.clone());

    if let Some(syncer) = state.syncer.as_ref() {
        syncer.clone().spawn_periodic(state.config.sync_interval);

        // The `SyncSource` loop runs beside it, on the same replica (MF-28, CC-03).
        if let Some(driver) = state.sync.as_ref() {
            crate::sync::driver::spawn_periodic(driver.clone(), syncer.clone());
        }

        // The foreign-model mirror runs beside it, on the same replica (DM-48, DM-49). A
        // Portal whose peers cannot be reached still serves; the mirror says so in its log and
        // the reference stands with whatever the repository already pinned.
        if let Some(gitea) = state.gitea.as_ref() {
            match crate::sync::schema_api::PeerSchemaApi::new() {
                Ok(peers) => {
                    crate::sync::mirror::spawn_periodic(
                        gitea.clone(),
                        syncer.clone(),
                        state.config.public_base_url.to_string(),
                        std::sync::Arc::new(peers),
                    );
                }
                Err(err) => {
                    tracing::warn!(error = %err, "no HTTP client for peer schema surfaces; foreign models are not mirrored")
                }
            }
        }
    }

    // A build pod per queued App build (AP-130), on the replica that holds the lease.
    if let (Some(syncer), Some(gitea), Some(settings)) = (
        state.syncer.as_ref(),
        state.gitea.as_ref(),
        state.config.build_pods.clone(),
    ) {
        crate::apps::build_pods::spawn_periodic(
            gitea.clone(),
            syncer.clone(),
            state.mirror.clone(),
            settings,
        );
    }

    if state.config.agent_settings.is_some() {
        crate::agents::reaper::spawn_periodic(state.clone());
    }

    // The internal listener runs beside the public one, on the same replica, whenever this Portal
    // verifies tokens: every route on it answers a workload token of the realm and nothing else,
    // so a Portal without a realm opens no second port. The gateway polls its revocations here
    // whether or not an agent runner is deployed (T-5869, F-SEC1-024).
    if internal_listener_opens(&state) {
        let internal = tokio::net::TcpListener::bind(state.config.internal_bind).await?;
        tracing::info!(bind = %state.config.internal_bind, "portal internal listener listening");
        let internal_router = internal_app(state.clone());
        tokio::spawn(async move {
            if let Err(err) = axum::serve(internal, internal_router)
                .with_graceful_shutdown(shutdown_signal())
                .await
            {
                tracing::error!(error = %err, "the internal listener stopped");
            }
        });
    }

    // A data subject's erasures, asked again each minute until every store is done (T-4668).
    crate::erasure::spawn_periodic(state.clone());
    // The assistant's kept answers, forgotten when a Policy or an Endpoint changes (T-4731).
    crate::answers::spawn_periodic(state.clone());
    // What a restart of Grafana took, put back from the Portal's rows (T-6509).
    crate::grafana::spawn_periodic(state.clone());

    let router = app(state);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %err, "failed to listen for ctrl+c");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(err) => {
                tracing::error!(error = %err, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_src_adds_the_data_host_and_nothing_else() {
        let mut config = Config::for_tests();
        assert_eq!(connect_src(&config), "'self'");
        config.public_base_url = "https://portal.dev.example.org/".parse().expect("a url");
        assert_eq!(connect_src(&config), "'self' https://api.dev.example.org");
        config.public_base_url = "https://dev.example.org/".parse().expect("a url");
        assert_eq!(connect_src(&config), "'self'");
    }

    /// The gateway's revocations poll needs the internal listener on every Portal with a realm,
    /// agent runner or not; a Portal without a realm keeps the port closed (T-5869).
    #[test]
    fn the_internal_listener_opens_with_a_realm_and_without_an_agent_runner() {
        let mut state = AppState::new(Config::for_tests(), None);
        assert!(state.config.agent_settings.is_none());
        assert!(!internal_listener_opens(&state));
        let issuer: url::Url = "https://id.example.org/realms/r".parse().expect("a url");
        state.bearer = Some(std::sync::Arc::new(
            crate::auth::bearer::BearerVerifier::new(&issuer, "portal"),
        ));
        assert!(internal_listener_opens(&state));
    }
}
