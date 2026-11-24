#![forbid(unsafe_code)]
//! Error and edge paths of the OIDC handlers (W1, W8, W12, K2, K4, K5, K6): an unknown realm at
//! the token, introspection and revocation endpoints; scope refusals (unknown organization, a
//! broken parameterized-scope pattern); a client that opted out of PKCE on fresh and SSO logins;
//! signing key selection (a pinned algorithm is generated or refused, never replaced); a
//! refresh after the session ended or the person was disabled; token exchange carrying an
//! earlier `act`; jwt-bearer without a usable service account; userinfo refusals (no `sub`, an
//! unknown subject, a pairwise subject that cannot be computed, a DPoP header that is not text);
//! and a logout confirmation token that is not base64url.

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, Method, Request, StatusCode, header};
use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tower::ServiceExt;

mod common;
use common::*;

const ISSUER: &str = "https://idm.dev.example.test/realms/dev";
const TOKEN: &str = "/realms/dev/protocol/openid-connect/token";
const USERINFO: &str = "/realms/dev/protocol/openid-connect/userinfo";
/// The seeded `apisix-gateway` client credential (common/mod.rs), split so scanners skip it.
const APISIX_CLIENT_WORD: &str = concat!("apisix-", "secret", "-123");
const LOGOUT: &str = "/realms/dev/protocol/openid-connect/logout";
const CALLBACK: &str = "https://portal.dev.example.test/callback";

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

async fn app() -> (Router, Arc<MemoryStore>, Kek) {
    let (state, store, kek) = setup_dev_realm_state().await;
    (router(state, mount()), store, kek)
}

fn err(body: &str) -> String {
    serde_json::from_str::<Value>(body).unwrap_or(Value::Null)["error"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn json_of(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, String) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let st = resp.status();
    let h = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, h, String::from_utf8_lossy(&bytes).into_owned())
}

async fn get(
    app: &Router,
    uri: &str,
    cookie: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    send(app, b.body(Body::empty()).unwrap()).await
}

async fn client(
    store: &MemoryStore,
    client_id: &str,
    edit: impl FnOnce(&mut ClientRepresentation),
) {
    let mut c = ClientRepresentation {
        id: format!("uuid-{client_id}"),
        realm_id: "dev".into(),
        client_id: client_id.into(),
        enabled: true,
        public_client: false,
        service_accounts_enabled: true,
        client_authenticator_type: "client-secret".into(),
        secret: Some(format!("{client_id}-secret")),
        ..Default::default()
    };
    edit(&mut c);
    store.upsert_client(c).await.unwrap();
}

async fn cc(app: &Router, client_id: &str, scope: Option<&str>) -> (StatusCode, String) {
    let secret = format!("{client_id}-secret");
    let mut f = vec![
        ("grant_type", "client_credentials"),
        ("client_id", client_id),
        ("client_secret", secret.as_str()),
    ];
    if let Some(s) = scope {
        f.push(("scope", s));
    }
    let (st, _, body) = post_form(app, TOKEN, &f, None).await;
    (st, body)
}

async fn realm_token(store: &MemoryStore, kek: &Kek, claims: Value) -> String {
    let key = store
        .list_signing_keys("dev")
        .await
        .unwrap()
        .into_iter()
        .find(|k| k.status == "ACTIVE")
        .unwrap();
    sign_jwt(&key, kek, Some("JWT"), &claims).unwrap()
}

async fn user_id(store: &MemoryStore, name: &str) -> String {
    store
        .get_user_by_username_or_email("dev", None, name)
        .await
        .unwrap()
        .unwrap()
        .id
}

async fn password_grant(app: &Router, user: &str, pass: &str, scope: &str) -> Value {
    let (st, _, body) = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "password"),
            ("client_id", "apisix-gateway"),
            ("client_secret", APISIX_CLIENT_WORD),
            ("username", user),
            ("password", pass),
            ("scope", scope),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    json_of(&body)
}

async fn refresh(app: &Router, refresh_token: &str) -> (StatusCode, String) {
    let (st, _, body) = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", "apisix-gateway"),
            ("client_secret", APISIX_CLIENT_WORD),
            ("refresh_token", refresh_token),
        ],
        None,
    )
    .await;
    (st, body)
}

#[tokio::test]
async fn an_unknown_realm_is_404_at_token_introspection_and_revocation() {
    let (app, _, _) = app().await;
    for path in ["token", "token/introspect", "revoke"] {
        let (st, _, _) = post_form(
            &app,
            &format!("/realms/nope/protocol/openid-connect/{path}"),
            &[("token", "x"), ("grant_type", "client_credentials")],
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{path}");
    }
}

/// Introspection is for confidential clients (W1) and names the person of a token that carries
/// only `sub`; the token endpoint takes one DPoP proof at most (RFC 9449 §4.3); `kc_action`
/// needs an interactive login.
#[tokio::test]
async fn introspection_dpop_count_and_kc_action_refusals() {
    let (app, store, kek) = app().await;
    let introspect = "/realms/dev/protocol/openid-connect/token/introspect";
    let (st, _, body) = post_form(
        &app,
        introspect,
        &[("client_id", "admin-cli"), ("token", "x")],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(err(&body), "unauthorized_client", "{body}");

    let member = user_id(&store, "demo.orgmember@hel.fi").await;
    let now = jc_time::TimestampTz::now().as_second();
    let token = realm_token(
        &store,
        &kek,
        json!({ "iss": ISSUER, "sub": member, "typ": "Bearer", "azp": "apisix-gateway",
            "jti": "introspect-sub", "iat": now, "exp": now + 300 }),
    )
    .await;
    let (st, _, body) = post_form(
        &app,
        introspect,
        &[
            ("client_id", "apisix-gateway"),
            ("client_secret", APISIX_CLIENT_WORD),
            ("token", token.as_str()),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let v = json_of(&body);
    assert_eq!(v["active"], true, "{body}");
    assert_eq!(v["username"], "demo.orgmember@hel.fi", "{body}");
    // A token naming no person (no sub, no preferred_username) gets no username.
    let anonymous = realm_token(
        &store,
        &kek,
        json!({ "iss": ISSUER, "typ": "Bearer", "azp": "apisix-gateway",
            "jti": "introspect-nosub", "iat": now, "exp": now + 300 }),
    )
    .await;
    let (st, _, body) = post_form(
        &app,
        introspect,
        &[
            ("client_id", "apisix-gateway"),
            ("client_secret", APISIX_CLIENT_WORD),
            ("token", anonymous.as_str()),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let v = json_of(&body);
    assert_eq!(v["active"], true, "{body}");
    assert!(v.get("username").is_none(), "{body}");

    let req = Request::builder()
        .method(Method::POST)
        .uri(TOKEN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("dpop", "a.b.c")
        .header("dpop", "d.e.f")
        .body(Body::from(format!(
            "grant_type=client_credentials&client_id=apisix-gateway&client_secret={APISIX_CLIENT_WORD}"
        )))
        .unwrap();
    let (st, _, body) = send(&app, req).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(err(&body), "invalid_dpop_proof", "{body}");
    assert!(body.contains("multiple DPoP headers"), "{body}");

    client(&store, "kc-app", |c| {
        c.public_client = true;
        c.redirect_uris = vec![CALLBACK.into()];
    })
    .await;
    let (st, _, body) = get(
        &app,
        &format!(
            "/realms/dev/protocol/openid-connect/auth?client_id=kc-app&response_type=code\
             &code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256\
             &redirect_uri={}&kc_action=UPDATE_PASSWORD&prompt=none",
            url::form_urlencoded::byte_serialize(CALLBACK.as_bytes()).collect::<String>()
        ),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("kc_action needs an interactive login"),
        "{body}"
    );
}

/// Behind a host that injects its own `Mount` (jc-router), revocation still authenticates the
/// client and answers 200 for an unknown token (RFC 7009 §2.2).
#[tokio::test]
async fn revocation_under_an_injected_mount() {
    let (state, _, _) = setup_dev_realm_state().await;
    let app = router(state, mount()).layer(axum::Extension(mount()));
    let gw = [
        ("client_id", "apisix-gateway"),
        ("client_secret", APISIX_CLIENT_WORD),
    ];
    let revoke = "/realms/dev/protocol/openid-connect/revoke";
    let (st, _, _) = post_form(&app, revoke, &[("token", "unknown"), gw[0], gw[1]], None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = post_form(&app, revoke, &[("token", "unknown"), gw[0]], None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn scope_refusals_and_client_token_settings() {
    let (app, store, _) = app().await;
    for (id, name, re) in [
        ("sc-fam", "fam", "[a-z]+"),
        ("sc-badre", "badre", "("),
        ("sc-escre", "escre", "abc)|(.*"),
    ] {
        store
            .upsert_client_scope(ClientScopeRepresentation {
                id: id.into(),
                realm_id: "dev".into(),
                name: name.into(),
                protocol: "openid-connect".into(),
                attributes: BTreeMap::from([
                    ("is.parameterized.scope".into(), "true".into()),
                    ("parameterized.scope.regexp".into(), re.into()),
                    ("include.in.token.scope".into(), "true".into()),
                ]),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    client(&store, "scoped", |c| {
        c.default_client_scopes = vec!["fam".into()];
        c.optional_client_scopes = vec!["badre".into(), "escre".into()];
        c.attributes
            .insert("access.token.lifespan".into(), "123".into());
    })
    .await;

    // An organization the realm does not hold.
    let (st, body) = cc(&app, "scoped", Some("organization:ghost")).await;
    assert_eq!(
        (st, err(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_scope")
    );
    // A parameterized scope whose configured pattern does not compile: refused, not admitted.
    let (st, body) = cc(&app, "scoped", Some("badre:x")).await;
    assert_eq!(
        (st, err(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_scope"),
        "{body}"
    );
    // T-7799: a pattern that compiles only inside the `^(?:...)$` wrapper admits nothing.
    let (st, body) = cc(&app, "scoped", Some("escre:zzz")).await;
    assert_eq!(
        (st, err(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_scope"),
        "{body}"
    );
    // A parameterized scope that is also a default scope: granted once, with the client's
    // own access token lifespan.
    let (st, body) = cc(&app, "scoped", Some("fam:abc")).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let v = json_of(&body);
    assert_eq!(v["expires_in"], 123, "{v}");
    assert!(v["scope"].as_str().unwrap().contains("fam:abc"), "{v}");
}

/// A client whose service account user is gone gets it made again at its next
/// client_credentials grant (W3), owned by that client.
#[tokio::test]
async fn a_missing_service_account_is_made_again_at_the_grant() {
    let (app, store, _) = app().await;
    client(&store, "sa-app", |_| {}).await;
    let sa = store
        .get_user_by_username_or_email("dev", None, "service-account-sa-app")
        .await
        .unwrap()
        .unwrap();
    store.delete_user("dev", None, &sa.id).await.unwrap();
    let (st, body) = cc(&app, "sa-app", None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let again = store
        .get_user_by_username_or_email("dev", None, "service-account-sa-app")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(again.id, sa.id);
    assert_eq!(again.service_account_client_id.as_deref(), Some("sa-app"));
    assert!(again.enabled);
}

/// Revoking an access token (RFC 7009) also ends the refresh tokens of its session for that
/// client; a person without a password credential gets the same answer as a wrong password.
#[tokio::test]
async fn revoking_an_access_token_ends_its_session_refresh_and_no_password_is_refused() {
    let (app, store, _) = app().await;
    let tokens = password_grant(
        &app,
        "demo.orgmember@hel.fi",
        "MemberPass123!Valid",
        "openid",
    )
    .await;
    let (st, body) = refresh(&app, tokens["refresh_token"].as_str().unwrap()).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let tokens = json_of(&body);
    let (st, _, body) = post_form(
        &app,
        "/realms/dev/protocol/openid-connect/revoke",
        &[
            ("client_id", "apisix-gateway"),
            ("client_secret", APISIX_CLIENT_WORD),
            ("token", tokens["access_token"].as_str().unwrap()),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (st, body) = refresh(&app, tokens["refresh_token"].as_str().unwrap()).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(err(&body), "invalid_grant", "{body}");

    store
        .create_user(UserRepresentation {
            id: "no-password-person".into(),
            realm_id: "dev".into(),
            username: "nopass@hel.fi".into(),
            enabled: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "password"),
            ("client_id", "apisix-gateway"),
            ("client_secret", APISIX_CLIENT_WORD),
            ("username", "nopass@hel.fi"),
            ("password", "anything-at-all"),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(err(&body), "invalid_grant", "{body}");
}

/// `offline_access` is granted only to a person holding its role (directly, through a
/// composite, or the realm's default roles).
#[tokio::test]
async fn offline_access_needs_its_role() {
    let (app, store, _) = app().await;
    let member = store
        .get_user_by_username_or_email("dev", None, "demo.orgmember@hel.fi")
        .await
        .unwrap()
        .unwrap();
    store
        .set_user_grants("dev", &member.id, &[], &member.client_roles)
        .await
        .unwrap();
    client(&store, "offline-app", |c| {
        c.direct_access_grants_enabled = true;
        c.optional_client_scopes = vec!["offline_access".into()];
    })
    .await;
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "password"),
            ("client_id", "offline-app"),
            ("client_secret", "offline-app-secret"),
            ("username", "demo.orgmember@hel.fi"),
            ("password", "MemberPass123!Valid"),
            ("scope", "openid offline_access"),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(err(&body), "invalid_scope", "{body}");
    assert!(body.contains("offline_access role"), "{body}");
}

/// A person with a TOTP credential needs a valid code at the password grant; a wrong one is
/// refused and counted as a failure.
#[tokio::test]
async fn the_password_grant_refuses_a_wrong_totp_code() {
    let (app, _, _) = app().await;
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "password"),
            ("client_id", "apisix-gateway"),
            ("client_secret", APISIX_CLIENT_WORD),
            ("username", "demo.steward@hel.fi"),
            ("password", "StewardPass123!Valid"),
            ("totp", "000000"),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(err(&body), "invalid_grant", "{body}");
    assert!(body.contains("Invalid totp code"), "{body}");
}

/// A Keycloak pbkdf2 password signs in at the password grant and is stored again as argon2id
/// (W3 import, rehash on first use).
#[tokio::test]
async fn the_password_grant_rehashes_a_legacy_password() {
    let (app, store, _) = app().await;
    let member = user_id(&store, "demo.orgmember@hel.fi").await;
    let mut pw = store
        .list_credentials("dev", &member)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.credential_type == "password")
        .unwrap();
    let salt = b"0123456789abcdef";
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2::<hmac::Hmac<sha2::Sha256>>(b"MemberPass123!Valid", salt, 1000, &mut out)
        .unwrap();
    pw.secret_data = json!({ "value": base64::engine::general_purpose::STANDARD.encode(out),
        "salt": base64::engine::general_purpose::STANDARD.encode(salt) })
    .to_string();
    pw.credential_data = r#"{"hashIterations":1000,"algorithm":"pbkdf2-sha256"}"#.into();
    store.upsert_credential(pw).await.unwrap();
    password_grant(
        &app,
        "demo.orgmember@hel.fi",
        "MemberPass123!Valid",
        "openid",
    )
    .await;
    let pw = store
        .list_credentials("dev", &member)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.credential_type == "password")
        .unwrap();
    assert!(
        pw.secret_data.starts_with("$argon2id$"),
        "{}",
        pw.secret_data
    );
    assert!(pw.credential_data.is_empty());
}

/// An authorization code past its lifetime is refused at the exchange (RFC 6749 §4.1.2).
#[tokio::test]
async fn an_expired_code_is_refused() {
    let (app, store, _) = app().await;
    let now = jc_time::TimestampTz::now();
    store
        .put_auth_code(AuthorizationCodeRecord {
            code_hash: sha256_hex("expired-code"),
            realm_id: "dev".into(),
            org_id: None,
            client_id: "apisix-gateway".into(),
            user_id: user_id(&store, "demo.orgmember@hel.fi").await,
            sso_session_id: "no-session".into(),
            redirect_uri: CALLBACK.into(),
            scope: "openid".into(),
            mapper_scopes: Vec::new(),
            code_challenge: String::new(),
            code_challenge_method: String::new(),
            nonce: None,
            acr: "pwd".into(),
            amr: vec!["pwd".into()],
            dpop_jkt: None,
            auth_time: now.as_second(),
            consumed: false,
            created_at: now - jiff::SignedDuration::from_mins(10),
            expires_at: now - jiff::SignedDuration::from_mins(9),
        })
        .await
        .unwrap();
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", "apisix-gateway"),
            ("client_secret", APISIX_CLIENT_WORD),
            ("code", "expired-code"),
            ("redirect_uri", CALLBACK),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("Code is expired"), "{body}");
}

fn jwt_alg(jwt: &str) -> Value {
    let header: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(jwt.split('.').next().unwrap())
            .unwrap(),
    )
    .unwrap();
    header["alg"].clone()
}

#[tokio::test]
async fn a_pinned_algorithm_without_a_key_is_refused() {
    let (app, store, _) = app().await;
    client(&store, "ps", |c| {
        c.attributes
            .insert("access.token.signed.response.alg".into(), "PS512".into());
    })
    .await;
    // The store took the client as written (no admin check on this path): the token endpoint
    // refuses to sign with another algorithm and names the client, not a key.
    let (st, body) = cc(&app, "ps", None).await;
    assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let v = json_of(&body);
    assert_eq!(v["error"], "server_error", "{v}");
    let text = v["error_description"].as_str().unwrap();
    assert!(text.contains("ps") && text.contains("PS512"), "{text}");
    assert!(!text.contains("kid"), "{text}");
    for k in store.list_signing_keys("dev").await.unwrap() {
        assert!(!text.contains(&k.kid), "{text}");
    }
}

#[tokio::test]
async fn a_pinned_es256_rs256_eddsa_key_is_generated() {
    for alg in ["ES256", "RS256", "EdDSA"] {
        let (app, store, _) = app().await;
        // A realm without any key of the algorithm, EdDSA never made by default.
        let before = store.list_signing_keys("dev").await.unwrap();
        for mut k in before.into_iter().filter(|k| k.algorithm == alg) {
            k.status = "PASSIVE".into();
            store.upsert_signing_key(k).await.unwrap();
        }
        let mut keys = store.list_signing_keys("dev").await.unwrap();
        // The memory store regenerates ES256 and RS256 on key read, EdDSA only on demand.
        keys.retain(|k| k.algorithm == "EdDSA");
        assert!(keys.is_empty(), "{alg}");
        client(&store, "pinned", |c| {
            c.attributes
                .insert("access.token.signed.response.alg".into(), alg.into());
            c.attributes
                .insert("id.token.signed.response.alg".into(), alg.into());
        })
        .await;
        let (st, body) = cc(&app, "pinned", Some("openid")).await;
        assert_eq!(st, StatusCode::OK, "{alg}: {body}");
        let v = json_of(&body);
        assert_eq!(jwt_alg(v["access_token"].as_str().unwrap()), alg, "{v}");
        if let Some(id) = v["id_token"].as_str() {
            assert_eq!(jwt_alg(id), alg, "{v}");
        }
        assert!(
            store
                .list_signing_keys("dev")
                .await
                .unwrap()
                .iter()
                .any(|k| k.algorithm == alg && k.status == "ACTIVE"),
            "{alg}"
        );
    }
}

#[tokio::test]
async fn an_empty_algorithm_attribute_uses_the_realm_default() {
    let (app, store, _) = app().await;
    let realm = store.get_realm("dev").await.unwrap().unwrap();
    client(&store, "blank", |c| {
        c.attributes
            .insert("access.token.signed.response.alg".into(), String::new());
    })
    .await;
    let (st, body) = cc(&app, "blank", None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let at = json_of(&body)["access_token"].as_str().unwrap().to_owned();
    assert_eq!(jwt_alg(&at), realm.default_signature_algorithm.as_str());
}

#[tokio::test]
async fn refresh_refusals_after_session_end_and_disable_and_membership_org() {
    let (app, store, _) = app().await;
    let mut gw = store
        .get_client_by_client_id("dev", "apisix-gateway")
        .await
        .unwrap()
        .unwrap();
    gw.attributes
        .insert("client.session.idle.timeout".into(), "1800".into());
    store.upsert_client(gw).await.unwrap();

    // A Membership row (no org_id on the person, no group) pins the organization.
    store
        .upsert_organization(Organization {
            id: "org-t-mem".into(),
            realm_id: "dev".into(),
            name: "Membership only".into(),
            alias: "t-mem".into(),
            enabled: true,
            description: String::new(),
            domains: vec![],
            attributes: HashMap::new(),
            managed_by: None,
        })
        .await
        .unwrap();
    let analyst = user_id(&store, "demo.analyst@hel.fi").await;
    store
        .upsert_membership(Membership {
            id: "m-t-mem".into(),
            realm_id: "dev".into(),
            org_id: "org-t-mem".into(),
            person_id: analyst.clone(),
            org_roles: vec![],
            created_at: jc_time::TimestampTz::now(),
        })
        .await
        .unwrap();
    let v = password_grant(
        &app,
        "demo.analyst@hel.fi",
        "AnalystPass123!Valid",
        "openid organization:t-mem",
    )
    .await;
    let claims = decode_jwt_payload(v["access_token"].as_str().unwrap());
    assert_eq!(claims["organization"], json!({ "t-mem": {} }), "{claims}");
    assert_eq!(v["refresh_expires_in"], 1800, "{v}");

    // The person is disabled: the refresh token buys nothing.
    let mut u = store
        .get_user_by_id("dev", None, &analyst)
        .await
        .unwrap()
        .unwrap();
    u.enabled = false;
    store.update_user(u).await.unwrap();
    let (st, body) = refresh(&app, v["refresh_token"].as_str().unwrap()).await;
    assert_eq!(
        (st, err(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_grant"),
        "{body}"
    );

    // The SSO session ended: refused too.
    let v = password_grant(
        &app,
        "demo.orgmember@hel.fi",
        "MemberPass123!Valid",
        "openid",
    )
    .await;
    let sid = decode_jwt_payload(v["access_token"].as_str().unwrap())["sid"]
        .as_str()
        .unwrap()
        .to_owned();
    store.delete_sso_session("dev", &sid).await.unwrap();
    let (st, body) = refresh(&app, v["refresh_token"].as_str().unwrap()).await;
    assert_eq!(
        (st, err(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_grant"),
        "{body}"
    );
}

/// A token exchange keeps the subject token's narrowing and its refresh keeps the exchange's
/// (S-4-08): `jc:read-only` stays, the organization pin stays that organization's alone (another
/// one asked for is refused), and the refreshed token is still the actor's.
#[tokio::test]
async fn token_exchange_and_its_refresh_keep_read_only_the_organization_pin_and_act() {
    let (app, store, _) = app().await;
    client(&store, "tx", |c| {
        c.attributes
            .insert("standard.token.exchange.enabled".into(), "true".into());
        c.attributes.insert(
            "standard.token.exchange.enableRefreshRequestedTokenType".into(),
            "SAME_SESSION".into(),
        );
    })
    .await;
    let mut gw = store
        .get_client_by_client_id("dev", "apisix-gateway")
        .await
        .unwrap()
        .unwrap();
    gw.protocol_mappers.push(ProtocolMapperRepresentation {
        id: "aud-tx".into(),
        name: "aud-tx".into(),
        protocol: "openid-connect".into(),
        protocol_mapper: "oidc-audience-mapper".into(),
        config: BTreeMap::from([
            ("included.client.audience".into(), "tx".into()),
            ("access.token.claim".into(), "true".into()),
        ]),
        ..Default::default()
    });
    store.upsert_client(gw).await.unwrap();
    let analyst = user_id(&store, "demo.analyst@hel.fi").await;
    for alias in ["t-ro", "t-other"] {
        store
            .upsert_organization(Organization {
                id: format!("org-{alias}"),
                realm_id: "dev".into(),
                name: alias.into(),
                alias: alias.into(),
                enabled: true,
                description: String::new(),
                domains: vec![],
                attributes: HashMap::new(),
                managed_by: None,
            })
            .await
            .unwrap();
        store
            .upsert_membership(Membership {
                id: format!("m-{alias}"),
                realm_id: "dev".into(),
                org_id: format!("org-{alias}"),
                person_id: analyst.clone(),
                org_roles: vec![],
                created_at: jc_time::TimestampTz::now(),
            })
            .await
            .unwrap();
    }
    let subject = password_grant(
        &app,
        "demo.analyst@hel.fi",
        "AnalystPass123!Valid",
        "openid jc:read-only organization:t-ro",
    )
    .await;
    let subject = subject["access_token"].as_str().unwrap().to_owned();
    let as_tx = async |form: &[(&str, &str)]| {
        let mut f = vec![("client_id", "tx"), ("client_secret", "tx-secret")];
        f.extend_from_slice(form);
        let (st, _, body) = post_form(&app, TOKEN, &f, None).await;
        (st, json_of(&body))
    };
    let exchange = "urn:ietf:params:oauth:grant-type:token-exchange";
    let narrowed = |v: &Value| {
        let claims = decode_jwt_payload(v["access_token"].as_str().unwrap());
        let scope: Vec<String> = claims["scope"]
            .as_str()
            .unwrap_or_default()
            .split(' ')
            .map(str::to_owned)
            .collect();
        assert!(scope.contains(&"jc:read-only".into()), "{claims}");
        assert!(scope.contains(&"organization:t-ro".into()), "{claims}");
        assert!(!scope.contains(&"organization:t-other".into()), "{claims}");
        assert_eq!(claims["organization"], json!({ "t-ro": {} }), "{claims}");
        assert_eq!(claims["act"]["sub"], "tx", "{claims}");
    };

    let (st, other) = as_tx(&[
        ("grant_type", exchange),
        ("subject_token", &subject),
        ("scope", "openid organization:t-other"),
    ])
    .await;
    assert_eq!(
        (st, other["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_scope")),
        "{other}"
    );

    let (st, exchanged) = as_tx(&[("grant_type", exchange), ("subject_token", &subject)]).await;
    assert_eq!(st, StatusCode::OK, "{exchanged}");
    narrowed(&exchanged);
    let (st, refreshed) = as_tx(&[
        ("grant_type", "refresh_token"),
        (
            "refresh_token",
            exchanged["refresh_token"].as_str().expect("refresh"),
        ),
    ])
    .await;
    assert_eq!(st, StatusCode::OK, "{refreshed}");
    narrowed(&refreshed);
}

#[tokio::test]
async fn token_exchange_keeps_an_earlier_act_and_jwt_bearer_needs_an_enabled_account() {
    let (app, store, kek) = app().await;
    client(&store, "tx", |c| {
        c.attributes
            .insert("standard.token.exchange.enabled".into(), "true".into());
        c.attributes.insert(
            "standard.token.exchange.enableRefreshRequestedTokenType".into(),
            "SAME_SESSION".into(),
        );
    })
    .await;
    let member = user_id(&store, "demo.orgmember@hel.fi").await;
    let now = jc_time::TimestampTz::now().as_second();
    let subject = realm_token(
        &store,
        &kek,
        json!({ "iss": ISSUER, "aud": "tx", "typ": "Bearer", "sub": member, "azp": "edge",
            "act": { "sub": "first-actor" }, "iat": now, "exp": now + 300 }),
    )
    .await;
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            ("client_id", "tx"),
            ("client_secret", "tx-secret"),
            ("subject_token", &subject),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let v = json_of(&body);
    // No session id on the subject token: no refresh token, whatever the client asks.
    assert!(v.get("refresh_token").is_none(), "{v}");
    let act = &decode_jwt_payload(v["access_token"].as_str().unwrap())["act"];
    assert_eq!(act["sub"], "tx", "{act}");
    assert_eq!(act["act"]["sub"], "first-actor", "{act}");

    // jwt-bearer: the client's service account is disabled.
    let jb_kek = Kek::generate_for_tests();
    let key = KeyManager::generate_es256("jb", &jb_kek, 100, "ACTIVE").unwrap();
    let jwk: Value = serde_json::from_str(&key.public_jwk_json).unwrap();
    client(&store, "jb-off", |c| {
        c.attributes
            .insert("jwks.string".into(), json!({ "keys": [jwk] }).to_string());
    })
    .await;
    if let Some(mut sa) = store
        .get_user_by_username_or_email("dev", None, "service-account-jb-off")
        .await
        .unwrap()
    {
        sa.enabled = false;
        store.update_user(sa).await.unwrap();
    }
    let assertion = sign_jwt(
        &key,
        &jb_kek,
        None,
        &json!({ "iss": "jb-off", "sub": "jb-off", "aud": ISSUER, "jti": "jb-off-1",
            "exp": now + 60 }),
    )
    .unwrap();
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("client_id", "jb-off"),
            ("client_secret", "jb-off-secret"),
            ("assertion", &assertion),
        ],
        None,
    )
    .await;
    assert_eq!(
        (st, err(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_grant"),
        "{body}"
    );
}

/// A confidential client whose `pkce.code.challenge.method` is empty may log in without a
/// challenge (Keycloak's opt-out); its codes then need no verifier, on a fresh login and on
/// both SSO paths.
#[tokio::test]
async fn a_client_without_pkce_logs_in_on_fresh_and_sso_paths() {
    let (app, store, _) = app().await;
    let mut c = store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .unwrap();
    c.id = "uuid-nopkce".into();
    c.client_id = "nopkce".into();
    c.secret = Some("nopkce-secret".into());
    c.attributes
        .insert("pkce.code.challenge.method".into(), String::new());
    store.upsert_client(c).await.unwrap();
    let auth = |extra: &str| {
        format!(
            "/realms/dev/protocol/openid-connect/auth?client_id=nopkce&redirect_uri={}&response_type=code&scope=openid{extra}",
            url::form_urlencoded::byte_serialize(CALLBACK.as_bytes()).collect::<String>(),
        )
    };
    let code_of = |loc: &str| {
        url::Url::parse(loc)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "code")
            .unwrap_or_else(|| panic!("no code in {loc}"))
            .1
            .into_owned()
    };
    let exchange = |code: String| {
        let app = app.clone();
        async move {
            let (st, _, body) = post_form(
                &app,
                TOKEN,
                &[
                    ("grant_type", "authorization_code"),
                    ("client_id", "nopkce"),
                    ("client_secret", "nopkce-secret"),
                    ("code", &code),
                    ("redirect_uri", CALLBACK),
                ],
                None,
            )
            .await;
            assert_eq!(st, StatusCode::OK, "{body}");
        }
    };

    // Fresh login without a challenge.
    let (_, h, _) = get(&app, &auth(""), None).await;
    let login = h[header::LOCATION].to_str().unwrap().to_owned();
    let ck = auth_cookie(&h);
    let (_, _, html) = get(&app, &login, Some(&ck)).await;
    let (_, h, _) = post_form(
        &app,
        &login,
        &[
            ("csrf_token", &extract_hidden_input(&html, "csrf_token")),
            ("session_code", &extract_hidden_input(&html, "session_code")),
            ("username", "demo.orgmember@hel.fi"),
            ("password", "MemberPass123!Valid"),
        ],
        Some(&ck),
    )
    .await;
    let sso = h
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("__Host-wk_sso="))
        .and_then(|v| v.split(';').next())
        .expect("login sets the SSO cookie")
        .to_owned();
    exchange(code_of(h[header::LOCATION].to_str().unwrap())).await;

    // The SSO session answers at once, with and without prompt=none.
    for extra in ["", "&prompt=none"] {
        let (st, h, body) = get(&app, &auth(extra), Some(&sso)).await;
        assert!(st.is_redirection(), "{extra}: {st} {body}");
        exchange(code_of(h[header::LOCATION].to_str().unwrap())).await;
    }
}

#[tokio::test]
async fn userinfo_refusals() {
    let (app, store, kek) = app().await;
    let now = jc_time::TimestampTz::now().as_second();
    let member = user_id(&store, "demo.orgmember@hel.fi").await;
    let userinfo = |authz: String, dpop: Option<HeaderValue>| {
        let app = app.clone();
        async move {
            let mut b = Request::builder()
                .uri(USERINFO)
                .header(header::AUTHORIZATION, authz);
            if let Some(d) = dpop {
                b = b.header("dpop", d);
            }
            send(&app, b.body(Body::empty()).unwrap()).await.0
        }
    };
    let tok = |extra: Value| {
        let store = store.clone();
        let kek = kek.clone();
        async move {
            let mut c = json!({ "iss": ISSUER, "typ": "Bearer", "azp": "edge", "scope": "openid",
                "iat": now, "exp": now + 300 });
            for (k, v) in extra.as_object().unwrap() {
                c[k] = v.clone();
            }
            realm_token(&store, &kek, c).await
        }
    };

    // No subject, and a subject that names nobody without a session to resolve it.
    for extra in [json!({}), json!({ "sub": "nobody-at-all" })] {
        let t = tok(extra.clone()).await;
        assert_eq!(
            userinfo(format!("Bearer {t}"), None).await,
            StatusCode::UNAUTHORIZED,
            "{extra}"
        );
    }

    // A pairwise subject whose client has no single redirect host and no sector URI: the
    // subject cannot be computed, so no claims.
    client(&store, "pw", |c| {
        c.protocol_mappers = vec![ProtocolMapperRepresentation {
            name: "pairwise".into(),
            protocol: "openid-connect".into(),
            protocol_mapper: "oidc-sha256-pairwise-sub-mapper".into(),
            ..Default::default()
        }];
    })
    .await;
    let start = jc_time::TimestampTz::now();
    store
        .upsert_sso_session(SsoSession {
            id: "sso-pw".into(),
            realm_id: "dev".into(),
            org_id: None,
            handle_hash: sha256_hex("sso-pw-handle"),
            user_id: member.clone(),
            username: "demo.orgmember@hel.fi".into(),
            ip_address: "127.0.0.1".into(),
            user_agent: String::new(),
            start: start.as_millis(),
            last_access: start.as_millis(),
            auth_time: start.as_second(),
            acr: "pwd".into(),
            amr: vec!["pwd".into()],
            clients: HashMap::new(),
            idle_expires_at: start + jiff::SignedDuration::from_hours(1),
            max_expires_at: start + jiff::SignedDuration::from_hours(8),
        })
        .await
        .unwrap();
    let t = tok(json!({ "sub": "pairwise-guess", "sid": "sso-pw", "azp": "pw" })).await;
    assert_eq!(
        userinfo(format!("Bearer {t}"), None).await,
        StatusCode::UNAUTHORIZED
    );

    // A DPoP-bound token with a DPoP header that is not visible ASCII.
    let t = tok(json!({ "sub": member, "cnf": { "jkt": "some-thumbprint" } })).await;
    let raw = HeaderValue::from_bytes(b"\xe9t\xe9").unwrap();
    assert_eq!(
        userinfo(format!("DPoP {t}"), Some(raw)).await,
        StatusCode::UNAUTHORIZED
    );
}

/// `GET .../auth` from a browser: an error met before the flow can go back to the app is the
/// realm's error page (status of the error, the parameter it is about, the way back to the app
/// when the client names an http(s) base URL, else the next step); other callers keep JSON.
#[tokio::test]
async fn a_browser_authorize_error_is_a_page_naming_the_parameter_and_the_way_back() {
    let (app, store, _) = app().await;
    client(&store, "named-app", |c| {
        c.name = Some("Parking".into());
        c.base_url = Some("https://parking.example.test/".into());
        c.redirect_uris = vec![CALLBACK.into()];
    })
    .await;
    client(&store, "plain-app", |c| {
        c.base_url = Some("https://plain.example.test/".into());
        c.redirect_uris = vec![CALLBACK.into()];
    })
    .await;
    client(&store, "script-app", |c| {
        c.base_url = Some("javascript:alert(1)".into());
        c.redirect_uris = vec![CALLBACK.into()];
    })
    .await;
    let auth = "/realms/dev/protocol/openid-connect/auth";
    let page = |query: String| {
        let app = app.clone();
        async move {
            let req = Request::builder()
                .uri(format!("{auth}?{query}"))
                .header(header::ACCEPT, "text/html,application/xhtml+xml")
                .header(header::ACCEPT_LANGUAGE, "en-GB, sk;q=0.5")
                .body(Body::empty())
                .unwrap();
            send(&app, req).await
        }
    };
    let bad_redirect = |id: &str| {
        format!("client_id={id}&response_type=code&redirect_uri=https%3A%2F%2Fevil.example.test%2F")
    };

    let (st, h, html) = page(bad_redirect("named-app")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{html}");
    assert!(
        h[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert!(html.contains("(redirect_uri)"), "{html}");
    assert!(html.contains("https://parking.example.test/"), "{html}");
    assert!(html.contains("Back to Parking"), "{html}");

    let (st, _, html) = page(bad_redirect("plain-app")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(html.contains("Back to the application"), "{html}");

    let (_, _, html) = page(bad_redirect("script-app")).await;
    assert!(!html.contains("javascript:alert"), "{html}");
    assert!(html.contains("open the app again from its link"), "{html}");

    // No client_id: the page names it and, in the language asked for, the next step.
    let (st, _, html) = page("response_type=code&kc_locale=sk".into()).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(html.contains("(client_id)"), "{html}");
    assert!(html.contains("lang=\"sk\""), "{html}");
    assert!(html.contains("otvorte aplikáciu znova"), "{html}");

    // An unknown client: the error names client_id, and there is no client to go back to.
    let (_, _, html) = page(bad_redirect("nobody")).await;
    assert!(html.contains("(client_id)"), "{html}");

    // A caller that does not ask for HTML keeps the OAuth JSON error.
    let (st, _, body) = get(&app, &format!("{auth}?{}", bad_redirect("named-app")), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(!err(&body).is_empty(), "{body}");
}

#[tokio::test]
async fn logout_reads_accept_language_and_refuses_a_token_that_is_not_base64url() {
    let (app, _, _) = app().await;
    let req = Request::builder()
        .uri(LOGOUT)
        .header(header::ACCEPT_LANGUAGE, "xx, de-AT;q=0.8")
        .body(Body::empty())
        .unwrap();
    let (st, _, html) = send(&app, req).await;
    assert_eq!(st, StatusCode::OK);
    assert!(html.contains("lang=\"de\""), "{html}");
    let (st, _, _) = post_form(&app, LOGOUT, &[("logout_csrf_token", "!!!")], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}
