//! The internal listener's preview list (Architecture/06 §7.2, Architecture/13 §6): what the gateway
//! reads to serve the running workspace previews.

use crate::error::ApiError;
use crate::ops::previews::{self, ServedList};
use crate::state::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use axum::Router;

/// Every running preview for the gateway, on the internal listener only (Architecture/06 §7.2,
/// Architecture/13 §6).
///
/// Manifests hold `secretRef`s and no secret, but they are a project's configuration, so the
/// gateway's own ServiceAccount token is what opens this route (PF-46, AG-52, T-1500). The
/// NetworkPolicy that admits the gateway to this port is the second control and not the only one:
/// a pod that reaches the port through a policy mistake presents no such token and reads nothing.
///
/// The gateway asks every ten seconds, so the answer carries an `ETag` over its body and a list
/// that did not change answers `304` with none (CC-78). The token is checked first, so a `304`
/// tells a caller without one nothing either.
pub async fn served_previews(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    crate::auth::internal::authenticate_gateway(&state, &headers).await?;
    let list: ServedList = previews::served(&state).await?;
    let body = serde_json::to_vec(&list)
        .map_err(|e| ApiError::Internal(format!("the preview list did not serialise: {e}")))?;
    Ok(super::polled_json(&headers, body))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/internal/previews", get(served_previews))
}
