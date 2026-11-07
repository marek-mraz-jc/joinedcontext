//! The internal listener's personal connectors (UI-98, EP-123, T-4531, T-5970): what the gateway
//! reads to serve `/api/mcp/me/{name}` to one person's token, from the connector store (API/14).

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::connectors::{Connector, ADMIN};
use crate::error::ApiError;
use crate::state::AppState;

/// Whether the connector reads data through the gateway: active and carrying `data-read`.
fn reads_data(connector: &Connector) -> bool {
    connector.is_active()
        && connector
            .segments
            .iter()
            .any(|s| s == "data-read" || s == ADMIN)
}

/// One person's data-reading connectors by their Keycloak `sub`, for the gateway's own client
/// only (PF-46): name, id, scope, expiry and allowed clients, nothing about who calls. A `sub`
/// with none answers an empty list, so the answer never says whether a person exists.
pub async fn connectors(
    State(state): State<AppState>,
    Path(sub): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    crate::auth::internal::authenticate_gateway(&state, &headers).await?;
    let base = crate::api::connectors::base(&state);
    let connectors: Vec<Value> = state
        .connectors
        .of_owner(&sub, &base)
        .await?
        .into_iter()
        .filter(reads_data)
        .map(|c| {
            json!({
                "id": c.id,
                "name": c.name,
                "scope": c.scope,
                "expiresAt": c.expires_at,
                "allowedClients": c.allowed_clients,
            })
        })
        .collect();
    Ok(Json(json!({ "connectors": connectors })))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/internal/connectors/{sub}", get(connectors))
}
