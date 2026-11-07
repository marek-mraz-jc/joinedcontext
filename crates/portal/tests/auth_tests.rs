//! T-0181: Keycloak OIDC authorization code flow with PKCE and encrypted cookie sessions.
//! T-0508: the edge's `X-Access-Token` verified like a bearer, ES256 and RS256 (ADR-N-019).
//!
//! The realm is a wiremock stand-in: discovery is enough to exercise the flow start,
//! the state check and the failure paths without a live Keycloak, and its JWKS carries one
//! ES256 and one RS256 key, the way a realm with the `edge` client's override does.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Shaped like a Keycloak realm: the issuer carries the `/realms/{realm}` path.
const REALM_PATH: &str = "/realms/banskabystrica";

fn issuer_of(server: &MockServer) -> String {
    format!("{}{REALM_PATH}", server.uri().trim_end_matches('/'))
}

async fn realm() -> MockServer {
    let server = MockServer::start().await;
    let issuer = issuer_of(&server);
    Mock::given(method("GET"))
        .and(path(format!(
            "{REALM_PATH}/.well-known/openid-configuration"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "end_session_endpoint": format!("{issuer}/protocol/openid-connect/logout"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM_PATH}/protocol/openid-connect/certs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(keys::jwks()))
        .mount(&server)
        .await;
    server
}

async fn app_with_realm(server: &MockServer) -> axum::Router {
    app_behind(server, false).await
}

/// `edge` is what the deployment sets behind APISIX: `JC_TRUST_EDGE_TOKEN=true` (ADR-N-019).
async fn app_behind(server: &MockServer, edge: bool) -> axum::Router {
    server::app(state_behind(server, edge).await)
}

async fn state_behind(server: &MockServer, edge: bool) -> AppState {
    let mut config = Config::from_vars(|k| match k {
        "JC_OIDC_ISSUER" => Some(issuer_of(server)),
        "JC_OIDC_CLIENT_ID" => Some("joinedcontext-portal".to_string()),
        "JC_OIDC_CLIENT_SECRET" => Some("test-secret".to_string()),
        "JC_PORTAL_COOKIE_KEY" => Some("k".repeat(64)),
        "JC_TRUST_EDGE_TOKEN" if edge => Some("true".to_string()),
        _ => None,
    })
    .expect("config");
    config.public_base_url = "https://portal.test".parse().expect("url");
    AppState::from_config(config).await.expect("discovery")
}

/// The realm's two signing keys, generated once per test binary: ES256 for every client and
/// RS256 for the `edge` client (AP-27). Generated rather than committed: a private key literal
/// in the repository trips gitleaks, and rightly so.
mod keys {
    use std::sync::OnceLock;

    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use p256::pkcs8::EncodePrivateKey as _;
    use rsa::pkcs1::EncodeRsaPrivateKey as _;
    use rsa::traits::PublicKeyParts as _;
    use serde_json::{json, Value};

    struct Realm {
        es256: EncodingKey,
        rs256: EncodingKey,
        jwks: Value,
    }

    fn realm() -> &'static Realm {
        static REALM: OnceLock<Realm> = OnceLock::new();
        REALM.get_or_init(|| {
            let ec = p256::SecretKey::random(&mut rand_core::OsRng);
            let mut ec_jwk: Value =
                serde_json::from_str(&ec.public_key().to_jwk_string()).expect("a jwk");
            ec_jwk["kid"] = json!("realm-es256");
            ec_jwk["alg"] = json!("ES256");
            ec_jwk["use"] = json!("sig");

            let rsa = rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 2048).expect("an rsa key");
            let rsa_jwk = json!({
                "kty": "RSA",
                "kid": "realm-rs256",
                "alg": "RS256",
                "use": "sig",
                "n": URL_SAFE_NO_PAD.encode(rsa.n().to_bytes_be()),
                "e": URL_SAFE_NO_PAD.encode(rsa.e().to_bytes_be()),
            });

            Realm {
                es256: EncodingKey::from_ec_der(ec.to_pkcs8_der().expect("der").as_bytes()),
                rs256: EncodingKey::from_rsa_der(rsa.to_pkcs1_der().expect("der").as_bytes()),
                jwks: json!({ "keys": [ec_jwk, rsa_jwk] }),
            }
        })
    }

    pub fn jwks() -> Value {
        realm().jwks.clone()
    }

    fn now() -> i64 {
        joinedcontext_portal::auth::session::now_unix()
    }

    /// An access token of `issuer` for the Portal, as Keycloak would mint it for `username`.
    pub fn token(algorithm: Algorithm, issuer: &str, username: &str, expires_in: i64) -> String {
        token_of_client(algorithm, issuer, username, expires_in, None)
    }

    /// [`token`] obtained by the Keycloak client `azp`, the way every real login names one.
    pub fn token_of_client(
        algorithm: Algorithm,
        issuer: &str,
        username: &str,
        expires_in: i64,
        azp: Option<&str>,
    ) -> String {
        let (kid, key) = match algorithm {
            Algorithm::ES256 => ("realm-es256", &realm().es256),
            Algorithm::RS256 => ("realm-rs256", &realm().rs256),
            other => panic!("the realm does not sign {other:?}"),
        };
        let mut header = Header::new(algorithm);
        header.kid = Some(kid.into());
        let mut claims = json!({
            "iss": issuer,
            "typ": "Bearer",
            "aud": "joinedcontext-portal",
            "sub": format!("f:1:{username}"),
            "preferred_username": username,
            "exp": now() + expires_in,
            "iat": now(),
            "realm_access": { "roles": ["portal-viewer"] },
        });
        if let Some(azp) = azp {
            claims["azp"] = json!(azp);
        }
        encode(&header, &claims, key).expect("a signed token")
    }

    /// An RS256 access token of the `edge` login carrying `extra` claims (`acr`, `amr`).
    pub fn token_with(issuer: &str, username: &str, extra: Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("realm-rs256".into());
        let mut claims = json!({
            "iss": issuer,
            "typ": "Bearer",
            "aud": "joinedcontext-portal",
            "sub": format!("f:1:{username}"),
            "preferred_username": username,
            "exp": now() + 300,
            "iat": now(),
            "azp": "edge",
        });
        for (name, value) in extra.as_object().expect("an object") {
            claims[name] = value.clone();
        }
        encode(&header, &claims, &realm().rs256).expect("a signed token")
    }

    /// The ID token the realm returns from the token endpoint after an authorization code, with
    /// the `nonce` the Portal parked in its flow cookie — the one claim that ties the answer to
    /// the login that was started (T-1681).
    pub fn id_token(issuer: &str, username: &str, nonce: &str) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("realm-rs256".into());
        encode(
            &header,
            &json!({
                "iss": issuer,
                "typ": "ID",
                "aud": "joinedcontext-portal",
                "azp": "joinedcontext-portal",
                "sub": format!("f:1:{username}"),
                "preferred_username": username,
                "nonce": nonce,
                "exp": now() + 300,
                "iat": now(),
                "realm_access": { "roles": ["portal-viewer"] },
            }),
            &realm().rs256,
        )
        .expect("a signed id token")
    }

    /// A client-credentials token as Keycloak mints it for the service account of `client`:
    /// `azp` is the client, the user name is Keycloak's `service-account-{client}`, and — with
    /// `azp` left out — what an older or foreign issuer might send.
    pub fn service_token(issuer: &str, client: Option<&str>) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("realm-es256".into());
        let name = client.unwrap_or("anonymous-client");
        let mut claims = json!({
            "iss": issuer,
            "typ": "Bearer",
            "aud": "joinedcontext-portal",
            "sub": format!("sa:{name}"),
            "preferred_username": format!("service-account-{name}"),
            "exp": now() + 300,
            "iat": now(),
        });
        if let Some(client) = client {
            claims["azp"] = json!(client);
        }
        encode(&header, &claims, &realm().es256).expect("a signed token")
    }

    /// A back-channel logout token as the realm signs one. `event` and `nonce` are what
    /// separates it from the ID token that travels in the open as `id_token_hint`: the realm
    /// signs both with the same key, so only the claims tell them apart (AP-29).
    pub fn logout_token(issuer: &str, username: &str, event: bool, nonce: bool) -> String {
        logout_token_for(issuer, username, "joinedcontext-portal", event, nonce)
    }

    /// [`logout_token`] addressed to the client `aud`.
    pub fn logout_token_for(
        issuer: &str,
        username: &str,
        aud: &str,
        event: bool,
        nonce: bool,
    ) -> String {
        let mut claims = json!({
            "iss": issuer,
            "typ": "Logout",
            "aud": aud,
            "sub": format!("f:1:{username}"),
            "iat": now(),
            "exp": now() + 300,
            "jti": "logout-token-1",
            "sid": "session-1",
        });
        if event {
            claims["events"] = json!({ "http://schemas.openid.net/event/backchannel-logout": {} });
        }
        if nonce {
            claims["nonce"] = json!("n-1");
        }
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("realm-rs256".into());
        encode(&header, &claims, &realm().rs256).expect("a signed token")
    }
}

fn set_cookie_values(response: &axum::response::Response) -> Vec<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn login_hands_the_portal_s_language_to_the_realm_and_drops_anything_else() {
    // T-4199: a Slovak Portal led to an English sign-in page.
    let realm = realm().await;
    let app = app_with_realm(&realm).await;
    for (query, expected) in [
        ("?lang=sk", Some("ui_locales=sk")),
        ("?lang=CS&redirect_to=%2Fprojects", Some("ui_locales=cs")),
        ("?lang=sk%26prompt%3Dnone", None),
        ("?lang=s", None),
        ("", None),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/auth/login{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{query}");
        let location = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        match expected {
            Some(param) => assert!(location.contains(param), "{query}: {location}"),
            None => assert!(!location.contains("ui_locales"), "{query}: {location}"),
        }
        assert!(!location.contains("prompt=none"), "{query}: {location}");
    }
}

#[tokio::test]
async fn login_starts_a_pkce_flow_and_parks_the_verifier_in_a_secure_cookie() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(location.contains("code_challenge="), "{location}");
    assert!(
        location.contains("code_challenge_method=S256"),
        "{location}"
    );
    assert!(location.contains("state="), "{location}");
    assert!(location.contains("nonce="), "{location}");
    assert!(location.contains("scope=openid"), "{location}");
    assert!(
        location.contains("redirect_uri=https%3A%2F%2Fportal.test%2Fapi%2Fv1%2Fauth%2Fcallback"),
        "{location}"
    );

    let cookies = set_cookie_values(&response);
    let flow = cookies
        .iter()
        .find(|c| c.starts_with("__Host-jc_oidc_flow="))
        .expect("flow cookie");
    assert!(flow.contains("HttpOnly"), "{flow}");
    assert!(flow.contains("Secure"), "{flow}");
    assert!(flow.contains("SameSite=Lax"), "{flow}");
}

#[tokio::test]
async fn callback_without_a_login_in_flight_is_refused() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/callback?code=abc&state=xyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

#[tokio::test]
async fn callback_with_a_foreign_state_is_refused() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let started = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let flow_cookie = set_cookie_values(&started)
        .into_iter()
        .find(|c| c.starts_with("__Host-jc_oidc_flow="))
        .map(|c| c.split(';').next().unwrap_or_default().to_string())
        .expect("flow cookie");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/callback?code=abc&state=not-the-state-we-issued")
                .header(header::COOKIE, flow_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        problem["type"],
        "https://joinedcontext.com/errors/invalid-request"
    );
    assert!(problem["detail"].as_str().unwrap().contains("state"));
}

#[tokio::test]
async fn callback_reports_a_provider_side_error() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/callback?error=access_denied&error_description=user%20said%20no")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // F-R2POR3-014: no login of this browser is in flight, so the text is the link's, not the
    // provider's, and the Portal does not repeat it.
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(!body.contains("said no"), "{body}");
}

#[tokio::test]
async fn me_without_a_session_is_unauthorized() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

#[tokio::test]
async fn login_without_a_configured_realm_is_unavailable() {
    let app = server::app(AppState::new(Config::for_tests(), None));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn a_mutation_without_the_csrf_token_is_forbidden() {
    let app = server::app(AppState::new(Config::for_tests(), None));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

#[tokio::test]
async fn a_mutation_with_a_matching_csrf_token_passes_the_gate() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    let token = "double-submit-token";

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header(header::COOKIE, format!("__Host-jc_csrf={token}"))
                .header("x-csrf-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "logout answers once CSRF passes"
    );
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );

    // ADR-N-019, AP-29: no session cookie came with the request, and all three Portal cookies
    // are still told to go, because the edge's own logout never reaches this handler.
    let cookies = set_cookie_values(&response);
    for name in ["__Host-jc_session", "__Host-jc_refresh", "__Host-jc_csrf"] {
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with(&format!("{name}=")) && c.contains("Max-Age=0")),
            "{name} is not cleared: {cookies:?}"
        );
    }

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let target: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert!(
        target["endSessionUrl"]
            .as_str()
            .is_some_and(|u| !u.is_empty()),
        "the SPA needs somewhere to navigate: {target}"
    );
}

/// The authorization request names each scope once (the client adds `openid` itself).
#[tokio::test]
async fn the_login_asks_for_each_scope_once() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    let scope = url::Url::parse(location)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "scope")
        .map(|(_, v)| v.into_owned())
        .expect("a scope");
    assert_eq!(scope, "openid profile email", "{location}");
}

/// A token response the way Keycloak answers `grant_type=refresh_token`: the refresh token is a
/// JWT whose `exp` is the SSO session's remaining life. Nothing is signed; the portal reads the
/// refresh token's `exp` as a hint and never verifies it.
fn refresh_token_with_exp(exp: i64) -> String {
    use base64::engine::{general_purpose::URL_SAFE_NO_PAD, Engine};
    format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"HS512"}"#),
        URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp},"typ":"Refresh"}}"#).as_bytes()),
        URL_SAFE_NO_PAD.encode(b"not-a-signature"),
    )
}

/// The cookies a browser would hold after a login, with the access token `access_in` seconds
/// from expiry, encrypted with the test key `app_with_realm` configures.
fn session_cookies(access_in: i64) -> String {
    session_cookies_issued(access_in, -600)
}

/// [`session_cookies`], for a session that started `issued_ago` seconds ago. Two sessions of one
/// person that started at different moments is what a logout has to tell apart (T-1678).
fn session_cookies_issued(access_in: i64, issued_ago: i64) -> String {
    stored_session(access_in, issued_ago)
        .iter()
        .filter_map(|c| c.split(';').next().map(str::to_string))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The Portal's own cookie session of `username` after a step-up login (T-5567), as the
/// `Cookie` header a browser sends.
fn stepped_up_cookie(username: &str, second_factor: bool) -> String {
    use axum::response::IntoResponse;
    use axum_extra::extract::cookie::{Key, PrivateCookieJar};
    use joinedcontext_portal::auth::session::{now_unix, store, Identity, Session};

    let now = now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{username}"),
            username: username.into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor,
        },
        expires_at: now + 3600,
        issued_at: now - 60,
        signed_in_at: None,
        id_token: "opaque-id-token-for-tests".into(),
        access_expires_at: now + 300,
        refresh_token: None,
    };
    let jar = store(
        PrivateCookieJar::new(Key::from("k".repeat(64).as_bytes())),
        &session,
    )
    .expect("store");
    set_cookie_values(&(jar, StatusCode::OK).into_response())
        .iter()
        .filter_map(|c| c.split(';').next().map(str::to_string))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The `Set-Cookie` lines `session::store` writes for a session, attributes and all.
fn stored_session(access_in: i64, issued_ago: i64) -> Vec<String> {
    stored_session_with(access_in, issued_ago, false)
}

/// [`stored_session`] of a session that did or did not prove a second factor (T-5567).
fn stored_session_with(access_in: i64, issued_ago: i64, second_factor: bool) -> Vec<String> {
    stored(&demo_session(access_in, issued_ago, second_factor))
}

/// demo.steward's session as [`stored_session_with`] writes it.
fn demo_session(
    access_in: i64,
    issued_ago: i64,
    second_factor: bool,
) -> joinedcontext_portal::auth::session::Session {
    use joinedcontext_portal::auth::session::{now_unix, Identity, Session};

    let now = now_unix();
    Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: None,
            name: None,
            roles: vec!["portal-viewer".into()],
            groups: Vec::new(),
            second_factor,
        },
        expires_at: now + 3600,
        issued_at: now + issued_ago,
        signed_in_at: None,
        id_token: "opaque-id-token-for-tests".into(),
        access_expires_at: now + access_in,
        refresh_token: Some("opaque-refresh-token-for-tests".into()),
    }
}

/// The `Set-Cookie` lines `session::store` writes for `session` with the test key.
fn stored(session: &joinedcontext_portal::auth::session::Session) -> Vec<String> {
    use axum::response::IntoResponse;
    use axum_extra::extract::cookie::{Key, PrivateCookieJar};

    let key = Key::from("k".repeat(64).as_bytes());
    let jar = joinedcontext_portal::auth::session::store(PrivateCookieJar::new(key), session)
        .expect("store");
    let response = (jar, StatusCode::OK).into_response();
    set_cookie_values(&response)
}

async fn mount_token_endpoint(realm: &MockServer, template: ResponseTemplate, expected: u64) {
    Mock::given(method("POST"))
        .and(path(format!("{REALM_PATH}/protocol/openid-connect/token")))
        .and(wiremock::matchers::body_string_contains(
            "grant_type=refresh_token",
        ))
        .and(wiremock::matchers::body_string_contains(
            "refresh_token=opaque-refresh-token-for-tests",
        ))
        .respond_with(template)
        .expect(expected)
        .mount(realm)
        .await;
}

#[tokio::test]
async fn a_session_near_access_expiry_is_refreshed_and_both_cookies_rotate() {
    let realm = realm().await;
    let sso_ends = joinedcontext_portal::auth::session::now_unix() + 3000;
    mount_token_endpoint(
        &realm,
        ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-access-token",
            "token_type": "Bearer",
            "expires_in": 300,
            "refresh_token": refresh_token_with_exp(sso_ends),
            "refresh_expires_in": 3000,
            "session_state": "keycloak-adds-fields",
        })),
        1,
    )
    .await;
    let app = app_with_realm(&realm).await;

    // Past the access token, inside the leeway: the only thing that keeps the user signed in.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/me")
                .header(header::ACCEPT, "application/json")
                .header(header::COOKIE, session_cookies(-5))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the refreshed session serves the request"
    );
    let cookies = set_cookie_values(&response);
    let session = cookies
        .iter()
        .find(|c| c.starts_with("__Host-jc_session="))
        .expect("rotated session cookie");
    let refresh = cookies
        .iter()
        .find(|c| c.starts_with("__Host-jc_refresh="))
        .expect("rotated refresh cookie");
    for cookie in [session, refresh] {
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("Secure"), "{cookie}");
        assert!(cookie.contains("SameSite=Lax"), "{cookie}");
        assert!(!cookie.contains("Max-Age=0"), "{cookie}");
        assert!(
            cookie.len() < 4096,
            "a cookie must stay under the browser limit"
        );
    }
    // The session now lives as long as the SSO session: roughly 3000 s, not the 300 s token.
    let max_age: i64 = session
        .split(';')
        .find_map(|p| p.trim().strip_prefix("Max-Age="))
        .and_then(|v| v.parse().ok())
        .expect("max-age");
    assert!((2990..=3000).contains(&max_age), "{max_age}");

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let me: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(me["username"], "demo.steward");
}

/// T-5567: a refresh keeps the second factor the session proved, though Keycloak's refreshed ID
/// token need not repeat `amr`, and a refresh without it never grants one.
#[tokio::test]
async fn a_refresh_keeps_the_second_factor_and_never_invents_one() {
    let realm = realm().await;
    let fresh_id = keys::id_token(&issuer_of(&realm), "demo.steward", "unused");
    mount_token_endpoint(
        &realm,
        ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-access-token",
            "token_type": "Bearer",
            "expires_in": 300,
            "refresh_token": refresh_token_with_exp(joinedcontext_portal::auth::session::now_unix() + 3000),
            "id_token": fresh_id,
        })),
        2,
    )
    .await;
    let app = app_with_realm(&realm).await;
    for proved in [true, false] {
        let cookies = stored_session_with(-5, -600, proved)
            .iter()
            .filter_map(|c| c.split(';').next().map(str::to_string))
            .collect::<Vec<_>>()
            .join("; ");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/me")
                    .header(header::ACCEPT, "application/json")
                    .header(header::COOKIE, cookies)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            set_cookie_values(&response)
                .iter()
                .any(|c| c.starts_with("__Host-jc_session=")),
            "the session was refreshed"
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let me: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(me["second_factor"], json!(proved), "{me}");
    }
}

#[tokio::test]
async fn a_refused_refresh_ends_the_session_with_401_for_fetch_and_login_for_navigation() {
    let realm = realm().await;
    mount_token_endpoint(
        &realm,
        ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "Session not active"
        })),
        2,
    )
    .await;
    let app = app_with_realm(&realm).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/me")
                .header(header::ACCEPT, "application/json")
                .header(header::COOKIE, session_cookies(-5))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let cookies = set_cookie_values(&response);
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("__Host-jc_session=") && c.contains("Max-Age=0")),
        "the session cookie is cleared: {cookies:?}"
    );
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("__Host-jc_refresh=") && c.contains("Max-Age=0")),
        "the refresh cookie is cleared: {cookies:?}"
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects/helsinki/spaces?page=2")
                .header(header::ACCEPT, "text/html,application/xhtml+xml")
                .header(header::COOKIE, session_cookies(-5))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "/login?redirect_to=%2Fprojects%2Fhelsinki%2Fspaces%3Fpage%3D2"
    );
}

#[tokio::test]
async fn a_refused_refresh_inside_the_leeway_lets_the_live_token_serve_the_request() {
    let realm = realm().await;
    mount_token_endpoint(
        &realm,
        ResponseTemplate::new(400).set_body_json(json!({ "error": "invalid_grant" })),
        1,
    )
    .await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/me")
                .header(header::COOKIE, session_cookies(30))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        set_cookie_values(&response).is_empty(),
        "nothing rotates and nothing is cleared while the access token still stands"
    );
}

async fn me_with(app: axum::Router, headers: &[(&str, String)]) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .uri("/api/v1/auth/me")
        .header(header::ACCEPT, "application/json");
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(json!(null)))
}

/// T-0621, AP-28, PF-50: an edge session never passes the Portal's callback, so `/auth/me` is
/// where its double-submit cookie comes from; a session that has one, or a Portal session, gets none.
#[tokio::test]
async fn me_issues_the_csrf_cookie_to_an_edge_session_that_lacks_one() {
    let realm = realm().await;
    let app = app_behind(&realm, true).await;
    let token = keys::token(
        jsonwebtoken::Algorithm::RS256,
        &issuer_of(&realm),
        "demo.steward",
        300,
    );
    let me = |headers: Vec<(&'static str, String)>| {
        let app = app.clone();
        async move {
            let mut request = Request::builder().uri("/api/v1/auth/me");
            for (name, value) in headers {
                request = request.header(name, value);
            }
            app.oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap()
        }
    };

    let response = me(vec![("x-access-token", token.clone())]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = set_cookie_values(&response);
    let csrf = cookies
        .iter()
        .find(|c| c.starts_with("__Host-jc_csrf="))
        .unwrap_or_else(|| panic!("an edge session is handed the CSRF cookie: {cookies:?}"));
    assert!(csrf.contains("Secure") && csrf.contains("Path=/"), "{csrf}");
    assert!(!csrf.contains("HttpOnly"), "the UI must read it: {csrf}");
    let value = csrf.split(';').next().unwrap().to_string();

    let response = me(vec![("x-access-token", token.clone()), ("cookie", value)]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        set_cookie_values(&response).is_empty(),
        "a session that has the cookie keeps it"
    );

    let response = me(vec![("authorization", format!("Bearer {token}"))]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        set_cookie_values(&response).is_empty(),
        "a bearer caller has no page to echo a cookie from"
    );
}

/// ADR-N-019, AP-28: behind the edge the user's token arrives as `X-Access-Token` and is
/// verified exactly as `Authorization: Bearer` would be; `me` says which front it came through.
#[tokio::test]
async fn the_edge_token_is_verified_like_a_bearer_when_the_deployment_trusts_the_edge() {
    let realm = realm().await;
    let app = app_behind(&realm, true).await;
    let token = keys::token(
        jsonwebtoken::Algorithm::RS256,
        &issuer_of(&realm),
        "demo.steward",
        300,
    );

    let (status, me) = me_with(app.clone(), &[("x-access-token", token.clone())]).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["username"], "demo.steward");
    assert_eq!(me["subject"], "f:1:demo.steward");
    assert_eq!(me["roles"], json!(["portal-viewer"]));
    assert_eq!(me["front"], "edge", "the UI routes logout by this: {me}");

    // The same token as a bearer is the same person through the other front.
    let (status, me) = me_with(app.clone(), &[("authorization", format!("Bearer {token}"))]).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["front"], "bearer");

    // A protected resource route takes the edge token through the same extractor.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects")
                .header("x-access-token", &token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "the edge token authenticates every protected route, not only /me"
    );
}

/// The edge's token is the person the APISIX plugin logged in: its `azp` is that login, not a
/// workload, so a route that refuses workload tokens (the filter preview) serves the person. The
/// same token as a bearer keeps its client.
#[tokio::test]
async fn an_edge_session_is_a_person_and_a_bearer_keeps_its_client() {
    use axum::extract::FromRequestParts;
    use joinedcontext_portal::auth::session::CurrentUser;

    let realm = realm().await;
    let state = state_behind(&realm, true).await;
    let token = keys::token_of_client(
        jsonwebtoken::Algorithm::RS256,
        &issuer_of(&realm),
        "demo.steward",
        300,
        Some("edge"),
    );
    let client_of = |name: &'static str, value: String| {
        let state = state.clone();
        async move {
            let (mut parts, _) = Request::builder()
                .uri("/api/v1/me")
                .header(name, value)
                .body(())
                .unwrap()
                .into_parts();
            CurrentUser::from_request_parts(&mut parts, &state)
                .await
                .unwrap_or_else(|_| panic!("{name} authenticates"))
                .0
                .identity
                .client
        }
    };

    assert_eq!(client_of("x-access-token", token.clone()).await, None);
    assert_eq!(
        client_of("authorization", format!("Bearer {token}"))
            .await
            .as_deref(),
        Some("edge")
    );
}

/// T-5610 (AG-63, AG-82): the token an MCP connector holds for a person writes nothing over REST,
/// one request per write family, and the refusal names the MCP route; it still reads, and the
/// same request with a ServiceAccount's token is not refused for its client.
#[tokio::test]
async fn a_connector_token_is_refused_on_rest_writes_and_pointed_at_the_mcp_route() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;
    let bearer = |azp: &str| {
        format!(
            "Bearer {}",
            keys::token_of_client(
                jsonwebtoken::Algorithm::ES256,
                &issuer_of(&realm),
                "demo.editor",
                300,
                Some(azp),
            )
        )
    };
    let send = |method: &'static str, uri: &'static str, authorization: String| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header(header::AUTHORIZATION, authorization)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let body = response.into_body().collect().await.unwrap().to_bytes();
            (status, String::from_utf8_lossy(&body).into_owned())
        }
    };
    let writes = [
        ("POST", "/api/v1/projects/helsinki/workspaces"),
        (
            "POST",
            "/api/v1/projects/helsinki/workspaces/bike-lanes/propose",
        ),
        ("DELETE", "/api/v1/projects/helsinki/workspaces/bike-lanes"),
        ("POST", "/api/v1/projects/helsinki/endpoints"),
        ("PUT", "/api/v1/projects/helsinki/endpoints/air"),
        ("PATCH", "/api/v1/projects/helsinki/endpoints/air"),
        ("DELETE", "/api/v1/projects/helsinki/endpoints/air"),
        ("POST", "/api/v1/projects/helsinki/ops/jc_change_approve"),
        (
            "POST",
            "/api/v1/projects/helsinki/access-requests/ar-1/approve",
        ),
    ];
    for client in ["mcp-mobile", "mcp-hub", "mcp-local"] {
        for (method, uri) in writes {
            let (status, body) = send(method, uri, bearer(client)).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {uri} as {client}: {body}"
            );
            assert!(body.contains("/api/v1/mcp"), "{method} {uri}: {body}");
        }
    }
    for (method, uri) in writes {
        let (status, body) = send(method, uri, bearer("pipeline-runner")).await;
        assert!(
            !body.contains("/api/v1/mcp"),
            "{method} {uri} refused a ServiceAccount as a connector: {status} {body}"
        );
    }
    let (status, body) = send("GET", "/api/v1/projects", bearer("mcp-mobile")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a connector token still reads: {body}"
    );
}

/// ADR-N-019 §3.4: a Portal without APISIX in front never trusts the header, whatever it says.
#[tokio::test]
async fn the_edge_header_is_ignored_when_the_deployment_does_not_trust_the_edge() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;
    let token = keys::token(
        jsonwebtoken::Algorithm::RS256,
        &issuer_of(&realm),
        "demo.steward",
        300,
    );

    let (status, body) = me_with(app.clone(), &[("x-access-token", token.clone())]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    // The flag is about the header, not the algorithm: the same token is still a good bearer.
    let (status, me) = me_with(app, &[("authorization", format!("Bearer {token}"))]).await;
    assert_eq!(status, StatusCode::OK, "{me}");
}

/// AP-27: the `edge` client signs RS256 by per-client override while the realm stays ES256, so
/// the verifier takes both and nothing else.
#[tokio::test]
async fn rs256_and_es256_tokens_are_both_accepted_and_hs256_is_not() {
    let realm = realm().await;
    let app = app_behind(&realm, true).await;
    for algorithm in [
        jsonwebtoken::Algorithm::ES256,
        jsonwebtoken::Algorithm::RS256,
    ] {
        let token = keys::token(algorithm, &issuer_of(&realm), "demo.viewer", 300);
        let (status, me) = me_with(app.clone(), &[("x-access-token", token)]).await;
        assert_eq!(status, StatusCode::OK, "{algorithm:?}: {me}");
        assert_eq!(me["username"], "demo.viewer");
    }

    // A token signed with a shared secret names one of the realm's kids and is still refused.
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    header.kid = Some("realm-rs256".into());
    let forged = jsonwebtoken::encode(
        &header,
        &json!({
            "iss": issuer_of(&realm), "aud": "joinedcontext-portal", "sub": "f:1:mallory",
            "exp": joinedcontext_portal::auth::session::now_unix() + 300,
        }),
        &jsonwebtoken::EncodingKey::from_secret(b"guessable"),
    )
    .unwrap();
    let (status, _) = me_with(app, &[("x-access-token", forged)]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// A bad edge token is 401 like a bad bearer, and never a look at the cookie beside it.
#[tokio::test]
async fn a_bad_edge_token_is_refused_and_does_not_fall_back_to_the_cookie() {
    let realm = realm().await;
    let app = app_behind(&realm, true).await;
    let expired = keys::token(
        jsonwebtoken::Algorithm::RS256,
        &issuer_of(&realm),
        "demo.steward",
        -300,
    );

    for bad in [expired, "not-a-jwt".to_string(), String::new()] {
        let (status, body) = me_with(
            app.clone(),
            &[
                ("x-access-token", bad.clone()),
                ("cookie", session_cookies(300)),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{bad:?}: {body}");
    }
}

/// The Portal's own cookie session is the `portal` front, and `me` says so.
#[tokio::test]
async fn a_cookie_session_reports_the_portal_front() {
    let realm = realm().await;
    let app = app_behind(&realm, true).await;

    let (status, me) = me_with(app, &[("cookie", session_cookies(300))]).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["username"], "demo.steward");
    assert_eq!(me["front"], "portal");
}

/// The provider's call carries no cookie, so it carries no CSRF token either. Before T-0803 the
/// guard answered it 403 and the sessions it named stayed valid (AP-29, CC-40).
async fn back_channel_logout(app: axum::Router, token: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/backchannel-logout")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                // A JWT is base64url and dots, so it is its own form value.
                .body(Body::from(format!("logout_token={token}")))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn a_logout_from_the_provider_ends_the_sessions_it_names() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let (status, me) = me_with(app.clone(), &[("cookie", session_cookies(300))]).await;
    assert_eq!(status, StatusCode::OK, "the session stands before: {me}");

    let token = keys::logout_token(&issuer_of(&realm), "demo.steward", true, false);
    let (status, body) = back_channel_logout(app.clone(), &token).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (status, me) = me_with(app, &[("cookie", session_cookies(300))]).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the session the logout named is still answered: {me}"
    );
}

/// T-5568: a person signed in at the edge has the edge client's session, so Keycloak addresses
/// the logout token to `edge`. Behind the edge it ends their session here; without one, a token
/// for another client verifies nothing.
#[tokio::test]
async fn a_logout_addressed_to_the_edge_client_ends_the_session_behind_the_edge() {
    let realm = realm().await;
    let issuer = issuer_of(&realm);

    let alone = app_with_realm(&realm).await;
    let token = keys::logout_token_for(&issuer, "demo.steward", "edge", true, false);
    let (status, body) = back_channel_logout(alone, &token).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let app = app_behind(&realm, true).await;
    let other = keys::logout_token_for(&issuer, "demo.steward", "grafana", true, false);
    let (status, body) = back_channel_logout(app.clone(), &other).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, me) = me_with(app.clone(), &[("cookie", session_cookies(300))]).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "another client's token ended nothing: {me}"
    );

    let (status, body) = back_channel_logout(app.clone(), &token).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, me) = me_with(app, &[("cookie", session_cookies(300))]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{me}");
}

#[tokio::test]
async fn only_a_token_saying_it_ends_a_session_is_taken_for_one() {
    let realm = realm().await;
    let issuer = issuer_of(&realm);
    let app = app_with_realm(&realm).await;

    // An ID token of this client verifies just as well, and travels in the open as
    // `id_token_hint`: without the event and the nonce rule its holder could end the session.
    let refused = [
        (
            "no event",
            keys::logout_token(&issuer, "demo.steward", false, false),
        ),
        (
            "a nonce",
            keys::logout_token(&issuer, "demo.steward", true, true),
        ),
        ("not a jwt", "not-a-jwt".to_owned()),
        ("nothing at all", String::new()),
    ];
    for (why, token) in refused {
        let (status, body) = back_channel_logout(app.clone(), &token).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
        assert!(
            !body.contains(&token) || token.is_empty(),
            "{why}: the refusal repeats the token: {body}"
        );
    }

    let (status, me) = me_with(app, &[("cookie", session_cookies(300))]).await;
    assert_eq!(status, StatusCode::OK, "no refusal revoked anything: {me}");
}

/// OPS-48, T-1402: the activity feed is the audit trail, so only the collector's own Keycloak
/// client appends to it. Every workload's token carries the Portal's audience; the client that
/// obtained it (`azp`) is what tells the collector from a department's ServiceAccount, and a
/// user name shaped like the collector's is not that.
#[tokio::test]
async fn only_the_collector_s_client_appends_to_the_activity_feed() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;
    let issuer = issuer_of(&realm);
    let post = |token: String| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/activity")
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(json!({ "resourceLogs": [] }).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let body = response.into_body().collect().await.unwrap().to_bytes();
            (status, String::from_utf8_lossy(&body).into_owned())
        }
    };

    let (status, body) = post(keys::service_token(&issuer, Some("activity-ingest"))).await;
    assert_eq!(status, StatusCode::OK, "the collector is let in: {body}");

    for (who, token) in [
        (
            "a department's service account",
            keys::service_token(&issuer, Some("helsinki-sensors")),
        ),
        (
            "a token that names no client",
            keys::service_token(&issuer, None),
        ),
        (
            "a person's token",
            keys::token(
                jsonwebtoken::Algorithm::ES256,
                &issuer,
                "service-account-activity-ingest",
                300,
            ),
        ),
    ] {
        let (status, body) = post(token).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}: {body}");
        assert!(
            body.contains("activity-ingest"),
            "{who}: the refusal names whose route it is: {body}"
        );
    }
}

// =================================================================================================
// Edge cases of the five auth doors (T-2058 … T-2062; AP-28, AP-29, PF-57, R20, T-2290)
//
// The happy paths and the plain refusals are above. What follows is what they leave: the return
// path never leaving the Portal, the cookies a refused callback does not mint, the token `me` never
// carries, the logout of a caller who has no session, and the logout token of somebody else.
// The grammar of a safe return target is a unit test beside `safe_redirect` itself
// (`src/auth/oidc.rs:724`, T-2290), so it is not repeated here.
// =================================================================================================

/// The one place a `redirect_to` may be read from is the flow cookie, which is encrypted: T-2058.
///
/// The return path is a caller's own input, and the authorization request goes to the identity
/// provider — which logs it, and which a person can read off their address bar. So the path is parked
/// in the private cookie and never in the URL, and two logins started from two tabs each park their
/// own: the flow cookie is replaced, not added to.
#[tokio::test]
async fn login_parks_the_return_path_in_the_cookie_and_never_in_the_url() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let start = |query: &'static str| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/auth/login{query}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            (status, location, set_cookie_values(&response))
        }
    };

    for query in [
        "?redirect_to=/projects/secret-project/spaces",
        "?redirect_to=//evil.example/secret-project",
        "?redirect_to=https://evil.example",
        "?redirect_to=",
        "?unknown=1",
        "",
    ] {
        let (status, location, cookies) = start(query).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{query}");
        assert!(
            location.contains("/protocol/openid-connect/auth"),
            "{query}: {location}",
        );
        // Nothing of the caller's input reaches the provider, and neither does the parameter's name.
        for leaked in ["secret-project", "evil.example", "redirect_to"] {
            assert!(
                !location.contains(leaked),
                "{query} sent {leaked:?} to the provider: {location}",
            );
        }
        // PKCE and both one-time values are in the request the browser is sent.
        for required in [
            "code_challenge=",
            "code_challenge_method=S256",
            "state=",
            "nonce=",
        ] {
            assert!(
                location.contains(required),
                "{query} lacks {required}: {location}"
            );
        }
        // The flow cookie is the browser's only copy, and it is not readable by a script.
        let flow = cookies
            .iter()
            .find(|cookie| cookie.starts_with("__Host-jc_oidc_flow="))
            .unwrap_or_else(|| panic!("{query}: no flow cookie: {cookies:?}"));
        for attribute in [
            "HttpOnly",
            "Secure",
            "SameSite=Lax",
            "Path=/",
            "Max-Age=600",
        ] {
            assert!(
                flow.contains(attribute),
                "{query}: {flow} lacks {attribute}"
            );
        }
        assert!(
            !flow.contains("secret-project") && !flow.contains("evil.example"),
            "{query}: the flow cookie is not encrypted: {flow}",
        );
    }

    // A parameter given twice is refused rather than one of the two silently winning: there is no
    // reading of `?redirect_to=/a&redirect_to=/b` that both a proxy and the Portal would agree on.
    let (status, location, cookies) = start("?redirect_to=/a&redirect_to=/b").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{location}");
    assert!(
        cookies
            .iter()
            .all(|cookie| !cookie.starts_with("__Host-jc_oidc_flow=")),
        "a refused login parked a flow: {cookies:?}",
    );

    // Two logins in a row: each parks its own flow, so the second tab's state is the one that
    // matches at the callback and the first tab's is spent.
    let (_, first_location, first_cookies) = start("?redirect_to=/a").await;
    let (_, second_location, second_cookies) = start("?redirect_to=/b").await;
    assert_ne!(
        first_location, second_location,
        "two logins reused one state and one nonce",
    );
    let flow_of = |cookies: &[String]| {
        cookies
            .iter()
            .find(|cookie| cookie.starts_with("__Host-jc_oidc_flow="))
            .cloned()
            .unwrap_or_default()
    };
    assert_ne!(
        flow_of(&first_cookies),
        flow_of(&second_cookies),
        "the second login parked the first login's flow",
    );
}

/// A callback that is refused mints nothing: T-2059.
///
/// Every way of failing — no login in flight, a state that is not the one started, a login the
/// provider refused, a code with no state, a state with no code — answers problem+json and leaves the
/// caller exactly as anonymous as they were. The provider's `error_description` is echoed only to
/// the login in flight, because a person needs the provider's own text; this case also proves a
/// description reaches no header and no markup.
#[tokio::test]
async fn a_refused_callback_never_mints_a_session() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let refused: Vec<(&str, String)> = vec![
        ("no login in flight", "?code=c&state=s".to_owned()),
        ("a code and no state", "?code=c".to_owned()),
        ("a state and no code", "?state=s".to_owned()),
        ("neither", String::new()),
        (
            "a login the provider refused",
            "?error=access_denied&error_description=the%20person%20said%20no".to_owned(),
        ),
        (
            "a description that would split a header",
            "?error=access_denied&error_description=no%0d%0aSet-Cookie:%20jc_session=forged"
                .to_owned(),
        ),
        (
            "a description that would be markup",
            "?error=access_denied&error_description=%3Cscript%3Ealert(1)%3C/script%3E".to_owned(),
        ),
    ];

    for (what, query) in refused {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/auth/callback{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{what}");
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json",
            "{what}",
        );
        // Nothing of the provider's text became a header of its own.
        assert!(
            response.headers().get("set-cookie").is_none()
                || set_cookie_values(&response)
                    .iter()
                    .all(|cookie| !cookie.starts_with("__Host-jc_session=")),
            "{what} minted a session: {:?}",
            set_cookie_values(&response),
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let problem: serde_json::Value = serde_json::from_slice(&body).expect("problem+json");
        assert!(
            problem["detail"].is_string(),
            "{what}: the detail is not one string: {problem}",
        );
        let raw = String::from_utf8_lossy(&body);
        assert!(
            !raw.contains('\r') && !raw.contains('\n'),
            "{what}: the body carries a raw line break: {raw}",
        );
    }
}

/// `/auth/me` answers who, and never with what: T-2060.
///
/// The session cookie holds the ID token and the refresh token, because the Portal needs them to
/// refresh and to hint a logout. Neither belongs in an answer a page reads, so this case names them:
/// the body is the identity and the front, and nothing else.
#[tokio::test]
async fn me_answers_the_identity_and_never_a_token() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/me")
                .header(header::COOKIE, session_cookies(300))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // A read needs no double-submit token, and answering one does not mint a cookie for a Portal
    // session — that is the edge session's case, above.
    assert!(
        set_cookie_values(&response)
            .iter()
            .all(|cookie| !cookie.starts_with("__Host-jc_csrf=")),
        "a Portal session was issued a new CSRF cookie: {:?}",
        set_cookie_values(&response),
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let me: serde_json::Value = serde_json::from_slice(&body).expect("an identity");
    assert_eq!(me["username"], "demo.steward");
    assert_eq!(me["front"], "portal");
    assert_eq!(me["roles"], json!(["portal-viewer"]));
    let raw = String::from_utf8_lossy(&body);
    for leaked in [
        "opaque-id-token-for-tests",
        "opaque-refresh-token-for-tests",
        "idToken",
        "id_token",
        "refresh",
        "expiresAt",
    ] {
        assert!(
            !raw.contains(leaked),
            "the answer carries {leaked:?}: {raw}"
        );
    }
}

/// Logging out is not an authenticated route, and that is on purpose: T-2061.
///
/// Behind the edge the Portal's cookies must go whether or not this Portal can still read a session,
/// so `logout` asks for no `CurrentUser` and clears all three either way (ADR-N-019, AP-29). What it
/// must not do is hint a logout it cannot prove: `id_token_hint` is only there when a session was
/// loadable, so an anonymous caller cannot make the Portal quote somebody else's token.
#[tokio::test]
async fn logout_clears_the_cookies_without_a_session_and_hints_at_no_token() {
    let realm = realm().await;
    let token = "double-submit-token";

    // The ID token stays on the server (T-6549): the Portal that signed this session in kept it.
    let signed_in = demo_session(300, -600, false);
    let out = |cookie: String| {
        let realm_ref = &realm;
        let signed_in = &signed_in;
        async move {
            let state = state_behind(realm_ref, false).await;
            state.keep_session_claims(signed_in);
            let app = server::app(state);
            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/auth/logout")
                        .header(header::COOKIE, cookie)
                        .header("x-csrf-token", token)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let cookies = set_cookie_values(&response);
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let target: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!(null));
            (status, cookies, target)
        }
    };

    // Nobody signed in: the cookies still go, and the end-session URL hints at no token.
    let (status, cookies, target) = out(format!("__Host-jc_csrf={token}")).await;
    assert_eq!(status, StatusCode::OK, "{target}");
    for name in ["__Host-jc_session", "__Host-jc_refresh", "__Host-jc_csrf"] {
        assert!(
            cookies
                .iter()
                .any(|cookie| cookie.starts_with(&format!("{name}="))
                    && cookie.contains("Max-Age=0")),
            "{name} is not cleared: {cookies:?}",
        );
    }
    let url = target["endSessionUrl"].as_str().unwrap_or_default();
    assert!(url.contains("post_logout_redirect_uri="), "{url}");
    assert!(url.contains("client_id=joinedcontext-portal"), "{url}");
    assert!(
        !url.contains("id_token_hint"),
        "an anonymous logout quoted a token: {url}",
    );

    // Signed in: the same, with the hint the provider needs to end the right SSO session.
    let cookie = stored(&signed_in)
        .iter()
        .filter_map(|c| c.split(';').next().map(str::to_string))
        .collect::<Vec<_>>()
        .join("; ");
    let (status, _cookies, target) = out(format!("{cookie}; __Host-jc_csrf={token}")).await;
    assert_eq!(status, StatusCode::OK, "{target}");
    let url = target["endSessionUrl"].as_str().unwrap_or_default();
    assert!(
        url.contains("id_token_hint=opaque-id-token-for-tests"),
        "{url}",
    );

    // And with no provider configured at all, the SPA is sent back to the Portal rather than nowhere.
    let app = server::app(AppState::new(Config::for_tests(), None));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header(header::COOKIE, format!("__Host-jc_csrf={token}"))
                .header("x-csrf-token", token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let target: serde_json::Value = serde_json::from_slice(&body).expect("a target");
    assert!(
        target["endSessionUrl"]
            .as_str()
            .is_some_and(|url| url.starts_with("http")),
        "{target}",
    );
}

/// A back-channel logout ends the session it names and no other: T-2062.
///
/// The token is the whole authentication of that door, so this case is about the ways one can fail to
/// be the right token: a signature nobody made, the logout event under another name, an `events`
/// member that is not an object, a body that is not a form, and a token for somebody else's subject.
/// Every one of them leaves the live session standing.
#[tokio::test]
async fn a_logout_token_that_is_not_this_sessions_leaves_it_standing() {
    let realm = realm().await;
    let issuer = issuer_of(&realm);
    let app = app_with_realm(&realm).await;

    // A forged signature on an otherwise perfect logout token. The first base64url character holds
    // the top six bits of the signature's first byte, so changing it always changes the signature;
    // replacing the first `a` did not when the signature held none, or only in its last
    // character, whose low bits are padding (T-3568).
    let good = keys::logout_token(&issuer, "demo.steward", true, false);
    let parts: Vec<&str> = good.split('.').collect();
    let first = if parts[2].starts_with('A') { "B" } else { "A" };
    let tampered = format!("{}.{}.{first}{}", parts[0], parts[1], &parts[2][1..]);
    assert_ne!(tampered, good);

    for (why, token) in [
        ("a signature nobody made", tampered),
        // Signed by the realm and verifying perfectly; only `ends_a_session` can refuse it.
        (
            "no logout event",
            keys::logout_token(&issuer, "demo.steward", false, false),
        ),
        ("only dots", "..".to_owned()),
        (
            "a bearer instead of a logout token",
            keys::token(jsonwebtoken::Algorithm::RS256, &issuer, "demo.steward", 300),
        ),
    ] {
        let (status, body) = back_channel_logout(app.clone(), &token).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
    }

    // A body that is not a form at all, and one without the field.
    for (why, content_type, body) in [
        ("json", "application/json", r#"{"logout_token":"x"}"#),
        ("an empty form", "application/x-www-form-urlencoded", ""),
        (
            "another field",
            "application/x-www-form-urlencoded",
            "token=x",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/backchannel-logout")
                    .header(header::CONTENT_TYPE, content_type)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            response.status().is_client_error(),
            "{why} answered {}",
            response.status(),
        );
    }

    let (status, me) = me_with(app.clone(), &[("cookie", session_cookies(300))]).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a refusal revoked the session: {me}"
    );

    // A perfectly good logout token for another subject: accepted, and it ends that subject's
    // sessions rather than this one's.
    let elsewhere = keys::logout_token(&issuer, "someone.else", true, false);
    let (status, body) = back_channel_logout(app.clone(), &elsewhere).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, me) = me_with(app.clone(), &[("cookie", session_cookies(300))]).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "another subject's logout ended this session: {me}",
    );

    // And this subject's own ends it.
    let (status, body) = back_channel_logout(app.clone(), &good).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, me) = me_with(app, &[("cookie", session_cookies(300))]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{me}");
}

/// With no identity provider configured the two token doors are shut before the token is read, so an
/// installation that is not wired up is not a place to test whether a token is well formed (T-2062).
#[tokio::test]
async fn without_a_provider_the_token_doors_are_shut_before_the_token_is_read() {
    for (uri, body) in [
        ("/api/v1/auth/backchannel-logout", "logout_token=not-a-jwt"),
        ("/api/v1/auth/backchannel-logout", "logout_token="),
    ] {
        let app = server::app(AppState::new(Config::for_tests(), None));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{uri} with {body:?}",
        );
    }

    // The callback is the same: no provider, no exchange, whatever the query says.
    let app = server::app(AppState::new(Config::for_tests(), None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/callback?code=c&state=s")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

// -------------------------------------------------------------------------------------------
// T-1678, PF-46 — attack vector: session fixation, theft and logout.
//
// A portal session is an encrypted cookie and nothing server-side, so clearing it tells *this*
// browser to forget it and says nothing to a copy taken off the wire, out of a shared machine or
// out of a backup. `POST /api/v1/auth/logout` therefore marks the session revoked as well as
// clearing the cookies, and `AppState::is_revoked` is consulted on every request
// (`auth::session::CurrentUser`), which is what makes "the session ends at logout" true for the
// copy too. The cookie's own flags are asserted in `auth::session`'s unit tests
// (`every_portal_cookie_carries_the_same_flags_and_never_a_negative_age`) and the provider's
// back channel in `a_logout_from_the_provider_ends_the_sessions_it_names`; neither is repeated.
// -------------------------------------------------------------------------------------------

/// The `x-csrf-token` value the logout below double-submits.
const LOGOUT_CSRF: &str = "logout-double-submit-token";

async fn logout_with(app: axum::Router, cookies: &str) -> StatusCode {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/v1/auth/logout")
            .header(
                header::COOKIE,
                format!("{cookies}; __Host-jc_csrf={LOGOUT_CSRF}"),
            )
            .header("x-csrf-token", LOGOUT_CSRF)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
    .status()
}

/// PF-46: a stolen copy of the session cookie stops working the moment its owner logs out.
#[tokio::test]
async fn a_session_cookie_replayed_after_a_logout_is_no_longer_a_session() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;
    let stolen = session_cookies(300);

    // The thief holds a working copy.
    let (before, identity) = me_with(app.clone(), &[("cookie", stolen.clone())]).await;
    assert_eq!(before, StatusCode::OK, "{identity}");

    assert_eq!(logout_with(app.clone(), &stolen).await, StatusCode::OK);

    // The same bytes, on the same Portal, after the owner logged out.
    let (after, problem) = me_with(app.clone(), &[("cookie", stolen.clone())]).await;
    assert_eq!(
        after,
        StatusCode::UNAUTHORIZED,
        "a copy of the cookie outlived the logout: {problem}",
    );
    // And it stays refused: the mark is not spent by reading it.
    let (again, _) = me_with(app, &[("cookie", stolen)]).await;
    assert_eq!(again, StatusCode::UNAUTHORIZED);
}

/// PF-46: logging out of one browser does not log the person out of the other one.
///
/// The mark is placed at the session's own `issued_at`, so it reaches this session and every
/// older one and stops there. Ending the sessions on other devices is the SSO logout at
/// `endSessionUrl`, which comes back as a back-channel logout and marks the subject at `now` —
/// a different door, with the provider's signature on it.
#[tokio::test]
async fn logging_out_of_one_browser_leaves_a_session_started_later_standing() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;
    let older = session_cookies_issued(300, -600);
    let newer = session_cookies_issued(300, -1);

    assert_eq!(logout_with(app.clone(), &older).await, StatusCode::OK);

    let (older_status, _) = me_with(app.clone(), &[("cookie", older)]).await;
    assert_eq!(older_status, StatusCode::UNAUTHORIZED);
    let (newer_status, identity) = me_with(app, &[("cookie", newer)]).await;
    assert_eq!(
        newer_status,
        StatusCode::OK,
        "the other browser was signed out too: {identity}",
    );
}

/// PF-46: an anonymous logout marks nobody, so it is not a way to sign other people out.
///
/// The route is behind the CSRF gate, so a page on another origin cannot reach it at all; this is
/// the layer under that — even reached, a logout with no session of its own revokes nothing.
#[tokio::test]
async fn a_logout_without_a_session_ends_nobody_elses() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;
    let live = session_cookies(300);

    assert_eq!(logout_with(app.clone(), "").await, StatusCode::OK);

    let (status, identity) = me_with(app, &[("cookie", live)]).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an anonymous logout ended a live session: {identity}",
    );
}

/// `value` with its middle character swapped for another base64url character, so the bytes it
/// decodes to differ.
fn tamper_middle(value: &str) -> String {
    let mut chars: Vec<char> = value.chars().collect();
    let middle = chars.len() / 2;
    chars[middle] = if chars[middle] == 'A' { 'B' } else { 'A' };
    chars.into_iter().collect()
}

/// PF-46: a cookie presented before there was a login never becomes one.
///
/// Session fixation is handing somebody a session identifier and waiting for them to sign in
/// under it. There is no identifier to hand over here — the cookie *is* the session, sealed with
/// a key only the Portal holds — and this is the case that says so: every shape an attacker can
/// put in front of `__Host-jc_session` is unreadable, and none of them is treated as a session.
#[tokio::test]
async fn a_cookie_planted_before_a_login_is_never_a_session() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let honest = session_cookies(300);
    let sealed = honest
        .split("__Host-jc_session=")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .expect("the session cookie")
        .to_owned();

    for planted in [
        // The value an attacker would like to fix: a plain identifier.
        "__Host-jc_session=attacker-chosen-session-id".to_string(),
        // The session as JSON, unsealed: this is what the cookie decrypts to, and presenting it
        // directly is the shortcut the encryption exists to close.
        format!(
            "__Host-jc_session={}",
            r#"{"identity":{"subject":"f:1:admin","username":"admin","roles":["portal-admin"],"groups":[]},"expires_at":9999999999,"issued_at":0,"id_token":"","access_expires_at":9999999999}"#
        ),
        "__Host-jc_session=".to_string(),
        // A sealed value with one byte changed: the authentication tag is what refuses it. The
        // change is in the middle: the last base64 character may carry only padding bits, so
        // swapping it can leave the bytes as they were and the cookie still valid.
        format!("__Host-jc_session={}", tamper_middle(&sealed)),
        // Somebody else's cookie name, hoping the Portal reads the first thing it finds.
        format!("jc_sess={sealed}; __Host-jc_session=attacker-chosen-session-id"),
    ] {
        let (status, body) = me_with(app.clone(), &[("cookie", planted.clone())]).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{planted:.80} was taken for a session: {body}",
        );
    }

    // The control: the sealed cookie the Portal wrote is a session, so the five above are
    // refused for what they are and not because the door is shut.
    let (status, _) = me_with(app, &[("cookie", honest)]).await;
    assert_eq!(status, StatusCode::OK);
}

// -------------------------------------------------------------------------------------------
// T-1681, PF-46 — attack vector: open redirect and code interception at login.
//
// The login door is already played in `login_parks_the_return_path_in_the_cookie_and_never_in_the_url`
// (`//evil.example`, `https://evil.example`, the parameter twice, the flow cookie's flags, PKCE,
// `state` and `nonce` in the request, and nothing of the caller's input reaching the provider) and
// in `auth::oidc`'s unit tests for `safe_redirect` itself. The callback's refusals are played in
// `a_refused_callback_never_mints_a_session` and `callback_with_a_foreign_state_is_refused`.
//
// What was missing is the *successful* callback: that the target actually followed is the
// sanitized one, and that the code which just bought a session cannot buy a second one. Both need
// a login that completes, which is what `complete_login` below is.
// -------------------------------------------------------------------------------------------

/// The flow the Portal parked, read back out of its own encrypted cookie.
fn parked_flow(set_cookies: &[String]) -> (serde_json::Value, String) {
    use axum_extra::extract::cookie::{Key, PrivateCookieJar};

    let raw = set_cookies
        .iter()
        .find(|cookie| cookie.starts_with("__Host-jc_oidc_flow="))
        .and_then(|cookie| cookie.split(';').next())
        .expect("the flow cookie");
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(header::COOKIE, raw.parse().expect("a cookie header"));
    let jar = PrivateCookieJar::from_headers(&headers, Key::from("k".repeat(64).as_bytes()));
    let flow = jar
        .get("__Host-jc_oidc_flow")
        .expect("the Portal's own seal");
    (
        serde_json::from_str(flow.value()).expect("the parked flow"),
        raw.to_owned(),
    )
}

/// Starts a login, mounts a token endpoint that answers the code with an ID token carrying the
/// nonce the Portal parked, and returns the callback's response.
///
/// `code` is what the browser brings back; mounting the endpoint per call is what lets a second
/// call replay the same one.
async fn complete_login(
    realm: &MockServer,
    app: &axum::Router,
    redirect_to: &str,
) -> (String, String, axum::response::Response) {
    let started = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/auth/login?redirect_to={redirect_to}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::SEE_OTHER);
    let (flow, flow_cookie) = parked_flow(&set_cookie_values(&started));
    let csrf_state = flow["csrf_state"].as_str().expect("a state").to_owned();
    let nonce = flow["nonce"].as_str().expect("a nonce").to_owned();

    let id_token = keys::id_token(&issuer_of(realm), "demo.steward", &nonce);

    let code = "authorization-code-from-the-realm";
    // The realm as RFC 6749 §4.1.2 requires it: an authorization code is good once. The success
    // answer has the higher priority and is good for one call; every exchange after it falls
    // through to `invalid_grant`, which is what Keycloak answers a code it has already spent.
    Mock::given(method("POST"))
        .and(path(format!("{REALM_PATH}/protocol/openid-connect/token")))
        .and(wiremock::matchers::body_string_contains(
            "grant_type=authorization_code",
        ))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "Code not valid",
        })))
        .with_priority(2)
        .mount(realm)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM_PATH}/protocol/openid-connect/token")))
        .and(wiremock::matchers::body_string_contains(
            "grant_type=authorization_code",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "opaque-access-token",
            "token_type": "Bearer",
            "expires_in": 300,
            "refresh_token": "opaque-refresh-token",
            "id_token": id_token,
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(realm)
        .await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/auth/callback?code={code}&state={csrf_state}"
                ))
                .header(header::COOKIE, flow_cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    (
        code.to_owned(),
        format!("{flow_cookie}|{csrf_state}"),
        response,
    )
}

/// PF-47, T-2868: a Portal whose `portal-api` client is `federated-jwt` trades the code with its
/// projected ServiceAccount token, read when the code arrives, and sends neither its client id
/// nor a secret, which Keycloak would refuse beside the assertion.
#[tokio::test]
async fn a_federated_portal_exchanges_the_code_with_its_projected_token_and_no_client_id() {
    let realm = realm().await;
    let dir = std::env::temp_dir().join(format!("jc-auth-federated-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let token_file = dir.join("token");
    std::fs::write(&token_file, "eyJ.projected.token\n").expect("token file");
    let mut config = Config::from_vars(|k| match k {
        "JC_OIDC_ISSUER" => Some(issuer_of(&realm)),
        "JC_OIDC_CLIENT_ID" => Some("joinedcontext-portal".to_string()),
        "JC_OIDC_CLIENT_ASSERTION_FILE" => Some(token_file.display().to_string()),
        "JC_PORTAL_COOKIE_KEY" => Some("k".repeat(64)),
        _ => None,
    })
    .expect("config");
    config.public_base_url = "https://portal.test".parse().expect("url");
    let app = server::app(AppState::from_config(config).await.expect("discovery"));

    let (code, _, response) = complete_login(&realm, &app, "/").await;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "the login completes"
    );

    let exchange = realm
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .find(|request| request.url.path().ends_with("/token"))
        .expect("the code was exchanged");
    assert!(exchange.headers.get(header::AUTHORIZATION).is_none());
    let form: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(&exchange.body)
            .into_owned()
            .collect();
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["code"], code);
    assert_eq!(
        form["redirect_uri"],
        "https://portal.test/api/v1/auth/callback"
    );
    assert!(
        !form["code_verifier"].is_empty(),
        "the PKCE verifier goes along"
    );
    assert_eq!(form["client_assertion"], "eyJ.projected.token");
    assert_eq!(
        form["client_assertion_type"],
        "urn:ietf:params:oauth:client-assertion-type:jwt-bearer"
    );
    assert!(!form.contains_key("client_id"), "{form:?}");
    assert!(!form.contains_key("client_secret"), "{form:?}");
}

/// PF-46: the place the browser lands after a login is the sanitized path, never the caller's.
///
/// `safe_redirect` runs at the login door, so what the callback follows is whatever was parked.
/// This is the end of that chain: a tampered `redirect_to` really does end on the Portal's own
/// home page, and an honest one really does come back.
#[tokio::test]
async fn a_completed_login_lands_on_this_portal_and_never_where_the_caller_asked() {
    for (asked, landed) in [
        ("/projects/helsinki/spaces", "/projects/helsinki/spaces"),
        ("%2F%2Fevil.example%2Fsecret", "/"),
        ("https%3A%2F%2Fevil.example", "/"),
        ("%2F%5Cevil.example", "/"),
        ("", "/"),
    ] {
        let realm = realm().await;
        let app = app_with_realm(&realm).await;
        let (_, _, response) = complete_login(&realm, &app, asked).await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{asked}");
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(location, landed, "{asked} landed on {location}");
        assert!(
            set_cookie_values(&response)
                .iter()
                .any(|cookie| cookie.starts_with("__Host-jc_session=")),
            "{asked}: no session was minted",
        );
    }
}

/// PF-46: the code that bought a session cannot buy a second one.
///
/// A code reaches a place it should not from a shared machine's history, a referrer or a proxy
/// log. Two locks answer that, and the case exercises both:
///
/// * the Portal removes the flow cookie before it exchanges anything, so a replay in the
///   victim's own browser has no login in flight and is refused before the token endpoint is
///   called at all;
/// * a replay that also carries a copy of the flow cookie gets as far as the exchange, and the
///   realm refuses the code the second time (RFC 6749 §4.1.2, `invalid_grant`). The realm in this
///   test enforces that rule the way Keycloak does, which is what makes the dependency explicit
///   rather than assumed.
///
/// Either way nothing is minted, and the session the honest login won is left standing.
#[tokio::test]
async fn an_authorization_code_replayed_after_it_bought_a_session_buys_nothing() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;
    let (code, flow_and_state, first) = complete_login(&realm, &app, "/projects/helsinki").await;
    assert_eq!(first.status(), StatusCode::SEE_OTHER);

    let (flow_cookie, csrf_state) = flow_and_state.split_once('|').expect("both parked values");
    let session_cookie = set_cookie_values(&first)
        .into_iter()
        .find(|cookie| cookie.starts_with("__Host-jc_session="))
        .and_then(|cookie| cookie.split(';').next().map(str::to_owned))
        .expect("the session the first callback minted");

    // The flow cookie is gone from the browser after a successful callback, so the honest replay
    // carries none; the attacker's replay carries the copy they intercepted with the code, and
    // the third carries the session the first callback minted.
    for (what, cookie) in [
        ("with no login in flight", String::new()),
        ("with the intercepted flow cookie", flow_cookie.to_owned()),
        (
            "with the session the code already bought",
            session_cookie.clone(),
        ),
    ] {
        let mut request = Request::builder().uri(format!(
            "/api/v1/auth/callback?code={code}&state={csrf_state}"
        ));
        if !cookie.is_empty() {
            request = request.header(header::COOKIE, &cookie);
        }
        let replay = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(
            replay.status(),
            StatusCode::BAD_REQUEST,
            "the replay {what}"
        );
        let minted: Vec<String> = set_cookie_values(&replay)
            .into_iter()
            .filter(|cookie| {
                cookie.starts_with("__Host-jc_session=") && !cookie.contains("Max-Age=0")
            })
            .collect();
        assert!(
            minted.is_empty(),
            "the replay {what} minted a session: {minted:?}",
        );
    }

    // The session the honest login won is untouched by the three refusals.
    let (status, _) = me_with(app, &[("cookie", session_cookie)]).await;
    assert_eq!(status, StatusCode::OK);
}

/// T-3034, AP-122: the silent sign-in check asks the realm with `prompt=none` for the public
/// `portal-ui` client and lands on the Portal's own empty page, the answer in the fragment. It
/// sets no cookie and keeps no verifier: nothing the realm answers can ever be redeemed.
#[tokio::test]
async fn the_sso_check_asks_the_realm_silently_and_keeps_nothing() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/sso-check")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(
        set_cookie_values(&response).is_empty(),
        "a check starts no flow: {:?}",
        set_cookie_values(&response)
    );
    let location: url::Url = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .parse()
        .expect("a URL");
    assert_eq!(
        location.as_str().split('?').next(),
        Some(format!("{}/protocol/openid-connect/auth", issuer_of(&realm)).as_str())
    );
    let query: std::collections::BTreeMap<String, String> =
        location.query_pairs().into_owned().collect();
    assert_eq!(query["client_id"], "portal-ui");
    assert_eq!(query["prompt"], "none");
    assert_eq!(query["response_mode"], "fragment");
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["scope"], "openid");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["code_challenge"].len(), 43, "an S256 challenge");
    assert_eq!(
        query["redirect_uri"],
        "https://portal.test/api/v1/auth/sso-check/done"
    );
    assert!(!query.contains_key("client_secret"));

    // Every check is a fresh challenge: no verifier is kept to pair a code with.
    let again = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/sso-check")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let other = again.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(!other.contains(&query["code_challenge"]), "{other}");

    let done = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/sso-check/done")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(done.status(), StatusCode::OK);
    assert_eq!(done.headers()[header::CACHE_CONTROL], "no-store");
    assert!(done.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    let body = done.into_body().collect().await.unwrap().to_bytes();
    let page = String::from_utf8_lossy(&body);
    assert!(!page.contains("<script"), "the page runs nothing: {page}");
}

#[tokio::test]
async fn the_sso_check_without_a_configured_realm_is_unavailable() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/sso-check")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

/// One `Set-Cookie` line a browser keeps as `__Host-`: the prefix, `Secure`, `Path=/` and no
/// `Domain`; a line missing any of them is dropped, removals included (T-4395).
fn assert_host_only(line: &str) {
    let attributes: Vec<String> = line
        .split(';')
        .map(|a| a.trim().to_ascii_lowercase())
        .collect();
    assert!(line.starts_with("__Host-"), "{line}");
    assert!(
        attributes.iter().any(|a| a == "secure"),
        "no Secure: {line}"
    );
    assert!(
        attributes.iter().any(|a| a == "path=/"),
        "not on Path=/: {line}"
    );
    assert!(
        !attributes.iter().any(|a| a.starts_with("domain=")),
        "a Domain: {line}"
    );
}

/// T-4395, AP-29: an App may set a cookie for the shared parent domain, so every cookie the
/// Portal sets, live or removal, is one the browser keeps only as the Portal host's own.
#[tokio::test]
async fn every_cookie_the_portal_sets_is_host_prefixed() {
    let realm = realm().await;
    let login = app_with_realm(&realm)
        .await
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let logout = server::app(AppState::new(Config::for_tests(), None))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/logout")
                .header(header::COOKIE, "__Host-jc_csrf=t")
                .header("x-csrf-token", "t")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let refreshed = {
        use axum::response::IntoResponse;
        use axum_extra::extract::cookie::{Key, PrivateCookieJar};
        let jar = PrivateCookieJar::new(Key::from("k".repeat(64).as_bytes()));
        let jar = joinedcontext_portal::auth::session::clear(jar);
        (jar, StatusCode::OK).into_response()
    };

    let mut lines = set_cookie_values(&login);
    lines.extend(set_cookie_values(&logout));
    lines.extend(set_cookie_values(&refreshed));
    lines.extend(stored_session(300, -600));
    for name in [
        "__Host-jc_oidc_flow",
        "__Host-jc_session",
        "__Host-jc_refresh",
        "__Host-jc_csrf",
    ] {
        assert!(
            lines.iter().any(|l| l.starts_with(&format!("{name}="))),
            "{name} never set: {lines:?}"
        );
    }
    for line in &lines {
        assert_host_only(line);
    }
}

/// T-4395: a cookie an App planted under an old, unprefixed name rides beside the real ones. It
/// neither replaces the session nor passes CSRF; only the `__Host-` token does.
#[tokio::test]
async fn a_planted_unprefixed_cookie_neither_is_the_session_nor_passes_csrf() {
    let realm = realm().await;
    let app = app_behind(&realm, true).await;
    let cookies = format!(
        "jc_session=planted; {}; jc_session=planted-again; jc_refresh=planted",
        session_cookies(300)
    );
    let (status, me) = me_with(app, &[("cookie", cookies)]).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["username"], "demo.steward");

    let logout = |header_token: &'static str| async move {
        server::app(AppState::new(Config::for_tests(), None))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/logout")
                    .header(
                        header::COOKIE,
                        "jc_csrf=planted; __Host-jc_csrf=real; jc_csrf=planted",
                    )
                    .header("x-csrf-token", header_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    };
    assert_eq!(logout("planted").await, StatusCode::FORBIDDEN);
    assert_eq!(logout("real").await, StatusCode::OK);
}

/// T-5567: a token proves a second factor by `amr` naming `otp` or by the step-up `acr`; the
/// password alone proves none, whatever the person's rights.
#[tokio::test]
async fn a_token_proves_a_second_factor_only_by_otp_or_the_step_up_level() {
    let realm = realm().await;
    let app = app_behind(&realm, true).await;
    let issuer = issuer_of(&realm);
    for (extra, proves) in [
        (json!({}), false),
        (json!({ "acr": "1", "amr": ["pwd"] }), false),
        (json!({ "acr": "0" }), false),
        (json!({ "amr": "otp" }), true),
        (json!({ "acr": "1", "amr": ["pwd", "otp"] }), true),
        (json!({ "acr": "mfa" }), true),
    ] {
        let token = keys::token_with(&issuer, "demo.steward", extra.clone());
        for front in ["x-access-token", "authorization"] {
            let value = if front == "authorization" {
                format!("Bearer {token}")
            } else {
                token.clone()
            };
            let (status, me) = me_with(app.clone(), &[(front, value)]).await;
            assert_eq!(status, StatusCode::OK, "{extra} {front}: {me}");
            assert_eq!(me["second_factor"], json!(proves), "{extra} {front}: {me}");
        }
    }
}

/// T-5567: the edge logs in with fixed parameters, so the step-up is the Portal's own login; its
/// cookie proves the factor for the edge session of the same person, never of another one.
#[tokio::test]
async fn behind_the_edge_the_portal_s_step_up_session_proves_the_factor_of_the_same_person() {
    let realm = realm().await;
    let app = app_behind(&realm, true).await;
    let token = keys::token_with(
        &issuer_of(&realm),
        "demo.steward",
        json!({ "amr": ["pwd"] }),
    );
    for (cookie, proves) in [
        (stepped_up_cookie("demo.steward", true), true),
        (stepped_up_cookie("demo.steward", false), false),
        (stepped_up_cookie("demo.editor", true), false),
    ] {
        let (status, me) = me_with(
            app.clone(),
            &[("x-access-token", token.clone()), ("cookie", cookie)],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{me}");
        assert_eq!(me["username"], "demo.steward", "{me}");
        assert_eq!(me["second_factor"], json!(proves), "{me}");
    }
    // A bearer is a script: a cookie beside it lends it nothing.
    let (_, me) = me_with(
        app.clone(),
        &[
            ("authorization", format!("Bearer {token}")),
            ("cookie", stepped_up_cookie("demo.steward", true)),
        ],
    )
    .await;
    assert_eq!(me["second_factor"], json!(false), "{me}");
}

/// T-5567: the step-up login asks the realm for the level its OTP subflow sets, and for the code
/// even while the realm session lives; an ordinary login asks neither.
#[tokio::test]
async fn the_step_up_login_asks_the_realm_for_the_second_factor() {
    let realm = realm().await;
    let app = app_with_realm(&realm).await;
    for (query, step_up) in [
        ("?step_up=true&redirect_to=%2Fchanges", true),
        ("?redirect_to=%2Fchanges", false),
        ("?step_up=false", false),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/auth/login{query}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{query}");
        let location = response.headers()[header::LOCATION].to_str().unwrap();
        assert_eq!(
            location.contains("acr_values=mfa"),
            step_up,
            "{query}: {location}"
        );
        assert_eq!(
            location.contains("prompt=login"),
            step_up,
            "{query}: {location}"
        );
    }
}
