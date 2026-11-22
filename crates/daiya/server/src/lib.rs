#![forbid(unsafe_code)]
//! `daiya-server`: Mountable router (`router(state, mount)`) for `jc-router`, configuration
//! struct (`ServerConfig`), and unified CLI operation dispatcher (R2, R10a, R11, R12).
//!
//! Library code installs no global tracing subscriber, metrics recorder, allocator, or rustls
//! provider, and performs no environment variable reads (R11).

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
pub use daiya_api::DaiyaState;
use daiya_api::api_router;
use daiya_model::{InputLimits, OrgQuotas};
pub use daiya_query::{EndpointBase, OperatorReach, OwnReach};
use jc_router::Mount;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::SocketAddr;

pub mod config;
mod operations;

pub use operations::execute_cli_command;

/// Server configuration passed as a struct (R11: no env reads in library code).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub listen_addr: SocketAddr,
    pub limits: InputLimits,
    pub default_quotas: OrgQuotas,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: SocketAddr::from(([127, 0, 0, 1], 3000)),
            limits: InputLimits::default(),
            default_quotas: OrgQuotas::default(),
        }
    }
}

async fn http_mount_info(
    Extension(default_mount): Extension<DefaultMount>,
    req_mount: Option<Extension<Mount>>,
) -> Response {
    let mount = req_mount.map_or(default_mount.0, |Extension(m)| m);
    let session_cookie = mount.cookie_name("cr_session");
    let odata_url = format!("{}/odata/v4", mount.public_base());
    (
        StatusCode::OK,
        Json(json!({
            "module": mount.module,
            "publicBase": mount.public_base(),
            "pathPrefix": mount.path_prefix,
            "odataUrl": odata_url,
            "sessionCookieName": session_cookie,
        })),
    )
        .into_response()
}

#[derive(Clone)]
struct DefaultMount(Mount);

/// Middleware applying security headers, CSP (`frame-ancestors 'self'; object-src 'none'; base-uri 'self'`),
/// and prefix-aware mount metadata (R11, R12, `apisix-plugins.yaml:68`).
async fn security_and_mount_headers(
    Extension(default_mount): Extension<DefaultMount>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let mount = req
        .extensions()
        .get::<Mount>()
        .cloned()
        .unwrap_or(default_mount.0);
    let public_base = mount.public_base();
    let cookie_name = mount.cookie_name("cr_session");

    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    headers.entry("content-security-policy").or_insert_with(|| {
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; frame-ancestors 'self'; object-src 'none'; base-uri 'self'",
        )
    });
    headers
        .entry("x-content-type-options")
        .or_insert_with(|| HeaderValue::from_static("nosniff"));
    headers
        .entry("x-frame-options")
        .or_insert_with(|| HeaderValue::from_static("SAMEORIGIN"));
    headers
        .entry("referrer-policy")
        .or_insert_with(|| HeaderValue::from_static("strict-origin-when-cross-origin"));
    // Tenant answers and new public-dashboard tokens are never stored by an intermediary
    // unless a handler says otherwise.
    headers
        .entry(axum::http::header::CACHE_CONTROL)
        .or_insert_with(|| HeaderValue::from_static("private, no-store"));
    if let Ok(hv) = HeaderValue::from_str(&public_base) {
        headers.insert("x-daiya-public-base", hv);
    }
    if let Ok(hv) = HeaderValue::from_str(&cookie_name) {
        headers.insert("x-daiya-cookie-name", hv);
    }
    resp
}

/// Builds the mountable `daiya` [`Router`] honouring `jc_router::Mount` (R11).
pub fn router(state: DaiyaState, mount: Mount) -> Router {
    let mount_info = Router::new()
        .route("/mount-info", get(http_mount_info))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            daiya_api::state::refuse_unknown_credential,
        ));

    Router::new()
        .merge(api_router(state, mount.clone()))
        .merge(mount_info)
        .layer(middleware::from_fn(security_and_mount_headers))
        .layer(Extension(DefaultMount(mount)))
}
