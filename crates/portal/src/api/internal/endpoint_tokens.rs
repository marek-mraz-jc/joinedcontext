//! A Portal page's read of a closed Endpoint when the gateway runs in another process (T-7226).
//!
//! jc's data plane serves `/api/endpoint/{slug}` apart from the Portal, so the session cookie a
//! Portal page sends there names nobody in that process. The gateway forwards the cookie here,
//! with its own workload token, and gets the person's token for that one Endpoint
//! ([`crate::auth::endpoint_token`]): the same token the gateway takes in one jc, so the
//! person's own grants decide, 200 or 403. Only the gateway's token opens the route.

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

/// The Endpoint the page reads and the host the browser sent the read to, which picks the
/// Organization's realm as the Portal's own pages do.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ask {
    pub slug: String,
    pub host: Option<String>,
}

#[derive(Serialize)]
struct Answer {
    token: String,
}

/// `200 {"token"}` for a live session that may hold one, `204` when the cookie names nobody.
pub async fn endpoint_token_for_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    // The caller first, the body after: nobody without the gateway's token learns its shape.
    crate::auth::internal::authenticate_gateway(&state, &headers).await?;
    let ask: Ask = serde_json::from_slice(&body).map_err(|why| {
        ApiError::BadRequest(format!(
            "the body is {{\"slug\": \"<Endpoint name>\", \"host\": \"<host>\"}}: {why}"
        ))
    })?;
    let named = !ask.slug.is_empty()
        && ask.slug.len() <= 64
        && ask
            .slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !named {
        return Err(ApiError::BadRequest(
            "slug must be an Endpoint's name, e.g. air-quality".into(),
        ));
    }
    Ok(match session_token(&state, &headers, &ask).await {
        Some(token) => {
            let mut response = Json(Answer { token }).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, "no-store".parse().expect("static"));
            response
        }
        None => StatusCode::NO_CONTENT.into_response(),
    })
}

/// The token of the session in `headers`' cookie alone: the gateway's own bearer is dropped, so
/// it is never taken for the person's.
async fn session_token(state: &AppState, headers: &HeaderMap, ask: &Ask) -> Option<String> {
    let mut cookie = HeaderMap::new();
    for value in headers.get_all(header::COOKIE) {
        cookie.append(header::COOKIE, value.clone());
    }
    let pin = ask.host.as_deref().and_then(|host| {
        crate::organization_host::for_host(&state.organization_hosts.current(), host).cloned()
    });
    let asked = crate::auth::endpoint_token(state, &cookie, &ask.slug);
    let token = match pin {
        Some(pin) => crate::organization_host::scope_in(state, pin, asked).await,
        None => asked.await,
    };
    match token {
        Ok(token) => token,
        // The reason carries no token (`exchange_for_endpoint`).
        Err(reason) => {
            tracing::info!(slug = %ask.slug, %reason, "internal: no Endpoint token for the session");
            None
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/internal/endpoint-token", post(endpoint_token_for_session))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::endpoint_token::tests::{cookie_session, realm, signed_in, EXCHANGED, KEPT};
    use crate::config::Config;

    /// T-7226: the cookie the gateway forwards gets the person's token for that Endpoint, and
    /// the gateway's own bearer beside it is never taken for the person.
    #[tokio::test]
    async fn a_forwarded_cookie_gets_the_persons_token_and_the_gateways_bearer_is_dropped() {
        let mut state = AppState::new(Config::for_tests(), None);
        let _realm = realm(&mut state, 1).await;
        let mut headers = cookie_session(&state, &signed_in(), Some(KEPT));
        headers.insert(header::AUTHORIZATION, "Bearer gateway".parse().unwrap());
        let ask = Ask {
            slug: "closedslug".into(),
            host: Some("portal.test".into()),
        };
        assert_eq!(
            session_token(&state, &headers, &ask).await,
            Some(EXCHANGED.to_owned())
        );
        let mut bearer_only = HeaderMap::new();
        bearer_only.insert(
            header::AUTHORIZATION,
            format!("Bearer {KEPT}").parse().unwrap(),
        );
        assert_eq!(session_token(&state, &bearer_only, &ask).await, None);
    }

    /// T-7917: the gateway's token opens the route, and a live session's cookie gets the
    /// person's token for that one Endpoint, never cached by anyone on the way.
    #[tokio::test]
    async fn the_gateway_gets_the_persons_token_uncached() {
        use http_body_util::BodyExt;
        let mut config = Config::for_tests();
        config.gateway_client_id = Some("context-gateway".into());
        let mut state = AppState::new(config, None);
        let _realm = realm(&mut state, 1).await;
        let issuer = "https://idm.example.test/realms/dev";
        let (verifier, sign) = crate::auth::bearer::test_verifier(issuer, "portal-api", "k1");
        state.bearer = Some(std::sync::Arc::new(verifier));
        let now = crate::auth::session::now_unix();
        let gateway = sign(&serde_json::json!({
            "iss": issuer, "typ": "Bearer", "aud": crate::auth::internal::INTERNAL_AUDIENCE,
            "sub": "svc", "azp": "context-gateway", "exp": now + 60, "iat": now,
        }));
        let mut headers = cookie_session(&state, &signed_in(), Some(KEPT));
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {gateway}").parse().expect("header"),
        );
        let response = endpoint_token_for_session(
            State(state.clone()),
            headers.clone(),
            axum::body::Bytes::from_static(br#"{"slug":"closedslug"}"#),
        )
        .await
        .expect("answered");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .expect("a cache rule"),
            "no-store"
        );
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let answer: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(answer["token"], EXCHANGED);
        // The same call with a slug that is no Endpoint's name is the gateway's mistake.
        let refused = endpoint_token_for_session(
            State(state),
            headers,
            axum::body::Bytes::from_static(br#"{"slug":"Not A Slug"}"#),
        )
        .await;
        assert!(
            matches!(refused, Err(ApiError::BadRequest(_))),
            "{refused:?}"
        );
    }

    /// T-7917: a Portal that cannot exchange the session's token answers nobody a token, on an
    /// Organization's host as on the installation's.
    #[tokio::test]
    async fn a_session_that_cannot_be_exchanged_gets_no_token_pinned_or_not() {
        use crate::organization_host::OrganizationHost;
        let host = OrganizationHost {
            organization: "ba".into(),
            base: "https://portal.ba.example".parse().expect("url"),
        };
        let config = Config {
            organization_hosts: vec![host],
            ..Config::for_tests()
        };
        // No Keycloak client here: the exchange is refused with a reason, which is logged.
        let state = AppState::new(config, None);
        let headers = cookie_session(&state, &signed_in(), Some(KEPT));
        for host in [None, Some("portal.ba.example".to_owned())] {
            let ask = Ask {
                slug: "closedslug".into(),
                host: host.clone(),
            };
            assert_eq!(
                session_token(&state, &headers, &ask).await,
                None,
                "{host:?}"
            );
        }
    }
}
