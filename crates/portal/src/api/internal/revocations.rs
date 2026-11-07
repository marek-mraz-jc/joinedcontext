//! The internal listener's list of revoked subjects (T-5568, TR-03187 AR-6): what the gateway
//! reads so a person disabled, deleted or logged out, or a ServiceAccount removed, is refused on
//! its doors within one poll rather than at the expiry of the token they still hold.

use crate::error::ApiError;
use crate::state::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    subject: String,
    revoked_at: i64,
}

#[derive(Serialize)]
struct List {
    items: Vec<Item>,
}

/// Every mark a token could still be valid against, for the gateway alone: its own workload
/// token opens the route (PF-46, AG-52), and the subjects are user ids nobody else needs.
/// Polled every ten seconds, so it answers `304` while the list stands (CC-78).
pub async fn revoked_subjects(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    crate::auth::internal::authenticate_gateway(&state, &headers).await?;
    let items = state
        .revocations(crate::auth::session::now_unix())
        .into_iter()
        .map(|(subject, revoked_at)| Item {
            subject,
            revoked_at,
        })
        .collect();
    let body = serde_json::to_vec(&List { items })
        .map_err(|e| ApiError::Internal(format!("the revocations did not serialise: {e}")))?;
    Ok(super::polled_json(&headers, body))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/internal/revocations", get(revoked_subjects))
}
