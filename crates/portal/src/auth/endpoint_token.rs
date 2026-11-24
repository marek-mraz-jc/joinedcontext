//! The person's token for one Endpoint, whichever front they came through (T-6783).
//!
//! A bearer or edge caller presents an access token; a Portal cookie session has its access
//! token kept on the server ([`AppState::keep_access_token`]). Either is exchanged at the realm
//! for a token of audience the hub and scope `endpoint:{slug}` (ADR-N-025 4b), which the gateway
//! takes for that Endpoint alone and decides on the person's own grants: a person the Policy
//! does not grant gets the gateway's 403, never a token that reaches more.

use axum::http::HeaderMap;

use crate::agents::identity::persons_token;
use crate::auth::realms::exchanger;
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
    if Front::of(headers, state.config.trust_edge_token) != Front::Portal {
        let Some(token) = persons_token(headers, state.config.trust_edge_token) else {
            return Ok(None);
        };
        return exchanger(state, token)
            .await
            .map_err(|err| err.to_string())?
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
    let exchanged = exchanger(state, &access)
        .await
        .map_err(|err| err.to_string())?
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
            stepped_up_at: None,
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

    /// T-7142: a person of an Organization's own realm has their token exchanged at that realm,
    /// on that Organization's host; the same token anywhere else goes to the installation's
    /// realm, which knows no such person, and is never sent to the Organization's.
    #[tokio::test]
    async fn an_organization_realms_token_is_exchanged_at_its_own_realm_alone() {
        use crate::auth::oidc::tests::{config_for, mock_realm};
        use crate::organization_host::{scope, OrganizationHost};
        let installation = mock_realm().await;
        let ba = mock_realm().await;
        let issuer = format!("{}/realms/bb", ba.uri());
        let host = |name: &str| OrganizationHost {
            organization: name.into(),
            base: format!("https://portal.{name}.example")
                .parse()
                .expect("url"),
        };
        let config = Config {
            oidc: Some(config_for(&installation)),
            organization_hosts: vec![host("ba"), host("hel")],
            organization_realms: vec![crate::auth::realms::RealmSetting {
                organization: "ba".into(),
                issuer: issuer.parse().expect("url"),
                jwks_url: format!("{issuer}/protocol/openid-connect/certs")
                    .parse()
                    .expect("url"),
            }],
            ..Config::for_tests()
        };
        let mut state = AppState::new(config, None);
        let _installation = realm(&mut state, 0).await;
        let person = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &json!({ "iss": issuer, "sub": "sub-ba" }),
            &jsonwebtoken::EncodingKey::from_secret(b"unverified-here"),
        )
        .expect("token");
        Mock::given(method("POST"))
            .and(path("/realms/bb/protocol/openid-connect/token"))
            .and(body_string_contains(format!("subject_token={person}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": EXCHANGED, "token_type": "Bearer", "expires_in": 300
            })))
            .expect(1)
            .mount(&ba)
            .await;
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {person}").parse().expect("header"),
        );
        let on_ba = scope(host("ba"), endpoint_token(&state, &headers, "closedslug")).await;
        assert_eq!(on_ba, Ok(Some(EXCHANGED.to_owned())));
        let on_hel = scope(host("hel"), endpoint_token(&state, &headers, "closedslug")).await;
        assert!(
            on_hel.is_err(),
            "hel's host exchanged ba's token: {on_hel:?}"
        );
        assert!(endpoint_token(&state, &headers, "closedslug")
            .await
            .is_err());
    }

    /// A session whose access token this process does not hold (a restart, another replica), a
    /// logged-out one, a revoked one and a request with no session name nobody: no exchange.
    fn bearer(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::AUTHORIZATION, value.parse().expect("header"));
        headers
    }

    /// A bearer caller's own token is exchanged for the Endpoint, every call (nothing is kept
    /// for a token the Portal does not hold); a header that is no bearer names nobody.
    #[tokio::test]
    async fn a_bearer_callers_token_is_exchanged_and_a_non_bearer_names_nobody() {
        let mut state = AppState::new(Config::for_tests(), None);
        let _realm = realm(&mut state, 2).await;
        let headers = bearer(&format!("Bearer {KEPT}"));
        for _ in 0..2 {
            assert_eq!(
                endpoint_token(&state, &headers, "closedslug").await,
                Ok(Some(EXCHANGED.to_owned()))
            );
        }
        for nobody in ["Basic cGlhOnB3", "Bearer ", "Bearer    "] {
            assert_eq!(
                endpoint_token(&state, &bearer(nobody), "closedslug").await,
                Ok(None),
                "{nobody:?}"
            );
        }
    }

    /// A refused exchange is an error naming no token, for a bearer and for a cookie session; a
    /// Portal with no client to exchange at says so.
    #[tokio::test]
    async fn a_refused_or_impossible_exchange_is_an_error_without_the_token() {
        let mut state = AppState::new(Config::for_tests(), None);
        let _realm = realm(&mut state, 0).await;
        // The realm exchanges only `KEPT`: any other token is answered 404 by the mock.
        let foreign = "foreign-access-token-5521";
        let refused = endpoint_token(&state, &bearer(&format!("Bearer {foreign}")), "closedslug")
            .await
            .expect_err("refused");
        assert!(!refused.contains(foreign), "{refused}");
        let session = signed_in();
        let headers = cookie_session(&state, &session, Some(foreign));
        let refused = endpoint_token(&state, &headers, "closedslug")
            .await
            .expect_err("refused");
        assert!(!refused.contains(foreign), "{refused}");

        state.oidc = None;
        for headers in [bearer(&format!("Bearer {KEPT}")), cookie_session(&state, &session, Some(KEPT))] {
            let error = endpoint_token(&state, &headers, "closedslug")
                .await
                .expect_err("no client");
            assert!(error.contains("no Keycloak client"), "{error}");
            assert!(!error.contains(KEPT), "{error}");
        }
    }

    /// A session whose sign-in or access token is over names nobody, even with a token kept.
    #[tokio::test]
    async fn an_expired_session_or_access_token_names_nobody() {
        let mut state = AppState::new(Config::for_tests(), None);
        let _realm = realm(&mut state, 0).await;
        let now = session::now_unix();
        let mut access_over = signed_in();
        access_over.access_expires_at = now - 1;
        let mut session_over = signed_in();
        session_over.expires_at = now - 1;
        for session in [access_over, session_over] {
            let headers = cookie_session(&state, &session, Some(KEPT));
            assert_eq!(endpoint_token(&state, &headers, "closedslug").await, Ok(None));
        }
    }

    /// An exchanged JWT is kept until 30 s before its `exp` and never past the session; one that
    /// is already inside that margin is used once and exchanged again on the next read.
    #[tokio::test]
    async fn an_exchanged_token_is_kept_by_its_exp_and_not_once_inside_the_margin() {
        use base64::Engine as _;
        let jwt = |exp: i64| {
            let b64 = |v: &serde_json::Value| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
            };
            format!("{}.{}.sig", b64(&json!({"alg": "none"})), b64(&json!({"exp": exp})))
        };
        let now = session::now_unix();
        for (exp, exchanges) in [(now + 600, 1), (now + 10, 2)] {
            let mut state = AppState::new(Config::for_tests(), None);
            let realm = crate::auth::oidc::tests::mock_realm().await;
            Mock::given(method("POST"))
                .and(path("/realms/bb/protocol/openid-connect/token"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "access_token": jwt(exp), "token_type": "Bearer", "expires_in": 300
                })))
                .expect(exchanges)
                .mount(&realm)
                .await;
            state.oidc = Some(std::sync::Arc::new(
                crate::auth::oidc::OidcClient::discover(
                    &crate::auth::oidc::tests::config_for(&realm),
                    "https://portal.test/cb",
                )
                .await
                .expect("discovery"),
            ));
            let session = signed_in();
            let headers = cookie_session(&state, &session, Some(KEPT));
            for _ in 0..2 {
                assert_eq!(
                    endpoint_token(&state, &headers, "closedslug").await,
                    Ok(Some(jwt(exp)))
                );
            }
            // Another Endpoint is its own exchange, never the first one's token.
            assert_eq!(state.session_exchanged(&session, "otherslug", now), None);
            drop(realm);
        }
    }

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
