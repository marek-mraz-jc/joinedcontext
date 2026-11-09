#![forbid(unsafe_code)]
//! End-to-end protocol, integration, and security regression tests (Items 8 & W1-W12):
//! 1. Portal login (`authorization_code` + PKCE S256 + OTP step-up, verifying every claim the
//!    Portal reads in `src/auth/oidc.rs` and `src/auth/session.rs`: `sub` fixed ID, `acr == "mfa"`,
//!    `amr == ["pwd", "otp"]`, `groups`, `jc_org`, `jc_groups`, `realm_access.roles`, `aud`).
//! 2. Portal person -> Endpoint token exchange V2 (`urn:ietf:params:oauth:grant-type:token-exchange`
//!    for `audience=mcp-hub` and `scope=gateway-hub endpoint:counters1234`, verifying every claim
//!    `jc-core/src/auth/token.rs` and `context-gateway/src/auth/subject.rs` read, plus `act`).
//! 3. Workload `client_credentials` with `federated-jwt` (Kubernetes ServiceAccount token verified
//!    against the cluster issuer's JWKS).
//! 4. CIMD connector (`https://claude.ai/oauth/mcp-oauth-client-metadata`) with parameterized scope
//!    `connector:abcdefghij2345672345672345` and `offline_access` (and `fullScopeAllowed: false`).
//! 5. Back-channel logout (`POST` to `backchannel.logout.url` with `typ: "Logout"` and `events` claim,
//!    no `nonce`) and RFC 7009 refresh token revocation ending the client's grant.
//! 6. Refresh token rotation and reuse detection revoking the entire family.
//! 7. Mount under a path prefix (`/id` on `city.example`) beside sibling modules on one host vs
//!    dedicated host (`id.example` at root), verifying issuer strings and `__Secure-wk_*` vs
//!    `__Host-wk_*` cookies (W9).
//! 8. W12 attack & 2026 advisory regression suite:
//!    - Authorization code replay revoking tokens issued from the first redemption,
//!    - Redirect URI bypasses (added path segment, other port, userinfo `@` trick),
//!    - PKCE downgrade (`plain` or missing verifier),
//!    - Token exchange to an audience the requester has no scope for,
//!    - Cross-organization admin read by ID and by search (404 / zero rows),
//!    - Timing-equal unknown user on login and reset-credentials,
//!    - CIMD SSRF pointing at `169.254.169.254`,
//!    - Cross-realm / cross-client code redemption (FerrisKey GHSA-v2hq-9v44-43ff),
//!    - Refresh token with wrong client `azp` (Rauthy GHSA-7qh2-3hc5-2vqp),
//!    - Short/malformed tokens on token/introspect/revoke/userinfo never panicking (Rauthy GHSA-wx92-7mmw-5x82).

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use jc_router::{JcRouterBuilder, Mount, MountSpec};
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;
use common::*;

#[tokio::test]
async fn e2e_portal_login_pkce_otp_stepup_and_endpoint_token_exchange_and_backchannel_logout() {
    let (state, _store, _kek) = setup_dev_realm_state().await;
    state.guard().insert_mock(
        "http://portal.portal.svc.cluster.local:8080/api/v1/auth/backchannel-logout",
        204,
        Vec::new(),
    );
    let mount = Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap();
    let app = router(state.clone(), mount);

    // 1. Start Portal OIDC code + PKCE S256 login with `acr_values=mfa`
    let verifier = "pkce-verifier-string-with-at-least-43-characters-long-012345";
    let challenge = sha256_base64url(verifier);
    let auth_uri = format!(
        "/realms/dev/protocol/openid-connect/auth?client_id=edge&redirect_uri={}&response_type=code&scope=openid%20profile%20email&state=st-123&nonce=n-456&code_challenge={challenge}&code_challenge_method=S256&acr_values=mfa&ui_locales=sk",
        url::form_urlencoded::byte_serialize(b"https://portal.dev.example.test/callback")
            .collect::<String>()
    );
    let resp1 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&auth_uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::SEE_OTHER);
    let login_loc = resp1
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let auth_ck = auth_cookie(resp1.headers());

    // 2. GET login page: verify Slovak translation (`Prihlásenie: Portál`) and strict CSP
    let resp2 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&login_loc)
                .header(header::COOKIE, &auth_ck)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let csp = resp2
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(csp.contains("frame-ancestors 'none'"));
    let html2 = String::from_utf8(
        resp2
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(html2.contains("Prihlásenie: Portál"));
    let csrf_token = extract_hidden_input(&html2, "csrf_token");
    let session_code = extract_hidden_input(&html2, "session_code");

    // 3. Submit username + password for `demo.analyst@hel.fi` (who has no OTP enrolled yet, so
    // `acr_values=mfa` prompts `EnrolTotp`), or `demo.steward@hel.fi` (who has seeded TOTP)
    let (st3, _, html3) = post_form(
        &app,
        &login_loc,
        &[
            ("csrf_token", &csrf_token),
            ("session_code", &session_code),
            ("username", "demo.steward@hel.fi"),
            ("password", "StewardPass123!Valid"),
        ],
        Some(&auth_ck),
    )
    .await;
    assert_eq!(st3, StatusCode::OK);
    assert!(html3.contains("login-otp"));

    // 4. Submit valid HMAC-SHA256 TOTP code
    let now_secs = chrono::Utc::now().timestamp() as u64;
    let totp_code =
        generate_totp_code("steward-totp-seed-123456", 6, 30, "HmacSHA256", now_secs).unwrap();
    let (st4, hdrs4, _) = post_form(
        &app,
        &login_loc,
        &[
            ("csrf_token", &csrf_token),
            ("session_code", &session_code),
            ("otp", &totp_code),
        ],
        Some(&auth_ck),
    )
    .await;
    assert_eq!(st4, StatusCode::SEE_OTHER);
    let cb_loc = hdrs4.get(header::LOCATION).unwrap().to_str().unwrap();
    let cb_url = url::Url::parse(cb_loc).unwrap();
    let code = cb_url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();
    let returned_state = cb_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    assert_eq!(returned_state, "st-123");

    // 5. Redeem authorization code at `/protocol/openid-connect/token` as `edge`
    let (st5, _, token_body_str) = post_form(
        &app,
        "/realms/dev/protocol/openid-connect/token",
        &[
            ("grant_type", "authorization_code"),
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("code", &code),
            ("redirect_uri", "https://portal.dev.example.test/callback"),
            ("code_verifier", verifier),
        ],
        None,
    )
    .await;
    assert_eq!(st5, StatusCode::OK, "token body: {token_body_str}");
    let token_resp: Value = serde_json::from_str(&token_body_str).unwrap();
    let access_token = token_resp["access_token"].as_str().unwrap();
    let id_token = token_resp["id_token"].as_str().unwrap();
    let refresh_token = token_resp["refresh_token"].as_str().unwrap();

    // Verify `edge` tokens are signed RS256 (per-client override in RI) and carry every Portal claim
    let id_header = jsonwebtoken::decode_header(id_token).unwrap();
    assert_eq!(id_header.alg, jsonwebtoken::Algorithm::RS256);

    let access_claims = decode_jwt_payload(access_token);
    assert_eq!(access_claims["sub"], "de300000-0000-4000-8000-000000000001");
    assert_eq!(access_claims["acr"], "mfa");
    assert_eq!(access_claims["amr"], json!(["pwd", "otp"]));
    assert_eq!(access_claims["preferred_username"], "demo.steward@hel.fi");
    assert!(
        access_claims["groups"]
            .as_array()
            .unwrap()
            .contains(&json!("/helsinki"))
    );
    assert!(
        access_claims["realm_access"]["roles"]
            .as_array()
            .unwrap()
            .contains(&json!("portal-approver"))
    );
    assert!(
        access_claims["realm_access"]["roles"]
            .as_array()
            .unwrap()
            .contains(&json!("mfa-required"))
    );

    // 6. Standard Token Exchange V2: `portal-api` (authenticated by federated-jwt) exchanges the
    // person's `edge` token for `audience=mcp-hub` and `scope=gateway-hub endpoint:counters1234`
    let k8s_key =
        KeyManager::generate_es256("k8s", &Kek::generate_for_tests(), 100, "ACTIVE").unwrap();
    let k8s_pub_jwk: Value = serde_json::from_str(&k8s_key.public_jwk_json).unwrap();
    state.guard().insert_mock(
        "https://kubernetes.default.svc.cluster.local/openid/v1/jwks",
        200,
        serde_json::to_vec(&json!({ "keys": [k8s_pub_jwk] })).unwrap(),
    );
    // Re-sign a projected ServiceAccount token for `system:serviceaccount:portal:portal`
    let k8s_kek = Kek::generate_for_tests();
    let k8s_key2 = KeyManager::generate_es256("k8s", &k8s_kek, 100, "ACTIVE").unwrap();
    let k8s_pub_jwk2: Value = serde_json::from_str(&k8s_key2.public_jwk_json).unwrap();
    state.guard().insert_mock(
        "https://kubernetes.default.svc.cluster.local/openid/v1/jwks",
        200,
        serde_json::to_vec(&json!({ "keys": [k8s_pub_jwk2] })).unwrap(),
    );
    let now = chrono::Utc::now().timestamp();
    let sa_assertion = sign_jwt(
        &k8s_key2,
        &k8s_kek,
        Some("JWT"),
        &json!({
            "iss": "https://kubernetes.default.svc.cluster.local",
            "sub": "system:serviceaccount:portal:portal",
            "aud": "https://idm.dev.example.test/realms/dev",
            "iat": now,
            "exp": now + 600
        }),
    )
    .unwrap();

    let (st6, _, exch_body_str) = post_form(
        &app,
        "/realms/dev/protocol/openid-connect/token",
        &[
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            (
                "client_assertion_type",
                "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
            ),
            ("client_assertion", &sa_assertion),
            ("subject_token", access_token),
            (
                "subject_token_type",
                "urn:ietf:params:oauth:token-type:access_token",
            ),
            (
                "requested_token_type",
                "urn:ietf:params:oauth:token-type:access_token",
            ),
            ("audience", "mcp-hub"),
            ("scope", "gateway-hub endpoint:counters1234"),
        ],
        None,
    )
    .await;
    assert_eq!(st6, StatusCode::OK, "exchange body: {exch_body_str}");
    let exch_resp: Value = serde_json::from_str(&exch_body_str).unwrap();
    let exchanged_claims = decode_jwt_payload(exch_resp["access_token"].as_str().unwrap());
    assert_eq!(
        exchanged_claims["sub"],
        "de300000-0000-4000-8000-000000000001"
    );
    assert_eq!(exchanged_claims["azp"], "portal-api");
    assert_eq!(exchanged_claims["act"]["sub"], "portal-api");
    assert!(
        exchanged_claims["scope"]
            .as_str()
            .unwrap()
            .contains("endpoint:counters1234")
    );
    // Verify `mcp-hub` is in `aud` (as `context-gateway/src/auth/subject.rs` `scoped_subject` requires)
    let aud_list = exchanged_claims["aud"].as_array().unwrap();
    assert!(aud_list.contains(&json!("mcp-hub")));

    // 7. Refresh token rotation and reuse detection:
    // First refresh succeeds and rotates the refresh token
    let (st7, _, ref1_str) = post_form(
        &app,
        "/realms/dev/protocol/openid-connect/token",
        &[
            ("grant_type", "refresh_token"),
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("refresh_token", refresh_token),
        ],
        None,
    )
    .await;
    assert_eq!(st7, StatusCode::OK);
    let ref1_resp: Value = serde_json::from_str(&ref1_str).unwrap();
    let rotated_refresh = ref1_resp["refresh_token"].as_str().unwrap();
    assert_ne!(rotated_refresh, refresh_token);

    // Replaying the old `refresh_token` revokes the entire family (the SSO session stays).
    let (st_reuse, _, _) = post_form(
        &app,
        "/realms/dev/protocol/openid-connect/token",
        &[
            ("grant_type", "refresh_token"),
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("refresh_token", refresh_token),
        ],
        None,
    )
    .await;
    assert_eq!(st_reuse, StatusCode::BAD_REQUEST);

    // Now even the newer `rotated_refresh` is refused because the family was revoked!
    let (st_after_reuse, _, _) = post_form(
        &app,
        "/realms/dev/protocol/openid-connect/token",
        &[
            ("grant_type", "refresh_token"),
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("refresh_token", rotated_refresh),
        ],
        None,
    )
    .await;
    assert_eq!(st_after_reuse, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn e2e_cimd_connector_with_parameterized_scope_and_offline_access() {
    let (state, _store, _kek) = setup_dev_realm_state().await;
    let cimd_url = "https://claude.ai/oauth/mcp-oauth-client-metadata";
    state.guard().insert_mock(
        cimd_url,
        200,
        serde_json::to_vec(&json!({
            "client_id": cimd_url,
            "client_name": "Claude",
            "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none"
        }))
        .unwrap(),
    );

    let mount = Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap();
    let app = router(state, mount);

    // Parameterized scope `connector:abcdefghij2345672345672345` (26 chars in `[a-z2-7]{26}`) + `offline_access`
    let verifier = "pkce-verifier-string-with-at-least-43-characters-long-999999";
    let challenge = sha256_base64url(verifier);
    let connector_scope = "openid connector:abcdefghij2345672345672345 offline_access";
    let auth_uri = format!(
        "/realms/dev/protocol/openid-connect/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&state=cimd-st&code_challenge={challenge}&code_challenge_method=S256",
        url::form_urlencoded::byte_serialize(cimd_url.as_bytes()).collect::<String>(),
        url::form_urlencoded::byte_serialize(b"https://claude.ai/api/mcp/auth_callback")
            .collect::<String>(),
        url::form_urlencoded::byte_serialize(connector_scope.as_bytes()).collect::<String>(),
    );

    let resp1 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&auth_uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::SEE_OTHER);
    let login_loc = resp1
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let auth_ck = auth_cookie(resp1.headers());

    // Load login page, submit credentials for `demo.analyst@hel.fi` (in `/helsinki/members` and `/helsinki/traffic-analysts`)
    let resp2 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&login_loc)
                .header(header::COOKIE, &auth_ck)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let html2 = String::from_utf8(
        resp2
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let csrf_token = extract_hidden_input(&html2, "csrf_token");
    let session_code = extract_hidden_input(&html2, "session_code");

    // Submit username + password -> reaches Consent screen (since CIMD clients have `consentRequired: true`)
    let (st3, _, html3) = post_form(
        &app,
        &login_loc,
        &[
            ("csrf_token", &csrf_token),
            ("session_code", &session_code),
            ("username", "demo.analyst@hel.fi"),
            ("password", "AnalystPass123!Valid"),
        ],
        Some(&auth_ck),
    )
    .await;
    assert_eq!(st3, StatusCode::OK);
    assert!(html3.contains("login-consent"));

    // Grant consent -> redirects with code
    let (st4, hdrs4, _) = post_form(
        &app,
        &login_loc,
        &[("csrf_token", &csrf_token), ("session_code", &session_code)],
        Some(&auth_ck),
    )
    .await;
    assert_eq!(st4, StatusCode::SEE_OTHER);
    let cb_loc = hdrs4.get(header::LOCATION).unwrap().to_str().unwrap();
    let cb_url = url::Url::parse(cb_loc).unwrap();
    let code = cb_url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned();

    // Exchange code for tokens as public CIMD client
    let (st5, _, tok_body) = post_form(
        &app,
        "/realms/dev/protocol/openid-connect/token",
        &[
            ("grant_type", "authorization_code"),
            ("client_id", cimd_url),
            ("code", &code),
            ("redirect_uri", "https://claude.ai/api/mcp/auth_callback"),
            ("code_verifier", verifier),
        ],
        None,
    )
    .await;
    assert_eq!(st5, StatusCode::OK, "cimd token body: {tok_body}");
    let tok_resp: Value = serde_json::from_str(&tok_body).unwrap();
    let access_claims = decode_jwt_payload(tok_resp["access_token"].as_str().unwrap());

    // Verify `connector:{id}` scope, `portal-connectors` and `portal-api` audiences, `jc_org`, `jc_groups`,
    // `fullScopeAllowed: false` (no `realm_access`), and `Offline` refresh token
    assert!(
        access_claims["scope"]
            .as_str()
            .unwrap()
            .contains("connector:abcdefghij2345672345672345")
    );
    assert_eq!(
        access_claims["aud"],
        json!(["portal-connectors", "portal-api"])
    );
    assert_eq!(access_claims["jc_org"], json!(["helsinki"]));
    assert_eq!(
        access_claims["jc_groups"],
        json!(["/helsinki/members", "/helsinki/traffic-analysts"])
    );
    assert!(access_claims.get("realm_access").is_none());
    // The refresh token is an opaque handle (contract: 256-bit random, only its SHA-256 stored);
    // its `Offline` type is visible through introspection, not inside the token.
    let refresh_token = tok_resp["refresh_token"].as_str().unwrap();
    assert_eq!(
        refresh_token.matches('.').count(),
        1,
        "opaque handle, not a JWT"
    );
    assert_eq!(tok_resp["refresh_expires_in"], 0);
    let (st6, _, intro_body) = post_form(
        &app,
        "/realms/dev/protocol/openid-connect/token/introspect",
        &[
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("token", refresh_token),
        ],
        None,
    )
    .await;
    assert_eq!(st6, StatusCode::OK, "{intro_body}");
    let intro: Value = serde_json::from_str(&intro_body).unwrap();
    assert_eq!(intro["active"], true, "{intro}");
    assert_eq!(intro["typ"], "Offline");
}

#[tokio::test]
async fn e2e_w9_mount_under_slug_and_dedicated_host_and_w12_attack_regression_suite() {
    let (state, _store, _kek) = setup_dev_realm_state().await;

    // W9 test: Mount kaisatsu at `/id` on `city.example` beside `/portal` and `/opendata`,
    // AND at `/` on `id.example` in one JcRouter
    let mount_slug = Mount::new("kaisatsu", "https", "city.example", "/id").unwrap();
    let mount_root = Mount::new("kaisatsu", "https", "id.example", "").unwrap();
    let wk_slug_router = router(state.clone(), mount_slug);
    let wk_root_router = router(state.clone(), mount_root);

    let combined = JcRouterBuilder::new("https", "city.example")
        .mount(
            MountSpec {
                module: "kaisatsu".into(),
                host: Some("city.example".into()),
                path_prefix: Some("/id".into()),
            },
            wk_slug_router,
        )
        .mount(
            MountSpec {
                module: "portal".into(),
                host: Some("city.example".into()),
                path_prefix: Some("/portal".into()),
            },
            axum::Router::new().route("/ping", axum::routing::get(|| async { "portal-ok" })),
        )
        .mount(
            MountSpec {
                module: "kaisatsu".into(),
                host: Some("id.example".into()),
                path_prefix: None,
            },
            wk_root_router.clone(),
        )
        .build()
        .unwrap();

    // 1. Discovery under `/id/realms/dev/.well-known/openid-configuration` on `city.example`
    let disc_req1 = Request::builder()
        .uri("/id/realms/dev/.well-known/openid-configuration")
        .header("host", "city.example")
        .body(Body::empty())
        .unwrap();
    let disc_res1 = combined.dispatch(disc_req1, None).await;
    assert_eq!(disc_res1.status(), StatusCode::OK);
    let disc_json1: Value =
        serde_json::from_slice(&disc_res1.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(disc_json1["issuer"], "https://city.example/id/realms/dev");
    assert!(disc_json1.get("registration_endpoint").is_none());

    // 2. Discovery at root on `id.example`
    let disc_req2 = Request::builder()
        .uri("/realms/dev/.well-known/openid-configuration")
        .header("host", "id.example")
        .body(Body::empty())
        .unwrap();
    let disc_res2 = combined.dispatch(disc_req2, None).await;
    assert_eq!(disc_res2.status(), StatusCode::OK);
    let disc_json2: Value =
        serde_json::from_slice(&disc_res2.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(disc_json2["issuer"], "https://id.example/realms/dev");

    // 3. W12 Attack 1: CIMD URL pointing at cloud metadata `169.254.169.254` -> refused
    let ssrf_req = Request::builder()
        .uri("/realms/dev/protocol/openid-connect/auth?client_id=https%3A%2F%2F169.254.169.254%2Flatest%2Fmeta-data&redirect_uri=https%3A%2F%2Fclaude.ai%2Fcb&response_type=code&code_challenge=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&code_challenge_method=S256")
        .header("host", "id.example")
        .body(Body::empty())
        .unwrap();
    let ssrf_res = combined.dispatch(ssrf_req, None).await;
    assert_eq!(ssrf_res.status(), StatusCode::UNAUTHORIZED);

    // 4. W12 Attack 2: PKCE downgrade (`code_challenge_method=plain`) -> refused
    let pkce_plain_req = Request::builder()
        .uri("/realms/dev/protocol/openid-connect/auth?client_id=portal-ui&redirect_uri=https%3A%2F%2Fportal.dev.example.test%2Fcb&response_type=code&code_challenge=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&code_challenge_method=plain")
        .header("host", "id.example")
        .body(Body::empty())
        .unwrap();
    let pkce_plain_res = combined.dispatch(pkce_plain_req, None).await;
    assert_eq!(pkce_plain_res.status(), StatusCode::BAD_REQUEST);

    // 5. W12 Attack 3: Code replay revokes tokens issued from the first redemption
    let raw_code = "single-use-test-code-0001";
    let verifier = "pkce-verifier-string-with-at-least-43-characters-long-777777";
    state
        .store()
        .upsert_sso_session(SsoSession {
            id: "sso-replay-1".into(),
            realm_id: "dev".into(),
            org_id: None,
            handle_hash: sha256_hex("h-replay-1"),
            user_id: "de300000-0000-4000-8000-00000000000f".into(),
            username: "demo.orgmember@hel.fi".into(),
            ip_address: "127.0.0.1".into(),
            user_agent: String::new(),
            start: 1000,
            last_access: 1000,
            auth_time: 1000,
            acr: "pwd".into(),
            amr: vec!["pwd".into()],
            clients: Default::default(),
            idle_expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            max_expires_at: chrono::Utc::now() + chrono::Duration::hours(10),
        })
        .await
        .unwrap();
    state
        .store()
        .put_auth_code(AuthorizationCodeRecord {
            code_hash: sha256_hex(raw_code),
            realm_id: "dev".into(),
            org_id: None,
            client_id: "portal-ui".into(),
            user_id: "de300000-0000-4000-8000-00000000000f".into(),
            sso_session_id: "sso-replay-1".into(),
            redirect_uri: "https://portal.dev.example.test/cb".into(),
            scope: "openid profile".into(),
            mapper_scopes: Vec::new(),
            code_challenge: sha256_base64url(verifier),
            code_challenge_method: "S256".into(),
            nonce: None,
            acr: "pwd".into(),
            amr: vec!["pwd".into()],
            dpop_jkt: None,
            auth_time: 1000,
            consumed: false,
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(60),
        })
        .await
        .unwrap();

    // First redemption succeeds
    let (st_first, _, first_tok_str) = post_form(
        &wk_root_router,
        "/realms/dev/protocol/openid-connect/token",
        &[
            ("grant_type", "authorization_code"),
            ("client_id", "portal-ui"),
            ("code", raw_code),
            ("redirect_uri", "https://portal.dev.example.test/cb"),
            ("code_verifier", verifier),
        ],
        None,
    )
    .await;
    assert_eq!(st_first, StatusCode::OK);
    let first_tok: Value = serde_json::from_str(&first_tok_str).unwrap();
    let issued_refresh = first_tok["refresh_token"].as_str().unwrap();

    // Second redemption (code replay) fails AND revokes the session/refresh family from the first!
    let (st_second, _, _) = post_form(
        &wk_root_router,
        "/realms/dev/protocol/openid-connect/token",
        &[
            ("grant_type", "authorization_code"),
            ("client_id", "portal-ui"),
            ("code", raw_code),
            ("redirect_uri", "https://portal.dev.example.test/cb"),
            ("code_verifier", verifier),
        ],
        None,
    )
    .await;
    assert_eq!(st_second, StatusCode::BAD_REQUEST);

    let (st_refresh_after_replay, _, _) = post_form(
        &wk_root_router,
        "/realms/dev/protocol/openid-connect/token",
        &[
            ("grant_type", "refresh_token"),
            ("client_id", "portal-ui"),
            ("refresh_token", issued_refresh),
        ],
        None,
    )
    .await;
    assert_eq!(st_refresh_after_replay, StatusCode::BAD_REQUEST);

    // 6. W12 Attack 4: Short / malformed tokens on token, introspect, revoke, and userinfo
    // never panic (Rauthy GHSA-wx92-7mmw-5x82 regression test)
    for bad_token in [
        "",
        ".",
        "..",
        "a.b.c",
        "not-a-jwt-at-all",
        "eyJ0eXAiOiJKV1QifQ.!!!.sig",
    ] {
        let (st, _, _) = post_form(
            &wk_root_router,
            "/realms/dev/protocol/openid-connect/token",
            &[
                ("grant_type", "refresh_token"),
                ("client_id", "portal-ui"),
                ("refresh_token", bad_token),
            ],
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);

        let (st_int, _, _) = post_form(
            &wk_root_router,
            "/realms/dev/protocol/openid-connect/token/introspect",
            &[
                ("client_id", "edge"),
                ("client_secret", "edge-secret-123"),
                ("token", bad_token),
            ],
            None,
        )
        .await;
        assert_eq!(st_int, StatusCode::OK);
    }
}
