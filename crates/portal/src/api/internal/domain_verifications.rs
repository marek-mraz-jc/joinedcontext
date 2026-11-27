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
    // Plain strings and enums: serialising them cannot fail.
    let body = serde_json::to_vec(&Items { items }).map_err(ApiError::internal)?;
    Ok(super::polled_json(&headers, body))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/internal/domain-verifications", get(domain_verifications))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use axum::http::{header, HeaderValue, StatusCode};
    use axum::response::IntoResponse;
    use std::sync::Arc;

    const ISSUER: &str = "https://idm.example.test/realms/dev";

    /// T-7917: the registry's database does not answer, so the gateway gets a 500 that names no
    /// database host, and never a list of `pending` states it did not check.
    #[tokio::test]
    async fn a_dead_registry_database_answers_500_without_the_drivers_text() {
        let mut config = Config::for_tests();
        config.gateway_client_id = Some("context-gateway".into());
        let mut state = AppState::new(config, None);
        let (verifier, sign) = crate::auth::bearer::test_verifier(ISSUER, "portal-api", "k1");
        state.bearer = Some(Arc::new(verifier));
        state.db = Some(crate::test_db::dead_pool());
        state.mirror.upsert(
            serde_json::from_value(serde_json::json!({
                "apiVersion": "joinedcontext.com/v1alpha1",
                "kind": "Organization",
                "metadata": { "name": "hel", "namespace": "org" },
                "spec": { "domain": "hel.fi" }
            }))
            .expect("an Organization"),
        );
        let now = crate::auth::session::now_unix();
        let token = sign(&serde_json::json!({
            "iss": ISSUER, "typ": "Bearer", "aud": "portal-internal", "sub": "svc",
            "azp": "context-gateway", "exp": now + 60, "iat": now,
        }));
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).expect("header"),
        );
        let refused = domain_verifications(State(state), headers)
            .await
            .expect_err("the database is down");
        assert!(matches!(&refused, ApiError::Internal(m) if m.contains("were not read")));
        let response = refused.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(response.into_body(), 1 << 16)
            .await
            .expect("body");
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("127.0.0.1"), "{text}");
    }
}
