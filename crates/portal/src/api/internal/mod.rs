//! The internal listener (AG-52): every route the credential proxy, the agent runner, the pipeline
//! runner and the gateway call back on, and nothing else. It is served on its own port with no
//! session layer and no CSRF guard, so each handler authenticates its caller's bearer through
//! `crate::auth::internal` before it reads anything. A `/internal/` route lives here and nowhere
//! else, so a handler cannot be added to the wrong listener by being written in the wrong file.

use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use sha2::{Digest, Sha256};

use crate::state::AppState;

pub mod agent_runs;
pub mod connectors;
pub mod domain_verifications;
pub mod model_key;
pub mod pipeline_rejected;
pub mod pipeline_tests;
pub mod previews;
pub mod revocations;
pub mod usage;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(agent_runs::router())
        .merge(pipeline_tests::router())
        .merge(pipeline_rejected::router())
        .merge(previews::router())
        .merge(revocations::router())
        .merge(domain_verifications::router())
        .merge(model_key::router())
        .merge(connectors::router())
        .merge(usage::router())
}

/// A JSON body the gateway polls every ten seconds: its `ETag` is a SHA-256 over the body, and a
/// caller that sends that tag back gets `304` with nothing (CC-78). The caller is authenticated
/// before this is reached, so a `304` tells nobody without the token anything.
fn polled_json(headers: &HeaderMap, body: Vec<u8>) -> Response {
    let etag = format!("\"{:x}\"", Sha256::digest(&body));
    let unchanged = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|tag| tag.trim() == etag));
    if unchanged {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    (
        StatusCode::OK,
        [
            (header::ETAG, etag),
            (header::CONTENT_TYPE, "application/json".to_owned()),
        ],
        body,
    )
        .into_response()
}
