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
//! With `[antares.database]` the broker serves from Postgres (`ANTARES_STORE=postgres` unless
//! the environment asks for timescale), the URL built from the section and its password file
//! instead of `ANTARES_DATABASE_URL`, and a role that bypasses row-level security refuses the
//! start (`require_rls`, the standalone `ANTARES_REQUIRE_RLS=1`). The broker migrates itself at
//! start under sqlx's advisory lock, as standalone.
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

/// The largest Tenant schema the broker stores, dev's 1 MiB (`linkml_model::SURFACE_LIMITS`).
pub const SCHEMA_MAX_BYTES: &str = "1048576";

/// The subject headers the gateway sends and the engine reads, as dev names them.
pub const SUBJECT_HEADERS: &str = "x-jc-subject,x-jc-groups,x-jc-roles,x-jc-client,x-jc-restricted,x-jc-did,x-jc-read-only,x-jc-member";

/// dev's write settings (deployment `context-broker` base values), so a pipeline writes into jc
/// as fast as into dev (jc-smoke-pipelines: 3,708 entities 14.0 s with the broker's own defaults,
/// 2.1-2.9 s with these):
/// - `ANTARES_TEMPORAL_RECORD=ruled`: history only for a type its space, type rule or Entity gives
///   a history duration (the broker alone records every write).
/// - `ANTARES_PG_BATCH_ASYNC_COMMIT=true`: batch upserts commit without waiting for the WAL flush.
///   A Postgres crash can lose the last few hundred milliseconds of batch writes; the next run of
///   the pipeline writes them again, and single writes stay durable. dev accepts that.
/// - `ANTARES_PG_POOL=40`: the broker's pool (20 alone); `[antares.database] max_connections`
///   or the environment's value wins, so an installation sizes it to its Postgres.
/// - `ANTARES_HISTORY_BACKFILL=on`: a write older than the current value goes to history only
///   instead of replacing it (the Portal's "add an old value"). The ETSI suites assume off and
///   run on their own broker.
pub const WRITE_DEFAULTS: [(&str, &str); 4] = [
    ("ANTARES_TEMPORAL_RECORD", "ruled"),
    ("ANTARES_PG_BATCH_ASYNC_COMMIT", "true"),
    ("ANTARES_PG_POOL", "40"),
    ("ANTARES_HISTORY_BACKFILL", "on"),
];

/// The broker settings `jc` supplies for keys the environment leaves unset.
#[must_use]
pub fn defaults() -> BTreeMap<String, String> {
    WRITE_DEFAULTS
        .into_iter()
        .chain([
            ("ANTARES_API_SURFACES", SURFACES),
            ("ANTARES_POLICY", "joinedcontext"),
            ("ANTARES_POLICY_SUBJECT_HEADERS", SUBJECT_HEADERS),
            ("ANTARES_PROVENANCE_FROM", crate::modules::GATEWAY_IDENTITY),
            (
                "ANTARES_POLICY_SUBJECT_FROM",
                crate::modules::GATEWAY_IDENTITY,
            ),
            // The Portal PUTs each Tenant's schema to /q/schemas/{tenant}; unset, the broker takes
            // none and serves no OGC or OData collection (dev's value).
            ("ANTARES_SCHEMA_MAX_BYTES", SCHEMA_MAX_BYTES),
        ])
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
        .is_some_and(|v| jc_secret::constant_time_eq(v.as_bytes(), secret.as_bytes()));
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
    // The `/q` admin credential is mandatory in a shared process: the operator's, else the one
    // setup holds (a file while the Portal, which reads it from one, runs beside it; else made
    // per boot in memory).
    let token = setup
        .value(crate::setup::Secret::AntaresAdminToken)
        .ok_or_else(|| err("no admin token"))?;
    broker_cfg.admin_token_file = None;
    broker_cfg.admin_token = Some(antares_broker::AdminToken(token.to_owned()));
    if let Some(db) = &cfg.database {
        match broker_cfg.store_name.as_str() {
            "memory" => broker_cfg.store_name = "postgres".into(),
            "postgres" | "timescale" => {}
            other => {
                return Err(err(format!(
                    "[antares.database] is set but ANTARES_STORE={other}; unset it or use postgres"
                )));
            }
        }
        // As standalone (`config_from_env`): bus=local over a shared database double-fires
        // notifications when a second process serves it, unless the deployment says it runs one.
        let one_process = crate::config::antares_env("ANTARES_ALLOW_SHARED_LOCAL")
            .is_some_and(|v| matches!(v.as_str(), "1" | "true"));
        if broker_cfg.bus_mode == "local" && !one_process {
            return Err(err(
                "[antares.database] with ANTARES_BUS=local double-fires notifications when more \
                 than one jc shares the database; use ANTARES_BUS=nats, or set \
                 ANTARES_ALLOW_SHARED_LOCAL=1 for a single jc process",
            ));
        }
        let url = db.app_url_with_password().map_err(err)?;
        broker_cfg.database_url = Some(antares_broker::Secret::new(url.expose().to_owned()));
        // Unset: ANTARES_PG_POOL (default 20), as standalone.
        broker_cfg.pg_pool = db.max_connections;
    }
    if broker_cfg.database_url.is_some() || broker_cfg.store_name != "memory" {
        broker_cfg.require_rls = true;
    }
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

    /// dev's four write settings come as defaults beside the identity keys; the broker reads each
    /// through its host defaults, so the environment or `[antares.database]` still wins
    /// (antares-broker `host_defaults_fill_only_what_the_environment_leaves_unset`).
    #[test]
    fn the_broker_defaults_carry_dev_write_settings() {
        let d = defaults();
        for (key, value) in [
            ("ANTARES_TEMPORAL_RECORD", "ruled"),
            ("ANTARES_PG_BATCH_ASYNC_COMMIT", "true"),
            ("ANTARES_PG_POOL", "40"),
            ("ANTARES_HISTORY_BACKFILL", "on"),
            ("ANTARES_POLICY", "joinedcontext"),
        ] {
            assert_eq!(d.get(key).map(String::as_str), Some(value), "{key}");
        }
        assert_eq!(d.len(), 10);
    }

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
        assert_eq!(d["ANTARES_SCHEMA_MAX_BYTES"], "1048576");
    }

    /// The `l5d-client-id` a request reaches the broker with, behind `guard`.
    async fn seen_by_the_broker(
        guard: Router,
        headers: &[(&str, &str)],
    ) -> (StatusCode, Option<String>) {
        use tower::ServiceExt;
        let mut req = Request::builder().uri("/ngsi-ld/v1/types");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = guard
            .oneshot(req.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let seen = String::from_utf8(body.to_vec()).unwrap();
        (status, (!seen.is_empty()).then_some(seen))
    }

    fn echo() -> Router {
        Router::new().fallback(|req: Request| async move {
            req.headers()
                .get("l5d-client-id")
                .map(|v| v.to_str().unwrap().to_owned())
                .unwrap_or_default()
        })
    }

    #[tokio::test]
    async fn only_the_hop_with_its_secret_names_the_gateway_to_the_broker() {
        let id = crate::modules::GATEWAY_IDENTITY;
        let hop = crate::modules::GATEWAY_HOP_HEADER;
        let public = echo().layer(axum::middleware::from_fn(public_guard));
        // Outside callers: a claimed mesh identity is dropped, with or without a guessed secret.
        for headers in [
            vec![("l5d-client-id", id)],
            vec![("l5d-client-id", id), (hop, "hop-secret")],
        ] {
            assert_eq!(
                seen_by_the_broker(public.clone(), &headers).await,
                (StatusCode::OK, None)
            );
        }
        let secret: Arc<str> = "hop-secret".into();
        let guarded = echo().layer(axum::middleware::from_fn(move |req, next| {
            hop_guard(Arc::clone(&secret), req, next)
        }));
        // The hop without its secret, or with a wrong one, is 403 and the broker is not asked.
        for headers in [
            vec![("l5d-client-id", id)],
            vec![(hop, "hop-secreT")],
            vec![(hop, "")],
        ] {
            assert_eq!(
                seen_by_the_broker(guarded.clone(), &headers).await,
                (StatusCode::FORBIDDEN, None)
            );
        }
        // With it, the broker sees the gateway's identity, whatever the caller claimed.
        assert_eq!(
            seen_by_the_broker(guarded, &[(hop, "hop-secret"), ("l5d-client-id", "x.y")]).await,
            (StatusCode::OK, Some(id.to_owned()))
        );
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
