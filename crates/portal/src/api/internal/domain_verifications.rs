//! The internal listener's domain verifications (Architecture/03 §3, T-2572): what the gateway
//! reads to decide whether a write to a space may go through under `domainVerification: enforce`.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Serialize;

use crate::domain_verification::{gate_states, GateState};
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Serialize)]
struct Items {
    items: Vec<GateState>,
}

/// Every Organization of the repository with its domain and state, for the gateway only (PF-41,
/// PF-46). Nothing here is secret, but it is the input of a control: only the caller that
/// enforces it reads it, with its own ServiceAccount token, and the NetworkPolicy is the second
/// control, never the only one. Polled every ten seconds, so it answers `304` to its own `ETag`.
pub async fn domain_verifications(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    crate::auth::internal::authenticate_gateway(&state, &headers).await?;
    let organizations: Vec<(String, String)> = state
        .mirror
        .matching(|envelope| envelope.kind == "Organization")
        .into_iter()
        .filter_map(|envelope| {
            let domain = envelope.spec["domain"].as_str()?.to_owned();
            Some((envelope.metadata.name, domain))
        })
        .collect();
    let items = gate_states(state.db.as_ref(), &organizations)
        .await
        .map_err(|e| ApiError::Internal(format!("the domain verifications were not read: {e}")))?;
    let body = serde_json::to_vec(&Items { items }).map_err(|e| {
        ApiError::Internal(format!("the domain verifications did not serialise: {e}"))
    })?;
    Ok(super::polled_json(&headers, body))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/internal/domain-verifications", get(domain_verifications))
}
