#![forbid(unsafe_code)]
//! Security regressions found in the 2026-11-07 review of the Argon build. Each test failed
//! against the code before its fix:
//! - jwt-bearer grant minted tokens for any `sub` a client chose (impersonation),
//! - federated-jwt and private_key_jwt assertions were accepted for any audience, disabled
//!   federated clients still authenticated,
//! - HTTP Basic client secrets were cut at a raw `=`, an empty stored secret matched an empty one,
//! - `client_credentials` took any person named `service-account-{client}` as the principal,
//! - the password grant had no brute-force protection,
//! - a wrong OTP could be retried without limit inside one login session,
//! - browser TOTP enrolment stored the constant secret `enrol-seed-secret-256` for everybody,
//! - a public client could run token exchange,
//! - userinfo answered for a token whose SSO session had ended.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::engine::{Engine, general_purpose::STANDARD};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tower::ServiceExt;

mod common;
use common::*;

const ISSUER: &str = "https://idm.dev.example.test/realms/dev";
const TOKEN: &str = "/realms/dev/protocol/openid-connect/token";
const STEWARD_ID: &str = "de300000-0000-4000-8000-000000000001";

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

/// A confidential client whose own ES256 key sits in `jwks.string`; returns the key and its KEK.
async fn jwks_client(
    store: &kaisatsu_store_memory::MemoryStore,
    client_id: &str,
    authenticator: &str,
) -> (SigningKeyRecord, Kek) {
    let kek = Kek::generate_for_tests();
    let key = KeyManager::generate_es256(client_id, &kek, 100, "ACTIVE").unwrap();
    let jwk: Value = serde_json::from_str(&key.public_jwk_json).unwrap();
    store
        .upsert_client(ClientRepresentation {
            id: format!("uuid-{client_id}"),
            realm_id: "dev".into(),
            client_id: client_id.into(),
            enabled: true,
            public_client: false,
            service_accounts_enabled: true,
            client_authenticator_type: authenticator.into(),
            secret: Some(format!("{client_id}-secret-value")),
            attributes: BTreeMap::from([(
                "jwks.string".into(),
                json!({ "keys": [jwk] }).to_string(),
            )]),
            ..Default::default()
        })
        .await
        .unwrap();
    (key, kek)
}

fn signed(key: &SigningKeyRecord, kek: &Kek, claims: &Value) -> String {
    sign_jwt(key, kek, Some("JWT"), claims).unwrap()
}

fn json_body(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}

#[tokio::test]
async fn jwt_bearer_assertion_cannot_name_a_person_or_be_replayed() {
    let (state, store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    let (key, kek) = jwks_client(&store, "batch-signer", "client-secret").await;
    let now = chrono::Utc::now().timestamp();

    // Impersonation: the client's own key signs `sub` = the steward.
    let forged = signed(
        &key,
        &kek,
        &json!({ "iss": "batch-signer", "sub": STEWARD_ID, "aud": ISSUER,
                 "jti": "j-1", "iat": now, "exp": now + 60 }),
    );
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("client_id", "batch-signer"),
            ("client_secret", "batch-signer-secret-value"),
            ("assertion", &forged),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(json_body(&body)["error"], "invalid_grant");

    // Create the service account, then the client may speak for itself.
    let (st, _, cc) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "client_credentials"),
            ("client_id", "batch-signer"),
            ("client_secret", "batch-signer-secret-value"),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{cc}");
    let sa_sub = decode_jwt_payload(json_body(&cc)["access_token"].as_str().unwrap())["sub"]
        .as_str()
        .unwrap()
        .to_owned();
    let own = signed(
        &key,
        &kek,
        &json!({ "iss": "batch-signer", "sub": "batch-signer", "aud": ISSUER,
                 "jti": "j-2", "iat": now, "exp": now + 60 }),
    );
    let pairs = [
        ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
        ("client_id", "batch-signer"),
        ("client_secret", "batch-signer-secret-value"),
        ("assertion", own.as_str()),
    ];
    let (st, _, ok) = post_form(&app, TOKEN, &pairs, None).await;
    assert_eq!(st, StatusCode::OK, "{ok}");
    assert_eq!(
        decode_jwt_payload(json_body(&ok)["access_token"].as_str().unwrap())["sub"],
        sa_sub
    );
    // Same `jti` again: replay.
    let (st, _, _) = post_form(&app, TOKEN, &pairs, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // An assertion whose `iss` is not the client is refused.
    let foreign_iss = signed(
        &key,
        &kek,
        &json!({ "iss": "someone-else", "sub": "batch-signer", "aud": ISSUER,
                 "jti": "j-3", "iat": now, "exp": now + 60 }),
    );
    let (st, _, _) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("client_id", "batch-signer"),
            ("client_secret", "batch-signer-secret-value"),
            ("assertion", &foreign_iss),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn federated_assertion_must_be_minted_for_this_realm_and_client_enabled() {
    let (state, store, _) = setup_dev_realm_state().await;
    let k8s_kek = Kek::generate_for_tests();
    let k8s_key = KeyManager::generate_es256("k8s", &k8s_kek, 100, "ACTIVE").unwrap();
    let jwk: Value = serde_json::from_str(&k8s_key.public_jwk_json).unwrap();
    state.guard().insert_mock(
        "https://kubernetes.default.svc.cluster.local/openid/v1/jwks",
        200,
        serde_json::to_vec(&json!({ "keys": [jwk] })).unwrap(),
    );
    let app = router(state, mount());
    let now = chrono::Utc::now().timestamp();
    let sa_token = |aud: &str| {
        signed(
            &k8s_key,
            &k8s_kek,
            &json!({ "iss": "https://kubernetes.default.svc.cluster.local",
                     "sub": "system:serviceaccount:portal:portal",
                     "aud": aud, "iat": now, "exp": now + 600 }),
        )
    };
    let call = |assertion: String| {
        let app = app.clone();
        async move {
            post_form(
                &app,
                TOKEN,
                &[
                    ("grant_type", "client_credentials"),
                    (
                        "client_assertion_type",
                        "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
                    ),
                    ("client_assertion", &assertion),
                ],
                None,
            )
            .await
        }
    };

    // The pod's token for the Kubernetes API server is not a token for this realm.
    let (st, _, _) = call(sa_token("https://kubernetes.default.svc.cluster.local")).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _, _) = call(sa_token("https://idm.dev.example.test/realms/other")).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _, body) = call(sa_token(ISSUER)).await;
    assert_eq!(st, StatusCode::OK, "{body}");

    let mut portal_api = store
        .get_client_by_client_id("dev", "portal-api")
        .await
        .unwrap()
        .unwrap();
    portal_api.enabled = false;
    store.upsert_client(portal_api).await.unwrap();
    let (st, _, _) = call(sa_token(ISSUER)).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn private_key_jwt_assertion_must_name_this_server_as_audience() {
    let (state, store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    let (key, kek) = jwks_client(&store, "pkjwt-app", "client-jwt").await;
    let now = chrono::Utc::now().timestamp();
    let assertion = |aud: &str, jti: &str| {
        signed(
            &key,
            &kek,
            &json!({ "iss": "pkjwt-app", "sub": "pkjwt-app", "aud": aud,
                     "jti": jti, "iat": now, "exp": now + 60 }),
        )
    };
    for (aud, jti, expected) in [
        (
            "https://other-idp.example/realms/dev",
            "a1",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "https://idm.dev.example.test/realms/other",
            "a2",
            StatusCode::UNAUTHORIZED,
        ),
        (ISSUER, "a3", StatusCode::OK),
        (
            "https://idm.dev.example.test/realms/dev/protocol/openid-connect/token",
            "a4",
            StatusCode::OK,
        ),
    ] {
        let a = assertion(aud, jti);
        let (st, _, body) = post_form(
            &app,
            TOKEN,
            &[
                ("grant_type", "client_credentials"),
                (
                    "client_assertion_type",
                    "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
                ),
                ("client_assertion", &a),
            ],
            None,
        )
        .await;
        assert_eq!(st, expected, "aud {aud}: {body}");
    }
}

async fn post_basic(app: &axum::Router, user: &str, pass: &str) -> (StatusCode, Value) {
    let creds = STANDARD.encode(format!("{user}:{pass}"));
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(TOKEN)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::AUTHORIZATION, format!("Basic {creds}"))
                .body(Body::from("grant_type=client_credentials"))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn basic_auth_keeps_the_whole_secret_and_empty_secrets_never_match() {
    let (state, store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    for (cid, secret) in [("eq-client", "abc=def&ghi"), ("empty-client", "")] {
        store
            .upsert_client(ClientRepresentation {
                id: format!("uuid-{cid}"),
                realm_id: "dev".into(),
                client_id: cid.into(),
                enabled: true,
                service_accounts_enabled: true,
                secret: Some(secret.into()),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    // A raw '=' and '&' are part of the secret, not separators.
    let (st, body) = post_basic(&app, "eq-client", "abc=def&ghi").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    // The truncated prefix the old decoder produced is not the secret.
    let (st, _) = post_basic(&app, "eq-client", "abc").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // Form-encoded halves decode as a whole.
    let (st, _) = post_basic(&app, "eq-client", "abc%3Ddef%26ghi").await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = post_basic(&app, "eq-client", "abc%3").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // An empty configured secret is no secret.
    let (st, _) = post_basic(&app, "empty-client", "").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _, _) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "client_credentials"),
            ("client_id", "empty-client"),
            ("client_secret", ""),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn client_credentials_never_uses_a_person_holding_the_service_account_name() {
    let (state, store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    // The person holds the name before the client gets a service account.
    store
        .create_user(UserRepresentation {
            id: "person-squatter".into(),
            realm_id: "dev".into(),
            username: "service-account-squat-app".into(),
            realm_roles: vec!["portal-approver".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .upsert_client(ClientRepresentation {
            id: "uuid-squat".into(),
            realm_id: "dev".into(),
            client_id: "squat-app".into(),
            enabled: true,
            service_accounts_enabled: true,
            secret: Some("squat-secret".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "client_credentials"),
            ("client_id", "squat-app"),
            ("client_secret", "squat-secret"),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(json_body(&body)["error"], "unauthorized_client");

    // Deleting the client leaves the person alone.
    store
        .delete_client("dev", None, "uuid-squat")
        .await
        .unwrap();
    assert!(
        store
            .get_user_by_id("dev", None, "person-squatter")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn password_grant_locks_after_repeated_failures_like_the_login_page() {
    let (state, _store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    let attempt = |pw: &'static str, user: &'static str| {
        let app = app.clone();
        async move {
            post_form(
                &app,
                TOKEN,
                &[
                    ("grant_type", "password"),
                    ("client_id", "apisix-gateway"),
                    ("client_secret", "apisix-secret-123"),
                    ("username", user),
                    ("password", pw),
                ],
                None,
            )
            .await
        }
    };
    for _ in 0..10 {
        let (st, _, _) = attempt("wrong-password-1A", "demo.orgmember@hel.fi").await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
    // The right password no longer works while the person is locked, with the same answer.
    let (st, _, body) = attempt("MemberPass123!Valid", "demo.orgmember@hel.fi").await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        json_body(&body)["error_description"],
        "Invalid user credentials"
    );
    // Nobody else is locked: the address was unknown, so no shared per-address counter ran.
    let (st, _, body) = attempt("AnalystPass123!Valid", "demo.analyst@hel.fi").await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

/// A password grant for `user` as `apisix-gateway` with `extra` fields.
async fn password_grant(
    app: &axum::Router,
    user: &str,
    pw: &str,
    extra: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut form = vec![
        ("grant_type", "password"),
        ("client_id", "apisix-gateway"),
        ("client_secret", concat!("apisix-", "secret-123")),
        ("username", user),
        ("password", pw),
    ];
    form.extend_from_slice(extra);
    let (st, _, body) = post_form(app, TOKEN, &form, None).await;
    (st, json_body(&body))
}

async fn refresh_grant(app: &axum::Router, refresh: &str) -> (StatusCode, Value) {
    let (st, _, body) = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", "apisix-gateway"),
            ("client_secret", concat!("apisix-", "secret-123")),
            ("refresh_token", refresh),
        ],
        None,
    )
    .await;
    (st, json_body(&body))
}

#[tokio::test]
async fn the_password_grant_obeys_the_floor() {
    let (state, _store, _) = setup_dev_realm_state().await;
    let plain = router(state.clone(), mount());
    let floor = router(
        state.with_second_factor(kaisatsu_server::SecondFactor::Required),
        mount(),
    );
    let (st, _) = password_grant(&plain, "demo.analyst@hel.fi", "AnalystPass123!Valid", &[]).await;
    assert_eq!(st, StatusCode::OK);
    // Under the floor the person without an authenticator gets no token from a password alone.
    let (st, body) =
        password_grant(&floor, "demo.analyst@hel.fi", "AnalystPass123!Valid", &[]).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "invalid_grant");
    assert!(
        body["error_description"]
            .as_str()
            .unwrap()
            .contains("second factor"),
        "{body}"
    );
    // The steward passes it with the authenticator's code.
    let now = chrono::Utc::now().timestamp() as u64;
    let code = generate_totp_code("steward-totp-seed-123456", 6, 30, "HmacSHA256", now).unwrap();
    let (st, body) = password_grant(
        &floor,
        "demo.steward@hel.fi",
        "StewardPass123!Valid",
        &[("totp", &code)],
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let claims = decode_jwt_payload(body["access_token"].as_str().unwrap());
    assert_eq!(claims["amr"], json!(["pwd", "otp"]));
}

#[tokio::test]
async fn client_credentials_never_asks_for_a_second_factor() {
    let (state, _store, _) = setup_dev_realm_state().await;
    let floor = router(
        state.with_second_factor(kaisatsu_server::SecondFactor::Required),
        mount(),
    );
    let (st, _, body) = post_form(
        &floor,
        TOKEN,
        &[
            ("grant_type", "client_credentials"),
            ("client_id", "apisix-gateway"),
            ("client_secret", concat!("apisix-", "secret-123")),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(json_body(&body)["access_token"].is_string());
}

#[tokio::test]
async fn turning_the_floor_on_revokes_refresh_and_offline_sessions() {
    let (state, store, _) = setup_dev_realm_state().await;
    let mut gateway = store
        .get_client_by_client_id("dev", "apisix-gateway")
        .await
        .unwrap()
        .unwrap();
    gateway.optional_client_scopes.push("offline_access".into());
    store.upsert_client(gateway).await.unwrap();
    let before = router(state.clone(), mount());
    let after = router(
        state.with_second_factor(kaisatsu_server::SecondFactor::Required),
        mount(),
    );
    let (st, online) =
        password_grant(&before, "demo.analyst@hel.fi", "AnalystPass123!Valid", &[]).await;
    assert_eq!(st, StatusCode::OK, "{online}");
    let (st, offline) = password_grant(
        &before,
        "demo.analyst@hel.fi",
        "AnalystPass123!Valid",
        &[("scope", "openid offline_access")],
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{offline}");
    assert!(
        offline["scope"]
            .as_str()
            .unwrap()
            .split(' ')
            .any(|s| s == "offline_access"),
        "{offline}"
    );
    let online = online["refresh_token"].as_str().unwrap().to_owned();
    let offline = offline["refresh_token"].as_str().unwrap().to_owned();
    // The floor turns on: the next refresh of a password-only session is refused ...
    let (st, body) = refresh_grant(&after, &online).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "invalid_grant");
    assert!(
        body["error_description"]
            .as_str()
            .unwrap()
            .contains("second factor"),
        "{body}"
    );
    // ... and every session of the person ended with it, the offline one included: even a
    // server without the floor refuses the rest.
    let (st, body) = refresh_grant(&before, &offline).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error_description"], "Session not active");
}

/// Starts a browser login for `edge` and returns `(login_url, csrf, session_code)`.
async fn start_login(app: &axum::Router, extra: &str) -> (String, String, String) {
    let verifier = "pkce-verifier-string-with-at-least-43-characters-long-012345";
    let uri = format!(
        "/realms/dev/protocol/openid-connect/auth?client_id=edge&redirect_uri={}&response_type=code&scope=openid&state=s&code_challenge={}&code_challenge_method=S256&ui_locales=en{extra}",
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
    let login = resp.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let cookie = resp.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&login)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html = String::from_utf8(
        page.into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let csrf = extract_hidden_input(&html, "csrf_token");
    let code = extract_hidden_input(&html, "session_code");
    (format!("{login}##{cookie}"), csrf, code)
}

#[tokio::test]
async fn wrong_otp_cannot_be_retried_without_limit_in_one_login() {
    let (state, _store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    let (login_and_ck, csrf, code) = start_login(&app, "").await;
    let (login, ck) = login_and_ck.split_once("##").unwrap();
    let (st, _, html) = post_form(
        &app,
        login,
        &[
            ("csrf_token", &csrf),
            ("session_code", &code),
            ("username", "demo.steward@hel.fi"),
            ("password", "StewardPass123!Valid"),
        ],
        Some(ck),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(html.contains("login-otp"));
    for _ in 0..10 {
        let (st, _, _) = post_form(
            &app,
            login,
            &[
                ("csrf_token", &csrf),
                ("session_code", &code),
                ("otp", "000000"),
            ],
            Some(ck),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
    }
    let now = chrono::Utc::now().timestamp() as u64;
    let right = generate_totp_code("steward-totp-seed-123456", 6, 30, "HmacSHA256", now).unwrap();
    let (st, _, _) = post_form(
        &app,
        login,
        &[
            ("csrf_token", &csrf),
            ("session_code", &code),
            ("otp", &right),
        ],
        Some(ck),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::OK,
        "a locked person must not finish the login with the 11th code"
    );
}

#[tokio::test]
async fn browser_totp_enrolment_uses_a_fresh_secret_shown_on_the_page() {
    let (state, store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    let (login_and_ck, csrf, code) = start_login(&app, "&acr_values=mfa").await;
    let (login, ck) = login_and_ck.split_once("##").unwrap();
    let (st, _, html) = post_form(
        &app,
        login,
        &[
            ("csrf_token", &csrf),
            ("session_code", &code),
            ("username", "demo.analyst@hel.fi"),
            ("password", "AnalystPass123!Valid"),
        ],
        Some(ck),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(html.contains("login-config-totp"), "{html}");
    let marker = "aria-labelledby=\"kc-key-label\">";
    let at = html.find(marker).expect("enrolment page shows the key") + marker.len();
    let secret: String = html[at..]
        .chars()
        .take_while(|c| c.is_ascii_uppercase() || ('2'..='7').contains(c) || *c == ' ')
        .filter(|c| *c != ' ')
        .collect();
    assert_eq!(secret.len(), 32);

    let realm = store.get_realm("dev").await.unwrap().unwrap();
    let now = chrono::Utc::now().timestamp() as u64;
    // The old built-in constant must not enrol anyone.
    let constant = generate_totp_code(
        "enrol-seed-secret-256",
        6,
        30,
        &realm.otp_policy_algorithm,
        now,
    )
    .unwrap();
    let cred = CredentialRecord {
        credential_type: "otp".into(),
        secret_data: json!({ "value": secret }).to_string(),
        credential_data: json!({
            "digits": realm.otp_policy_digits,
            "period": realm.otp_policy_period,
            "algorithm": realm.otp_policy_algorithm,
            "secretEncoding": "BASE32",
        })
        .to_string(),
        ..Default::default()
    };
    let shown = generate_totp_code_for_credential(&cred, now).unwrap();
    if constant != shown {
        let (st, _, _) = post_form(
            &app,
            login,
            &[
                ("csrf_token", &csrf),
                ("session_code", &code),
                ("otp", &constant),
            ],
            Some(ck),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "the constant secret must not enrol");
    }
    let (st, hdrs, _) = post_form(
        &app,
        login,
        &[
            ("csrf_token", &csrf),
            ("session_code", &code),
            ("otp", &shown),
        ],
        Some(ck),
    )
    .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(hdrs[header::LOCATION].to_str().unwrap().contains("code="));
    let creds = store
        .list_credentials("dev", "de300000-0000-4000-8000-000000000010")
        .await
        .unwrap();
    let otp = creds
        .iter()
        .find(|c| c.credential_type == "otp")
        .expect("TOTP enrolled");
    assert!(otp.secret_data.contains(&secret));
    assert!(!otp.secret_data.contains("enrol-seed-secret-256"));
}

fn realm_token(
    store: &kaisatsu_store_memory::MemoryStore,
    keys: &[SigningKeyRecord],
    claims: &Value,
) -> String {
    let key = keys.iter().find(|k| k.status == "ACTIVE").unwrap();
    sign_jwt(key, store.kek(), Some("JWT"), claims).unwrap()
}

#[tokio::test]
async fn public_client_cannot_run_token_exchange() {
    let (state, store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    let mut ui = store
        .get_client_by_client_id("dev", "portal-ui")
        .await
        .unwrap()
        .unwrap();
    ui.attributes
        .insert("standard.token.exchange.enabled".into(), "true".into());
    store.upsert_client(ui).await.unwrap();
    let keys = store.list_signing_keys("dev").await.unwrap();
    let now = chrono::Utc::now().timestamp();
    let subject = realm_token(
        &store,
        &keys,
        &json!({ "iss": ISSUER, "sub": STEWARD_ID, "aud": ["portal-ui"], "azp": "edge",
                 "typ": "Bearer", "jti": "subj-1", "iat": now, "exp": now + 300 }),
    );
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            ("client_id", "portal-ui"),
            ("subject_token", &subject),
            (
                "subject_token_type",
                "urn:ietf:params:oauth:token-type:access_token",
            ),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(json_body(&body).get("access_token").is_none());
}

#[tokio::test]
async fn userinfo_refuses_tokens_of_an_ended_session_or_revoked() {
    let (state, store, _) = setup_dev_realm_state().await;
    let app = router(state, mount());
    let keys = store.list_signing_keys("dev").await.unwrap();
    let now = chrono::Utc::now().timestamp();
    let ask = |token: String| {
        let app = app.clone();
        async move {
            app.oneshot(
                Request::builder()
                    .uri("/realms/dev/protocol/openid-connect/userinfo")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
        }
    };
    let base = json!({ "iss": ISSUER, "sub": STEWARD_ID, "azp": "edge", "typ": "Bearer",
                       "scope": "openid", "iat": now, "exp": now + 300 });
    let mut live = base.clone();
    live["jti"] = json!("ui-live");
    assert_eq!(ask(realm_token(&store, &keys, &live)).await, StatusCode::OK);

    let mut ended = base.clone();
    ended["jti"] = json!("ui-ended");
    ended["sid"] = json!("sso-that-was-logged-out");
    assert_eq!(
        ask(realm_token(&store, &keys, &ended)).await,
        StatusCode::UNAUTHORIZED
    );

    let mut revoked = base;
    revoked["jti"] = json!("ui-revoked");
    store
        .mark_token_revoked(RevokedTokenRecord {
            jti_hash: sha256_hex("ui-revoked"),
            realm_id: "dev".into(),
            expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
        })
        .await
        .unwrap();
    assert_eq!(
        ask(realm_token(&store, &keys, &revoked)).await,
        StatusCode::UNAUTHORIZED
    );
}

/// K2: Proved red against the code before K2, where `organization:<alias>` in `scope` was
/// accepted without checking the person's memberships and injected into `jc_org` via
/// `active_org`.
#[tokio::test]
async fn k2_organization_scope_checks_membership_and_never_emits_foreign_org_claim() {
    let (state, store, _) = setup_dev_realm_state().await;
    for org in ["helsinki", "other"] {
        store
            .upsert_organization(Organization {
                id: org.into(),
                realm_id: "dev".into(),
                name: org.into(),
                alias: org.into(),
                enabled: true,
                description: String::new(),
                domains: Vec::new(),
                attributes: Default::default(),
                managed_by: None,
            })
            .await
            .unwrap();
    }
    let app = router(state, mount());

    // 1. Non-member (`demo.orgmember@hel.fi` belongs to `helsinki`, not `other`) requests
    // `scope=openid organization:other`: token must carry no `organization` or `jc_org` for `other`.
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "password"),
            ("client_id", "apisix-gateway"),
            ("client_secret", concat!("apisix-", "secret-123")),
            ("username", "demo.orgmember@hel.fi"),
            ("password", "MemberPass123!Valid"),
            ("scope", "openid organization:other"),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let non_member_claims = decode_jwt_payload(json_body(&body)["access_token"].as_str().unwrap());
    assert!(
        non_member_claims.get("organization").is_none(),
        "non-member must not receive organization claim: {non_member_claims}"
    );
    assert!(
        non_member_claims
            .get("jc_org")
            .and_then(Value::as_array)
            .is_none_or(|a| !a.iter().any(|v| v.as_str() == Some("other"))),
        "non-member must not receive jc_org=other: {non_member_claims}"
    );

    // 2. Member (`demo.orgmember@hel.fi` asks `organization:helsinki`) gets `organization` and `jc_org`.
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "password"),
            ("client_id", "apisix-gateway"),
            ("client_secret", concat!("apisix-", "secret-123")),
            ("username", "demo.orgmember@hel.fi"),
            ("password", "MemberPass123!Valid"),
            ("scope", "openid organization:helsinki"),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let member_claims = decode_jwt_payload(json_body(&body)["access_token"].as_str().unwrap());
    assert_eq!(member_claims["organization"], json!({ "helsinki": {} }));
    assert_eq!(member_claims["jc_org"], json!(["helsinki"]));
}
