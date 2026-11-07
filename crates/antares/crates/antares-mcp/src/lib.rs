// SPDX-License-Identifier: EUPL-1.2
//! The MCP surface of a view (`docs/analysis/mcp-surface.md`, T-3442):
//! `POST /x/views/{view}/mcp` is a Streamable HTTP endpoint whose tools and
//! resources are the view's operations, each run as one operation
//! through [`antares_ops`], so the operation's own policy gate fires once
//! and the view narrows the answer as it does on the NGSI-LD view root.
//!
//! Protocol: 2026-07-28 stateless first; the legacy `initialize` of
//! 2025-11-25 and 2025-06-18 is served without a session (D-8). The tool
//! list is the view's allowlist filtered by one engine probe (D-3); input
//! schemas name only what the view shows (D-4); a list result is paged
//! with an opaque cursor under a byte budget, never cut (D-10). A write
//! tool runs only after the person confirmed the call (D-11, T-3514);
//! resource subscriptions come with T-3515.
//!
//! ```
//! use antares_ops::Surface;
//! let mcp = antares_mcp::Mcp;
//! assert_eq!(mcp.prefix(), "/x/views");
//! assert_eq!(antares_ops::mount(mcp).name(), "mcp");
//! ```
#![forbid(unsafe_code)]

mod area;
mod budget;
mod confirm;
mod cursor;
mod decided;
#[cfg(feature = "fuzzing")]
pub mod fuzzing;
mod guide;
mod listen;
mod schema;
mod server;
mod summary;
mod tools;
mod units;

use antares_ops::{Caller, Ops, Refusal, Surface};
use axum::extract::{Path, Request};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse as _, Response};
use axum::routing::any;
use axum::Router;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use std::sync::{Arc, LazyLock};
use tower::ServiceExt as _;

/// Where the surface mounts: the view root it shares (ADR-0027 section 1).
pub const PREFIX: &str = "/x/views";

/// The `Host` names the endpoint answers, comma separated; unset, the
/// loopback names only (DNS rebinding, MC8).
pub const ALLOWED_HOSTS_ENV: &str = "ANTARES_MCP_ALLOWED_HOSTS";

/// The browser origins the endpoint answers, comma separated; unset, a
/// request carrying any `Origin` is refused (MC8).
pub const ALLOWED_ORIGINS_ENV: &str = "ANTARES_MCP_ALLOWED_ORIGINS";

/// The most bytes one tool result's structured content may take (D-10).
pub const MAX_RESULT_BYTES_ENV: &str = "ANTARES_MCP_MAX_RESULT_BYTES";

/// A file holding the key that seals a write's `requestState` (MCP 3.4),
/// at least 32 bytes, mounted from a Secret; unset, each process draws its
/// own and a confirmation answers only the process that asked.
pub const REQUEST_STATE_KEY_FILE_ENV: &str = "ANTARES_MCP_REQUEST_STATE_KEY_FILE";

/// Adaptive summary budget environment variables (T-4680).
pub const SUMMARY_BUSY_MEMORY_PCT_ENV: &str = "ANTARES_SUMMARY_BUSY_MEMORY_PCT";
/// Pod memory percentage threshold for Tight state.
pub const SUMMARY_TIGHT_MEMORY_PCT_ENV: &str = "ANTARES_SUMMARY_TIGHT_MEMORY_PCT";
/// CPU pressure threshold for Busy state.
pub const SUMMARY_BUSY_CPU_AVG10_ENV: &str = "ANTARES_SUMMARY_BUSY_CPU_AVG10";
/// CPU pressure threshold for Tight state.
pub const SUMMARY_TIGHT_CPU_AVG10_ENV: &str = "ANTARES_SUMMARY_TIGHT_CPU_AVG10";
/// The node's `MemAvailable` in MiB below which a summary is Busy.
pub const SUMMARY_BUSY_NODE_MIB_ENV: &str = "ANTARES_SUMMARY_BUSY_NODE_MIB";
/// The node's `MemAvailable` in MiB below which a summary is Tight.
pub const SUMMARY_TIGHT_NODE_MIB_ENV: &str = "ANTARES_SUMMARY_TIGHT_NODE_MIB";
/// Normal row budget for summary.
pub const SUMMARY_ROWS_ENV: &str = "ANTARES_SUMMARY_ROWS";
/// Normal byte budget for summary.
pub const SUMMARY_MAX_BYTES_ENV: &str = "ANTARES_SUMMARY_MAX_BYTES";
/// Concurrency limit for summary tasks.
pub const SUMMARY_CONCURRENCY_ENV: &str = "ANTARES_SUMMARY_CONCURRENCY";

/// The largest area `area_overview` searches, in km² (T-5279).
pub const AREA_MAX_KM2_ENV: &str = "ANTARES_AREA_MAX_KM2";

/// Every variable this crate reads. The broker accepts these as config keys
/// from here, so a new one is added in this crate alone (T-3766).
pub const ENV_KEYS: &[&str] = &[
    ALLOWED_HOSTS_ENV,
    ALLOWED_ORIGINS_ENV,
    AREA_MAX_KM2_ENV,
    CONFIRM_URL_ENV,
    MAX_RESULT_BYTES_ENV,
    REQUEST_STATE_KEY_FILE_ENV,
    SUBSCRIPTION_ENDPOINTS_ENV,
    SUMMARY_BUSY_CPU_AVG10_ENV,
    SUMMARY_BUSY_MEMORY_PCT_ENV,
    SUMMARY_BUSY_NODE_MIB_ENV,
    SUMMARY_CONCURRENCY_ENV,
    SUMMARY_MAX_BYTES_ENV,
    SUMMARY_ROWS_ENV,
    SUMMARY_TIGHT_CPU_AVG10_ENV,
    SUMMARY_TIGHT_MEMORY_PCT_ENV,
    SUMMARY_TIGHT_NODE_MIB_ENV,
];

pub use antares_ops::PRIMER;
pub use decided::{routes as confirmations, CONFIRM_URL_ENV};
pub use listen::Sink;
pub use schema::SUBSCRIPTION_ENDPOINTS_ENV;

/// The largest request body the endpoint reads (`MCP_MAX_REQUEST_BYTES`).
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// The MCP surface: `/x/views/{view}/mcp`.
#[derive(Debug, Clone, Copy)]
pub struct Mcp;

impl Surface for Mcp {
    fn name(&self) -> &'static str {
        "mcp"
    }

    fn prefix(&self) -> &'static str {
        PREFIX
    }

    fn version_info(&self) -> serde_json::Value {
        serde_json::json!({
            "endpoint": format!("{PREFIX}/{{view}}/mcp"),
            "protocolVersions": server::VERSIONS.iter().map(rmcp::model::ProtocolVersion::as_str).collect::<Vec<_>>(),
        })
    }

    fn router(&self, ops: Ops) -> Router {
        Router::new().route(
            "/{view}/mcp",
            any(
                move |Path(id): Path<String>, caller: Caller, req: Request| {
                    let ops = ops.clone();
                    async move { serve(&ops, &id, caller, req).await }
                },
            ),
        )
    }
}

/// The transport as this surface runs it (MCP 6.3): no sessions, JSON
/// answers, the deployment's hosts and origins with Origin validation on,
/// and a bounded body.
fn config() -> StreamableHttpServerConfig {
    static HOSTS: LazyLock<Option<Vec<String>>> =
        LazyLock::new(|| listed(std::env::var(ALLOWED_HOSTS_ENV).ok().as_deref()));
    static ORIGINS: LazyLock<Option<Vec<String>>> =
        LazyLock::new(|| listed(std::env::var(ALLOWED_ORIGINS_ENV).ok().as_deref()));
    config_of(HOSTS.as_deref(), ORIGINS.as_deref().unwrap_or_default())
}

/// [`config`] for `hosts` (rmcp's loopback names when `None`) and
/// `origins`.
fn config_of(hosts: Option<&[String]>, origins: &[String]) -> StreamableHttpServerConfig {
    let config = StreamableHttpServerConfig::default();
    let hosts = hosts.map_or_else(|| config.allowed_hosts.clone(), <[String]>::to_vec);
    config
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_max_request_body_bytes(MAX_REQUEST_BYTES)
        .with_allowed_hosts(hosts)
        .with_allowed_origins(origins.to_vec())
        .enforce_origin_validation()
}

/// The comma-separated names of `raw`, `None` when it names none.
pub(crate) fn listed(raw: Option<&str>) -> Option<Vec<String>> {
    let names: Vec<String> = raw?
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .collect();
    (!names.is_empty()).then_some(names)
}

/// A JSON-RPC error with no id, as `status`.
fn rpc_refusal(status: StatusCode, code: i32, message: &str) -> Response {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": null, "error": {"code": code, "message": message}
    });
    (status, axum::Json(body)).into_response()
}

async fn serve(ops: &Ops, id: &str, caller: Caller, req: Request) -> Response {
    // a view of another model too: its reads run through the mapping
    let Some(view) = ops.mapped_view(id) else {
        return Refusal::NotFound.ngsi_ld();
    };
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_REQUEST_BYTES).await else {
        return rpc_refusal(
            StatusCode::PAYLOAD_TOO_LARGE,
            -32600,
            "the request body is too large",
        );
    };
    // rmcp answers a body it cannot read with a bare 415; JSON-RPC 2.0
    // section 5.1 wants an error frame. MC6: batching left MCP in 2025-06-18,
    // so an array is an invalid request, refused whole.
    if parts.method == Method::POST {
        let refusal = match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Err(_) => Some((-32700, "the body is not JSON")),
            Ok(v) if v.is_array() => Some((-32600, "a JSON-RPC batch is not served")),
            Ok(v) if v.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0") => {
                Some((-32600, "not a JSON-RPC 2.0 message"))
            }
            Ok(_) => None,
        };
        if let Some((code, message)) = refusal {
            return rpc_refusal(StatusCode::BAD_REQUEST, code, message);
        }
    }
    let req = Request::from_parts(parts, axum::body::Body::from(bytes));
    let server = server::Server::new(view, caller, id);
    // ponytail: one service per request; the factory and its schema cache
    // live as long as the request, which a stateless endpoint needs anyway.
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(NeverSessionManager::default()),
        config(),
    );
    let Ok(answer) = service.oneshot(req).await;
    answer.map(axum::body::Body::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_names_its_trimmed_members_or_nothing() {
        assert_eq!(
            listed(Some(" a.example, ,b:8080 ")),
            Some(vec!["a.example".into(), "b:8080".into()])
        );
        assert_eq!(listed(Some(" , ")), None);
        assert_eq!(listed(None), None);
    }

    #[test]
    fn the_deployment_names_the_hosts_and_origins_else_loopback_and_none() {
        let own = config_of(
            Some(&["broker.example".to_owned()]),
            &["https://app.example".to_owned()],
        );
        assert_eq!(own.allowed_hosts, vec!["broker.example"]);
        let default = config_of(None, &[]);
        assert_eq!(
            default.allowed_hosts,
            StreamableHttpServerConfig::default().allowed_hosts
        );
        assert!(default.json_response);
        assert!(!default.legacy_session_mode);
    }

    #[test]
    fn the_surface_names_itself_and_its_versions() {
        assert_eq!(Mcp.name(), "mcp");
        assert_eq!(
            Mcp.version_info(),
            serde_json::json!({
                "endpoint": "/x/views/{view}/mcp",
                "protocolVersions": ["2026-07-28", "2025-11-25", "2025-06-18"]
            })
        );
    }
}
