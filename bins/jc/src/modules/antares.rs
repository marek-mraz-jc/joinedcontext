//! Antares (NGSI-LD broker) inside `jc`: the broker library's own steps (configuration from its
//! `ANTARES_*` environment, state, background work, router), served through the mount router
//! instead of its own listener. The host owns the tracing subscriber and the metrics recorder
//! (`telemetry::init_hosted`), so `/q/metrics` is not served by the broker; `ANTARES_HTTP_PORT`
//! and `ANTARES_METRICS_PORT` open no listener here.
//!
//! The broker runs as the platform builds it (features `full`, `jc-policy`, `linkml`, `ical`,
//! `feed`): Endpoint views and the `joinedcontext` engine. The keys dev sets for that come as
//! defaults an environment value overrides ([`defaults`]), and they name the gateway's identity
//! as the only caller whose subject and provenance headers the broker believes.
//!
//! On dev Linkerd sets `l5d-client-id` from the caller's certificate. Here nothing outside the
//! process may claim it: the public mount drops every `l5d-*` header, and only the gateway's hop
//! ([`Hop`]), a loopback listener that admits a request by a per-installation secret, sets it.
//!
//! The broker keeps process-wide statics (memory budget, WebSocket hub, caches): one Antares per
//! process.

use crate::app::StartError;
use crate::config::AntaresSection;
use crate::module::Module;
use crate::modules::Hop;
use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn err(e: impl std::fmt::Display) -> StartError {
    StartError::module(Module::Antares, e)
}

/// The surfaces dev's broker serves (deployment `context-broker` base values), without
/// `parquet` and `tabular` (OGC `f=parquet|csv|zip`), which this build leaves out.
pub const SURFACES: &str = "admin,views,mcp,access,ws,ws-views,ogc,linkml,odata,sta,ical,feed";

/// The subject headers the gateway sends and the engine reads, as dev names them.
pub const SUBJECT_HEADERS: &str = "x-jc-subject,x-jc-groups,x-jc-roles,x-jc-client,x-jc-restricted,x-jc-did,x-jc-read-only,x-jc-member";

/// The broker settings `jc` supplies for keys the environment leaves unset.
#[must_use]
pub fn defaults() -> BTreeMap<String, String> {
    [
        ("ANTARES_API_SURFACES", SURFACES),
        ("ANTARES_POLICY", "joinedcontext"),
        ("ANTARES_POLICY_SUBJECT_HEADERS", SUBJECT_HEADERS),
        ("ANTARES_PROVENANCE_FROM", crate::modules::GATEWAY_IDENTITY),
        (
            "ANTARES_POLICY_SUBJECT_FROM",
            crate::modules::GATEWAY_IDENTITY,
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

/// Removes every header a mesh would set (`l5d-*`) and the hop header: on the public mount no
/// caller is anybody but itself.
fn drop_claimed_identity(headers: &mut axum::http::HeaderMap) {
    let claimed: Vec<_> = headers
        .keys()
        .filter(|name| {
            name.as_str().starts_with("l5d-") || name.as_str() == crate::modules::GATEWAY_HOP_HEADER
        })
        .cloned()
        .collect();
    for name in claimed {
        headers.remove(&name);
    }
}

async fn public_guard(mut req: Request, next: Next) -> Response {
    drop_claimed_identity(req.headers_mut());
    next.run(req).await
}

/// The hop's door: the secret in constant time, else 403; then the gateway's identity in place of
/// anything the request said.
async fn hop_guard(secret: Arc<str>, mut req: Request, next: Next) -> Response {
    let admitted = req
        .headers()
        .get(crate::modules::GATEWAY_HOP_HEADER)
        .is_some_and(|v| crate::runtime::secret_eq(v.as_bytes(), secret.as_bytes()));
    if !admitted {
        return StatusCode::FORBIDDEN.into_response();
    }
    drop_claimed_identity(req.headers_mut());
    req.headers_mut().insert(
        "l5d-client-id",
        HeaderValue::from_static(crate::modules::GATEWAY_IDENTITY),
    );
    next.run(req).await
}

/// The broker's routers: the public mount, and the gateway's hop when `hop_secret` is given.
pub struct Built {
    pub router: Router,
    pub hop: Option<Hop>,
}

pub async fn build(
    cfg: &AntaresSection,
    setup: &crate::setup::Setup,
    hop_secret: Option<&str>,
    shutdown: &CancellationToken,
) -> Result<Built, StartError> {
    let render = antares_broker::telemetry::init_hosted();
    let mut broker_cfg = antares_broker::config_from_env().map_err(err)?;
    broker_cfg.defaults = defaults();
    // The `/q` admin credential is mandatory in a shared process.
    let token_file = cfg
        .admin_token_file
        .as_deref()
        .or_else(|| setup.path(crate::setup::Secret::AntaresAdminToken))
        .ok_or_else(|| err("no admin_token_file"))?;
    broker_cfg.admin_token_file = Some(token_file.display().to_string());
    if broker_cfg.metrics_port.is_some() {
        tracing::warn!(
            "ANTARES_METRICS_PORT opens no listener inside jc; use the jc admin listener"
        );
        broker_cfg.metrics_port = None;
    }
    let mut broker = antares_broker::build_state(&broker_cfg, render)
        .await
        .map_err(err)?;
    antares_broker::spawn_background(&mut broker)
        .await
        .map_err(err)?;
    let served = antares_broker::router(broker).map_err(err)?;
    let hop = match hop_secret {
        Some(secret) => Some(serve_hop(served.app.clone(), secret, shutdown).await?),
        None => None,
    };
    let router = Router::new()
        .fallback_service(served.app)
        .layer(axum::middleware::from_fn(public_guard));
    Ok(Built { router, hop })
}

async fn serve_hop(
    app: antares_broker::App,
    secret: &str,
    shutdown: &CancellationToken,
) -> Result<Hop, StartError> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|e| err(format!("gateway hop: {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| err(format!("gateway hop: {e}")))?;
    let secret: Arc<str> = secret.into();
    let guard_secret = Arc::clone(&secret);
    let router = Router::new()
        .fallback_service(app)
        .layer(axum::middleware::from_fn(
            move |req: Request, next: Next| hop_guard(Arc::clone(&guard_secret), req, next),
        ));
    let token = shutdown.clone();
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router)
            .with_graceful_shutdown(token.cancelled_owned())
            .await
        {
            tracing::error!(error = %e, "antares: the gateway hop stopped");
        }
    });
    tracing::info!(%addr, "antares: gateway hop listening on loopback");
    Ok(Hop { addr, secret })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_public_mount_drops_every_mesh_header_and_the_hop_header() {
        let mut h = axum::http::HeaderMap::new();
        h.insert(
            "l5d-client-id",
            HeaderValue::from_static(crate::modules::GATEWAY_IDENTITY),
        );
        h.insert("L5d-Dst-Override", HeaderValue::from_static("x"));
        h.insert(
            crate::modules::GATEWAY_HOP_HEADER,
            HeaderValue::from_static("guess"),
        );
        h.insert(
            "x-jc-subject",
            HeaderValue::from_static("kept for the broker to judge"),
        );
        drop_claimed_identity(&mut h);
        assert_eq!(h.len(), 1, "{h:?}");
        assert!(h.contains_key("x-jc-subject"));
    }

    #[test]
    fn the_defaults_name_the_gateway_as_the_only_believed_caller() {
        let d = defaults();
        assert_eq!(
            d["ANTARES_PROVENANCE_FROM"],
            crate::modules::GATEWAY_IDENTITY
        );
        assert_eq!(
            d["ANTARES_POLICY_SUBJECT_FROM"],
            crate::modules::GATEWAY_IDENTITY
        );
        assert_eq!(d["ANTARES_POLICY"], "joinedcontext");
        assert!(d["ANTARES_API_SURFACES"].split(',').any(|s| s == "views"));
    }

    #[test]
    fn a_hop_never_prints_its_secret() {
        let hop = Hop {
            addr: "127.0.0.1:1".parse().unwrap(),
            secret: "s3cr3t-value".into(),
        };
        assert!(!format!("{hop:?}").contains("s3cr3t"));
    }
}
