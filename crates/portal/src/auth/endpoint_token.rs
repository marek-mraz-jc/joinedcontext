//! The person's token for one Endpoint, whichever front they came through (T-6783).
//!
//! A bearer or edge caller presents an access token; a Portal cookie session has its access
//! token kept on the server ([`AppState::keep_access_token`]). Either is exchanged at the realm
//! for a token of audience the hub and scope `endpoint:{slug}` (ADR-N-025 4b), which the gateway
//! takes for that Endpoint alone and decides on the person's own grants: a person the Policy
//! does not grant gets the gateway's 403, never a token that reaches more.

use axum::http::HeaderMap;

use crate::agents::identity::persons_token;
use crate::auth::session::{self, Front};
use crate::state::AppState;

/// How long an exchanged token is kept when its `exp` cannot be read.
const FALLBACK_LIFETIME_SECS: i64 = 60;
/// An exchanged token is used until this many seconds before it expires.
const EXPIRY_MARGIN_SECS: i64 = 30;

/// The token the person behind `headers` reads Endpoint `slug` with. `Ok(None)` when the request
/// names nobody (no token, no live cookie session, a revoked or expired one, or a session whose
/// access token this process does not hold); `Err` when the realm or the Portal's client refused
/// the exchange, with a reason that carries no token.
pub async fn endpoint_token(
    state: &AppState,
    headers: &HeaderMap,
    slug: &str,
) -> Result<Option<String>, String> {
    let oidc = state
        .oidc
        .as_ref()
        .ok_or("this Portal has no Keycloak client to read the Endpoint with")?;
    if Front::of(headers, state.config.trust_edge_token) != Front::Portal {
        let Some(token) = persons_token(headers, state.config.trust_edge_token) else {
            return Ok(None);
        };
        return oidc
            .exchange_for_endpoint(token, slug)
            .await
            .map(Some)
            .map_err(|err| err.to_string());
    }
    let now = session::now_unix();
    let Some(session) = session::load_with(headers, state) else {
        return Ok(None);
    };
    if session.is_expired(now) || session.access_expired(now) || state.is_revoked(&session) {
        return Ok(None);
    }
    if let Some(kept) = state.session_exchanged(&session, slug, now) {
        return Ok(Some(kept));
    }
    let Some(access) = state.session_access_token(&session, now) else {
        return Ok(None);
    };
    let exchanged = oidc
        .exchange_for_endpoint(&access, slug)
        .await
        .map_err(|err| err.to_string())?;
    let until = crate::auth::oidc::jwt_exp(&exchanged)
        .map_or(now + FALLBACK_LIFETIME_SECS, |exp| exp - EXPIRY_MARGIN_SECS)
        .min(session.expires_at);
    if until > now {
        state.keep_exchanged(&session, slug, exchanged.clone(), until);
    }
    Ok(Some(exchanged))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::auth::session::{Identity, Session};
    use crate::config::Config;
    use axum::response::IntoResponse;
    use axum_extra::extract::cookie::PrivateCookieJar;
    use serde_json::json;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The person's access token the Portal kept at sign-in.
    pub(crate) const KEPT: &str = "kept-person-access-7311";
    /// What the realm hands back for the Endpoint.
    pub(crate) const EXCHANGED: &str = "exchanged-for-closedslug-2208";

    pub(crate) fn signed_in() -> Session {
        let now = session::now_unix();
        Session {
            identity: Identity {
                client: None,
                subject: "sub-pia".into(),
                username: "pia".into(),
                email: None,
                name: None,
                roles: Vec::new(),
                groups: vec!["helsinki-viewers".into()],
                second_factor: false,
            },
            expires_at: now + 3600,
            issued_at: now,
            signed_in_at: None,
            id_token: "id-token".into(),
            access_expires_at: now + 300,
            refresh_token: Some("refresh".into()),
        }
    }

    /// The request headers of a browser holding `session`'s cookies, with the claims and the
    /// access token kept as a sign-in keeps them.
    pub(crate) fn cookie_session(
        state: &AppState,
        session: &Session,
        access: Option<&str>,
    ) -> HeaderMap {
        state.keep_session_claims(session);
        if let Some(access) = access {
            state.keep_access_token(session, access.to_owned());
        }
        let jar = session::store(
            PrivateCookieJar::new(state.config.cookie_key.clone()),
            session,
        )
        .expect("store");
        let response = (jar, axum::http::StatusCode::OK).into_response();
        let mut headers = HeaderMap::new();
        for value in response.headers().get_all(axum::http::header::SET_COOKIE) {
            let pair = value
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned();
            headers.append(axum::http::header::COOKIE, pair.parse().unwrap());
        }
        headers
    }

    /// A realm that exchanges only `KEPT` for `closedslug`, at most `times` times.
    pub(crate) async fn realm(state: &mut AppState, times: u64) -> MockServer {
        let realm = crate::auth::oidc::tests::mock_realm().await;
        Mock::given(method("POST"))
            .and(path("/realms/bb/protocol/openid-connect/token"))
            .and(body_string_contains(format!("subject_token={KEPT}")))
            .and(body_string_contains("endpoint%3Aclosedslug"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": EXCHANGED, "token_type": "Bearer", "expires_in": 300
            })))
            .expect(times)
            .mount(&realm)
            .await;
        let client = crate::auth::oidc::OidcClient::discover(
            &crate::auth::oidc::tests::config_for(&realm),
            "https://portal.test/cb",
        )
        .await
        .expect("discovery");
        state.oidc = Some(std::sync::Arc::new(client));
        realm
    }

    /// T-6783: a cookie session reads a closed Endpoint with its kept access token exchanged
    /// for that Endpoint, and a page's further reads reuse the exchanged token.
    #[tokio::test]
    async fn a_cookie_session_reads_an_endpoint_with_its_kept_token_exchanged_once() {
        let mut state = AppState::new(Config::for_tests(), None);
        let _realm = realm(&mut state, 1).await;
        let headers = cookie_session(&state, &signed_in(), Some(KEPT));
        for _ in 0..3 {
            let token = endpoint_token(&state, &headers, "closedslug").await;
            assert_eq!(token, Ok(Some(EXCHANGED.to_owned())));
        }
    }

    /// A session whose access token this process does not hold (a restart, another replica), a
    /// logged-out one, a revoked one and a request with no session name nobody: no exchange.
    #[tokio::test]
    async fn a_session_without_a_live_kept_token_names_nobody() {
        let mut state = AppState::new(Config::for_tests(), None);
        let _realm = realm(&mut state, 0).await;
        let session = signed_in();
        let restarted = cookie_session(&state, &session, None);
        assert_eq!(
            endpoint_token(&state, &restarted, "closedslug").await,
            Ok(None)
        );
        assert_eq!(
            endpoint_token(&state, &HeaderMap::new(), "closedslug").await,
            Ok(None)
        );

        let logged_out = cookie_session(&state, &session, Some(KEPT));
        state.take_id_token(&session);
        assert_eq!(
            endpoint_token(&state, &logged_out, "closedslug").await,
            Ok(None)
        );

        let revoked = cookie_session(&state, &session, Some(KEPT));
        state
            .revoke_subject(&session.identity.subject, session.issued_at)
            .await;
        assert_eq!(
            endpoint_token(&state, &revoked, "closedslug").await,
            Ok(None)
        );
    }
}
