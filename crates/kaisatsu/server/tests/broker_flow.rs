#![forbid(unsafe_code)]
//! Upstream OIDC broker route tests (`/realms/{realm}/broker/{alias}/login` and
//! `/realms/{realm}/broker/{alias}/endpoint`, W7, W11, K12) against a live local Axum upstream
//! server allowed by the test `OutboundGuard` policy:
//! - happy path (new user sign-up and subsequent linked login),
//! - PKCE S256 verifier checked at the upstream token endpoint,
//! - wrong `state` and wrong `nonce` refused,
//! - unverified upstream e-mail refused,
//! - existing local account requires POST + CSRF + local password confirmation before linking.

use axum::body::Body;
use axum::extract::{Form, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_oidc::{OutboundGuard, OutboundPolicy};
use kaisatsu_server::{KaisatsuState, router};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::net::TcpListener;
use tower::ServiceExt;

mod common;
use common::*;

#[derive(Clone)]
struct FakeUpstreamState {
    key: SigningKeyRecord,
    kek: Kek,
    next_claims: Arc<Mutex<Value>>,
    last_token_form: Arc<Mutex<BTreeMap<String, String>>>,
}

async fn start_fake_upstream() -> (String, FakeUpstreamState) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let issuer = format!("http://127.0.0.1:{port}");
    let kek = Kek::generate_for_tests();
    let key = KeyManager::generate_es256("upstream", &kek, 100, "ACTIVE").unwrap();
    let st = FakeUpstreamState {
        key,
        kek,
        next_claims: Arc::new(Mutex::new(json!({}))),
        last_token_form: Arc::new(Mutex::new(BTreeMap::new())),
    };

    let app =
        Router::new()
            .route(
                "/jwks",
                get(|State(s): State<FakeUpstreamState>| async move {
                    let jwk: Value = serde_json::from_str(&s.key.public_jwk_json).unwrap();
                    Json(json!({ "keys": [jwk] }))
                }),
            )
            .route(
                "/token",
                post(
                    |State(s): State<FakeUpstreamState>,
                     Form(form): Form<BTreeMap<String, String>>| async move {
                        *s.last_token_form.lock().unwrap() = form;
                        let claims = s.next_claims.lock().unwrap().clone();
                        let id_token = sign_jwt(&s.key, &s.kek, Some("JWT"), &claims).unwrap();
                        Json(json!({
                            "access_token": "upstream-at",
                            "token_type": "Bearer",
                            "id_token": id_token
                        }))
                    },
                ),
            )
            .with_state(st.clone());

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (issuer, st)
}

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

async fn start_auth_session(app: &Router) -> (String, String) {
    let verifier = "pkce-verifier-string-with-at-least-43-characters-long-012345";
    let uri = format!(
        "/realms/dev/protocol/openid-connect/auth?client_id=edge&redirect_uri={}&response_type=code&scope=openid&state=client-state-1&code_challenge={}&code_challenge_method=S256",
        url::form_urlencoded::byte_serialize(b"https://portal.dev.example.test/callback")
            .collect::<String>(),
        sha256_base64url(verifier),
    );
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp.headers()[header::LOCATION].to_str().unwrap();
    let cookie = resp.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let parsed = url::Url::parse(&format!("https://idm.dev.example.test{loc}")).unwrap();
    let session_code = parsed
        .query_pairs()
        .find(|(k, _)| k == "session_code")
        .unwrap()
        .1
        .into_owned();
    (session_code, cookie)
}

async fn get_with_ck(app: &Router, uri: &str, cookie: &str) -> (StatusCode, HeaderMap, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    let h = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, h, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn k12_upstream_oidc_broker_happy_path_pkce_state_nonce_and_link_confirmation() {
    let (up_issuer, up_state) = start_fake_upstream().await;
    let (base_state, store, kek) = setup_dev_realm_state().await;
    let _ = base_state;

    let guard = OutboundGuard::new(OutboundPolicy {
        in_cluster_hosts: vec!["127.0.0.1".into()],
        ..OutboundPolicy::default()
    });
    store
        .upsert_idp(IdentityProviderRepresentation {
            internal_id: "idp-entra".into(),
            realm_id: "dev".into(),
            org_id: Some("helsinki".into()),
            alias: "entra".into(),
            provider_id: "oidc".into(),
            enabled: true,
            trust_email: true,
            first_broker_login_flow_alias: "first broker login".into(),
            config: BTreeMap::from([
                ("issuer".into(), up_issuer.clone()),
                ("authorizationUrl".into(), format!("{up_issuer}/authorize")),
                ("tokenUrl".into(), format!("{up_issuer}/token")),
                ("jwksUrl".into(), format!("{up_issuer}/jwks")),
                ("clientId".into(), "kaisatsu-broker-client".into()),
                ("clientSecret".into(), "broker-secret-xyz".into()),
            ]),
            managed_by: None,
        })
        .await
        .unwrap();

    let state = KaisatsuState::new(
        store.clone() as Arc<dyn Store>,
        kek,
        guard,
        Arc::new(LocalAuthorizer),
    );
    let app = router(state, mount()).layer(Extension(mount()));

    // 1. Start broker login -> redirects to upstream with state, nonce, and PKCE S256 challenge.
    let (session_code, auth_cookie) = start_auth_session(&app).await;
    let (st, hdrs, _) = get_with_ck(
        &app,
        &format!("/realms/dev/broker/entra/login?session_code={session_code}"),
        &auth_cookie,
    )
    .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let up_auth_url = url::Url::parse(hdrs[header::LOCATION].to_str().unwrap()).unwrap();
    let qp: BTreeMap<String, String> = up_auth_url.query_pairs().into_owned().collect();
    assert_eq!(qp["code_challenge_method"], "S256");
    let broker_state = qp["state"].clone();
    let broker_nonce = qp["nonce"].clone();
    let code_challenge = qp["code_challenge"].clone();

    // 2. Wrong state is refused.
    let (st_bad_state, _, _) = get_with_ck(
        &app,
        &format!("/realms/dev/broker/entra/endpoint?code=c1&state={session_code}.wrong"),
        &auth_cookie,
    )
    .await;
    assert_eq!(st_bad_state, StatusCode::BAD_REQUEST);

    // 3. Wrong nonce in upstream ID token is refused.
    let now = chrono::Utc::now().timestamp();
    *up_state.next_claims.lock().unwrap() = json!({
        "iss": up_issuer,
        "sub": "entra-sub-1",
        "aud": "kaisatsu-broker-client",
        "nonce": "wrong-nonce",
        "email": "new.broker@hel.fi",
        "email_verified": true,
        "iat": now,
        "exp": now + 300
    });
    let (st_bad_nonce, _, _) = get_with_ck(
        &app,
        &format!("/realms/dev/broker/entra/endpoint?code=c1&state={broker_state}"),
        &auth_cookie,
    )
    .await;
    assert_eq!(st_bad_nonce, StatusCode::UNAUTHORIZED);

    // Verify that the PKCE verifier sent to the upstream token endpoint hashes to `code_challenge`.
    let sent_verifier = up_state.last_token_form.lock().unwrap()["code_verifier"].clone();
    assert_eq!(sha256_base64url(&sent_verifier), code_challenge);

    // 4. Unverified e-mail is refused (`email_verified: false`).
    *up_state.next_claims.lock().unwrap() = json!({
        "iss": up_issuer,
        "sub": "entra-sub-unverified",
        "aud": "kaisatsu-broker-client",
        "nonce": broker_nonce,
        "email": "unverified@hel.fi",
        "email_verified": false,
        "iat": now,
        "exp": now + 300
    });
    let (st_unver, _, _) = get_with_ck(
        &app,
        &format!("/realms/dev/broker/entra/endpoint?code=c2&state={broker_state}"),
        &auth_cookie,
    )
    .await;
    assert_eq!(st_unver, StatusCode::FORBIDDEN);

    // 5. Happy path for a new user (`new.broker@hel.fi`): creates user + link and redirects with code.
    *up_state.next_claims.lock().unwrap() = json!({
        "iss": up_issuer,
        "sub": "entra-sub-new",
        "aud": "kaisatsu-broker-client",
        "nonce": broker_nonce,
        "email": "new.broker@hel.fi",
        "email_verified": true,
        "given_name": "Nora",
        "family_name": "Broker",
        "iat": now,
        "exp": now + 300
    });
    let (st_ok, h_ok, _) = get_with_ck(
        &app,
        &format!("/realms/dev/broker/entra/endpoint?code=c3&state={broker_state}"),
        &auth_cookie,
    )
    .await;
    assert_eq!(st_ok, StatusCode::SEE_OTHER);
    assert!(h_ok[header::LOCATION].to_str().unwrap().contains("code="));

    // 6. Existing local account (`demo.orgmember@hel.fi`) requires link confirmation + local password.
    let (s2, ck2) = start_auth_session(&app).await;
    let (_, h2, _) = get_with_ck(
        &app,
        &format!("/realms/dev/broker/entra/login?session_code={s2}"),
        &ck2,
    )
    .await;
    let u2 = url::Url::parse(h2[header::LOCATION].to_str().unwrap()).unwrap();
    let qp2: BTreeMap<String, String> = u2.query_pairs().into_owned().collect();
    *up_state.next_claims.lock().unwrap() = json!({
        "iss": up_issuer,
        "sub": "entra-sub-existing",
        "aud": "kaisatsu-broker-client",
        "nonce": qp2["nonce"],
        "email": "demo.orgmember@hel.fi",
        "email_verified": true,
        "iat": now,
        "exp": now + 300
    });
    let (st_conf, _, html_conf) = get_with_ck(
        &app,
        &format!(
            "/realms/dev/broker/entra/endpoint?code=c4&state={}",
            qp2["state"]
        ),
        &ck2,
    )
    .await;
    assert_eq!(st_conf, StatusCode::OK);
    assert!(
        html_conf.contains("login-broker-link-confirm")
            && html_conf.contains("demo.orgmember@hel.fi"),
        "{html_conf}"
    );
    let conf_csrf = extract_hidden_input(&html_conf, "csrf_token");

    // Wrong local password does not link
    let (st_wrong_pw, _, html_wrong_pw) = post_form(
        &app,
        "/realms/dev/broker/entra/endpoint",
        &[
            ("csrf_token", &conf_csrf),
            ("session_code", &s2),
            ("password", "WrongLocalPass!"),
        ],
        Some(&ck2),
    )
    .await;
    assert_eq!(st_wrong_pw, StatusCode::OK);
    assert!(html_wrong_pw.contains("role=\"alert\""));

    // Right local password confirms the link and completes login
    let (st_linked, h_linked, _) = post_form(
        &app,
        "/realms/dev/broker/entra/endpoint",
        &[
            ("csrf_token", &conf_csrf),
            ("session_code", &s2),
            ("password", "MemberPass123!Valid"),
        ],
        Some(&ck2),
    )
    .await;
    assert_eq!(st_linked, StatusCode::SEE_OTHER);
    assert!(
        h_linked[header::LOCATION]
            .to_str()
            .unwrap()
            .contains("code=")
    );
}

#[tokio::test]
async fn a_brokered_sign_in_under_the_floor_goes_on_to_the_second_factor() {
    let (up_issuer, up_state) = start_fake_upstream().await;
    let (_, store, kek) = setup_dev_realm_state().await;
    store
        .upsert_idp(IdentityProviderRepresentation {
            internal_id: "idp-entra".into(),
            realm_id: "dev".into(),
            org_id: Some("helsinki".into()),
            alias: "entra".into(),
            provider_id: "oidc".into(),
            enabled: true,
            trust_email: true,
            first_broker_login_flow_alias: "first broker login".into(),
            config: BTreeMap::from([
                ("issuer".into(), up_issuer.clone()),
                ("authorizationUrl".into(), format!("{up_issuer}/authorize")),
                ("tokenUrl".into(), format!("{up_issuer}/token")),
                ("jwksUrl".into(), format!("{up_issuer}/jwks")),
                ("clientId".into(), "kaisatsu-broker-client".into()),
                ("clientSecret".into(), "broker-secret-xyz".into()),
            ]),
            managed_by: None,
        })
        .await
        .unwrap();
    let guard = OutboundGuard::new(OutboundPolicy {
        in_cluster_hosts: vec!["127.0.0.1".into()],
        ..OutboundPolicy::default()
    });
    let state = KaisatsuState::new(
        store.clone() as Arc<dyn Store>,
        kek,
        guard,
        Arc::new(LocalAuthorizer),
    )
    .with_second_factor(kaisatsu_server::SecondFactor::Required);
    let app = router(state, mount()).layer(Extension(mount()));

    let (session_code, auth_cookie) = start_auth_session(&app).await;
    let (_, hdrs, _) = get_with_ck(
        &app,
        &format!("/realms/dev/broker/entra/login?session_code={session_code}"),
        &auth_cookie,
    )
    .await;
    let up = url::Url::parse(hdrs[header::LOCATION].to_str().unwrap()).unwrap();
    let qp: BTreeMap<String, String> = up.query_pairs().into_owned().collect();
    let now = chrono::Utc::now().timestamp();
    *up_state.next_claims.lock().unwrap() = json!({
        "iss": up_issuer,
        "sub": "entra-sub-floor",
        "aud": "kaisatsu-broker-client",
        "nonce": qp["nonce"],
        "email": "floor.broker@hel.fi",
        "email_verified": true,
        "iat": now,
        "exp": now + 300
    });
    let (st, h, _) = get_with_ck(
        &app,
        &format!(
            "/realms/dev/broker/entra/endpoint?code=c1&state={}",
            qp["state"]
        ),
        &auth_cookie,
    )
    .await;
    // No code: the upstream sign-in counts as the first factor only.
    assert_eq!(st, StatusCode::SEE_OTHER);
    let next = h[header::LOCATION].to_str().unwrap().to_owned();
    assert!(
        !next.contains("?code=") && !next.contains("&code="),
        "{next}"
    );
    assert!(next.contains("/login-actions/authenticate"), "{next}");
    // The person has no authenticator yet: the login page enrols one.
    let (st, _, html) = get_with_ck(&app, &next, &auth_cookie).await;
    assert_eq!(st, StatusCode::OK);
    assert!(html.contains("kc-totp-settings-form"), "{html}");
}
