#![forbid(unsafe_code)]
//! Token, introspection and revocation endpoint refusals (W1, W12): every way client
//! authentication fails, each grant's error answers, and what introspection and revocation say
//! about live, revoked and unknown tokens.

use axum::Router;
use axum::http::StatusCode;
use base64::engine::{Engine, general_purpose::STANDARD, general_purpose::URL_SAFE_NO_PAD};
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

mod common;
use common::*;

const ISSUER: &str = "https://idm.dev.example.test/realms/dev";
const TOKEN: &str = "/realms/dev/protocol/openid-connect/token";
const INTROSPECT: &str = "/realms/dev/protocol/openid-connect/token/introspect";
const REVOKE: &str = "/realms/dev/protocol/openid-connect/revoke";
const JWT_TYPE: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

async fn app() -> (Router, Arc<MemoryStore>, kaisatsu_server::KaisatsuState) {
    let (state, store, _) = setup_dev_realm_state().await;
    (router(state.clone(), mount()), store, state)
}

fn err(body: &str) -> String {
    serde_json::from_str::<Value>(body).unwrap_or(Value::Null)["error"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

async fn post_basic(
    app: &Router,
    uri: &str,
    basic: &str,
    form: &[(&str, &str)],
) -> (StatusCode, String) {
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();
    let req = axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", format!("Basic {basic}"))
        .body(axum::body::Body::from(encoded))
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
    let st = resp.status();
    let bytes = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    (st, String::from_utf8_lossy(&bytes).into_owned())
}

async fn add_client(
    store: &MemoryStore,
    client_id: &str,
    authenticator: &str,
    attrs: &[(&str, String)],
) {
    store
        .upsert_client(ClientRepresentation {
            id: format!("uuid-{client_id}"),
            realm_id: "dev".into(),
            client_id: client_id.into(),
            enabled: true,
            public_client: false,
            service_accounts_enabled: true,
            client_authenticator_type: authenticator.into(),
            secret: Some(format!("{client_id}-secret")),
            attributes: attrs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
            ..Default::default()
        })
        .await
        .unwrap();
}

fn fake_jwt(payload: &str) -> String {
    format!(
        "eyJhbGciOiJFUzI1NiJ9.{}.sig",
        URL_SAFE_NO_PAD.encode(payload)
    )
}

#[tokio::test]
async fn client_authentication_refusals() {
    let (app, store, _) = app().await;
    let cc = [("grant_type", "client_credentials")];
    let basic = |s: &str| STANDARD.encode(s);
    // Basic header shapes.
    for (b, why) in [
        ("***".to_owned(), "not base64"),
        (STANDARD.encode([0xff, 0xfe, b':', b'x']), "not UTF-8"),
        (basic("no-colon"), "no pair"),
        (basic("apisix-gateway:%zz"), "broken escape"),
        (basic("%zz:secret"), "broken escape in id"),
    ] {
        let (st, body) = post_basic(&app, TOKEN, &b, &cc).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{why}: {body}");
        assert_eq!(err(&body), "invalid_client");
    }
    let (st, _) = post_basic(
        &app,
        TOKEN,
        &basic("apisix-gateway:apisix-secret-123"),
        &[("grant_type", "client_credentials"), ("client_id", "edge")],
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "body and header name different clients"
    );
    let (st, body) = post_basic(&app, TOKEN, &basic("apisix-gateway:apisix-secret-123"), &cc).await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // Form credentials.
    for form in [
        vec![("grant_type", "client_credentials")],
        vec![
            ("grant_type", "client_credentials"),
            ("client_id", "apisix-gateway"),
        ],
        vec![
            ("grant_type", "client_credentials"),
            ("client_id", "apisix-gateway"),
            ("client_secret", ""),
        ],
        vec![
            ("grant_type", "client_credentials"),
            ("client_id", "portal-api"),
            ("client_secret", "x"),
        ],
        vec![
            ("grant_type", "client_credentials"),
            ("client_id", "no-such"),
            ("client_secret", "x"),
        ],
        vec![
            ("grant_type", "client_credentials"),
            ("client_assertion", "x.y.z"),
            ("client_assertion_type", "urn:other"),
        ],
    ] {
        let (st, _, body) = post_form(&app, TOKEN, &form, None).await;
        assert!(
            st == StatusCode::UNAUTHORIZED || st == StatusCode::BAD_REQUEST,
            "{form:?}: {st} {body}"
        );
        assert!(
            err(&body) == "invalid_client" || err(&body) == "unauthorized_client",
            "{form:?}: {body}"
        );
    }
    // A confidential client with an empty stored secret never matches.
    let mut gw = store
        .get_client_by_client_id("dev", "apisix-gateway")
        .await
        .unwrap()
        .unwrap();
    gw.secret = Some(String::new());
    store.upsert_client(gw).await.unwrap();
    let (st, _, _) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "client_credentials"),
            ("client_id", "apisix-gateway"),
            ("client_secret", "apisix-secret-123"),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Malformed assertions.
    for a in [
        "no-dots".to_owned(),
        "a.***.c".to_owned(),
        fake_jwt("not json"),
        fake_jwt(r#"{"iss":"x"}"#),
        fake_jwt(r#"{"sub":"no-such-client"}"#),
    ] {
        let (st, _, body) = post_form(
            &app,
            TOKEN,
            &[
                ("grant_type", "client_credentials"),
                ("client_assertion", &a),
                ("client_assertion_type", JWT_TYPE),
            ],
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{a}: {body}");
    }
}

#[tokio::test]
async fn private_key_jwt_refusals_and_replay() {
    let (app, store, _) = app().await;
    let kek = Kek::generate_for_tests();
    let key = KeyManager::generate_es256("pk", &kek, 100, "ACTIVE").unwrap();
    let jwk: Value = serde_json::from_str(&key.public_jwk_json).unwrap();
    add_client(
        &store,
        "pk-app",
        "client-jwt",
        &[("jwks.string", json!({"keys":[jwk]}).to_string())],
    )
    .await;
    add_client(&store, "pk-nojwks", "client-jwt", &[]).await;
    add_client(
        &store,
        "pk-badjwks",
        "client-jwt",
        &[("jwks.string", "{".into())],
    )
    .await;
    add_client(
        &store,
        "pk-nokeys",
        "client-jwt",
        &[("jwks.string", "{}".into())],
    )
    .await;
    let now = chrono::Utc::now().timestamp();
    let assertion = |sub: &str, aud: Value, jti: &str| {
        sign_jwt(
            &key,
            &kek,
            None,
            &json!({"iss": sub, "sub": sub, "aud": aud, "jti": jti, "iat": now, "exp": now + 60}),
        )
        .unwrap()
    };
    let call = |a: String, cid: Option<&'static str>| {
        let app = app.clone();
        async move {
            let mut f = vec![
                ("grant_type", "client_credentials".to_owned()),
                ("client_assertion", a),
                ("client_assertion_type", JWT_TYPE.to_owned()),
            ];
            if let Some(c) = cid {
                f.push(("client_id", c.to_owned()));
            }
            let pairs: Vec<(&str, &str)> = f.iter().map(|(k, v)| (*k, v.as_str())).collect();
            post_form(&app, TOKEN, &pairs, None).await
        }
    };
    // A password-authenticated client is not reached by an assertion naming another client_id.
    let (st, _, _) = call(assertion("pk-app", json!(ISSUER), "a1"), Some("edge")).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "sub must equal client_id");
    for cid in ["pk-nojwks", "pk-badjwks", "pk-nokeys"] {
        let (st, _, body) = call(assertion(cid, json!(ISSUER), &format!("j-{cid}")), None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{cid}: {body}");
    }
    let (st, _, _) = call(assertion("pk-app", json!("https://elsewhere"), "a2"), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "a foreign audience");
    let (st, _, _) = call(
        assertion(
            "pk-app",
            json!([
                "https://x",
                format!("{ISSUER}/protocol/openid-connect/token")
            ]),
            "a3",
        ),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "the token endpoint in an audience list");
    let (st, _, body) = call(assertion("pk-app", json!(ISSUER), "a3"), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "a replayed jti: {body}");
    let (st, _, _) = call(assertion("pk-app", json!(17), "a4"), None).await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "an audience of the wrong type"
    );
    // A client that must use an assertion cannot use its secret.
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "client_credentials"),
            ("client_id", "pk-app"),
            ("client_secret", "pk-app-secret"),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
}

#[tokio::test]
async fn federated_jwt_refusals() {
    let (app, store, state) = app().await;
    let kek = Kek::generate_for_tests();
    let cluster = KeyManager::generate_es256("cluster", &kek, 100, "ACTIVE").unwrap();
    let jwk: Value = serde_json::from_str(&cluster.public_jwk_json).unwrap();
    let k8s_iss = "https://kubernetes.default.svc.cluster.local";
    let now = chrono::Utc::now().timestamp();
    let sa = |sub: &str| {
        sign_jwt(
            &cluster,
            &kek,
            None,
            &json!({"iss": k8s_iss, "sub": sub, "aud": ISSUER, "iat": now, "exp": now + 600}),
        )
        .unwrap()
    };
    let call = |a: String, cid: Option<&'static str>| {
        let app = app.clone();
        async move {
            let mut f = vec![
                ("grant_type", "client_credentials".to_owned()),
                ("client_assertion", a),
                ("client_assertion_type", JWT_TYPE.to_owned()),
            ];
            if let Some(c) = cid {
                f.push(("client_id", c.to_owned()));
            }
            let pairs: Vec<(&str, &str)> = f.iter().map(|(k, v)| (*k, v.as_str())).collect();
            post_form(&app, TOKEN, &pairs, None).await
        }
    };
    let idp = store.get_idp("dev", "kubernetes").await.unwrap().unwrap();
    let jwks_url = idp
        .config
        .get("jwksUrl")
        .cloned()
        .unwrap_or_else(|| format!("{k8s_iss}/openid/v1/jwks"));
    let portal = store
        .get_client_by_client_id("dev", "portal-api")
        .await
        .unwrap()
        .unwrap();
    let sub = portal.attributes["jwt.credential.sub"].clone();

    // The cluster JWKS has no keys, then the right key.
    state
        .guard()
        .insert_mock(&jwks_url, 200, br#"{"nokeys":[]}"#.to_vec());
    let (st, _, body) = call(sa(&sub), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
    state.guard().insert_mock(
        &jwks_url,
        200,
        json!({"keys":[jwk]}).to_string().into_bytes(),
    );
    let (st, _, body) = call(sa(&sub), None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (st, _, _) = call(sa(&sub), Some("edge")).await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "client_id must match the federated client"
    );
    // Another issuer's signature fails.
    let other = KeyManager::generate_es256("other", &kek, 100, "ACTIVE").unwrap();
    let forged = sign_jwt(
        &other,
        &kek,
        None,
        &json!({"iss": k8s_iss, "sub": sub, "aud": ISSUER, "iat": now, "exp": now + 600}),
    )
    .unwrap();
    let (st, _, _) = call(forged, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // A federated client whose issuer IdP is missing.
    add_client(
        &store,
        "fed-orphan",
        "federated-jwt",
        &[
            (
                "jwt.credential.sub",
                "system:serviceaccount:x:orphan".into(),
            ),
            ("jwt.credential.issuer", "no-such-idp".into()),
        ],
    )
    .await;
    let (st, _, body) = call(sa("system:serviceaccount:x:orphan"), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
}

async fn browser_code(app: &Router) -> String {
    let verifier = "pkce-verifier-string-with-at-least-43-characters-long-012345";
    let uri = format!(
        "/realms/dev/protocol/openid-connect/auth?client_id=edge&redirect_uri={}&response_type=code&scope=openid&code_challenge={}&code_challenge_method=S256",
        url::form_urlencoded::byte_serialize(b"https://portal.dev.example.test/callback")
            .collect::<String>(),
        sha256_base64url(verifier),
    );
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri(&uri)
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    let login = resp.headers()["location"].to_str().unwrap().to_owned();
    let resp = tower::ServiceExt::oneshot(
        app.clone(),
        axum::http::Request::builder()
            .uri(&login)
            .body(axum::body::Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    let html = String::from_utf8(
        http_body_util::BodyExt::collect(resp.into_body())
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let (_, h, _) = post_form(
        app,
        &login,
        &[
            ("csrf_token", &extract_hidden_input(&html, "csrf_token")),
            ("session_code", &extract_hidden_input(&html, "session_code")),
            ("username", "demo.orgmember@hel.fi"),
            ("password", "MemberPass123!Valid"),
        ],
        None,
    )
    .await;
    let loc = h["location"].to_str().unwrap();
    url::Url::parse(loc)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned()
}

#[tokio::test]
async fn authorization_code_grant_refusals() {
    let (app, store, _) = app().await;
    let verifier = "pkce-verifier-string-with-at-least-43-characters-long-012345";
    let cb = "https://portal.dev.example.test/callback";
    let base = |code: &str| {
        vec![
            ("grant_type", "authorization_code".to_owned()),
            ("client_id", "edge".to_owned()),
            ("client_secret", "edge-secret-123".to_owned()),
            ("code", code.to_owned()),
            ("redirect_uri", cb.to_owned()),
            ("code_verifier", verifier.to_owned()),
        ]
    };
    let send = |form: Vec<(&'static str, String)>| {
        let app = app.clone();
        async move {
            let pairs: Vec<(&str, &str)> = form.iter().map(|(k, v)| (*k, v.as_str())).collect();
            post_form(&app, TOKEN, &pairs, None).await
        }
    };
    let (st, _, body) = send(vec![
        ("grant_type", "authorization_code".into()),
        ("client_id", "edge".into()),
        ("client_secret", "edge-secret-123".into()),
    ])
    .await;
    assert_eq!(
        (st, err(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
    let code = browser_code(&app).await;
    let mut no_redirect = base(&code);
    no_redirect.retain(|(k, _)| *k != "redirect_uri");
    let (_, _, body) = send(no_redirect).await;
    assert_eq!(err(&body), "invalid_request");
    let (_, _, body) = send(base("unknown-code")).await;
    assert_eq!(err(&body), "invalid_grant");
    for (k, v) in [
        ("redirect_uri", "https://portal.dev.example.test/other"),
        ("code_verifier", "short"),
        (
            "code_verifier",
            "pkce-verifier-string-with-at-least-43-characters-long-WRONG!",
        ),
    ] {
        let code = browser_code(&app).await;
        let mut f = base(&code);
        for e in f.iter_mut() {
            if e.0 == k {
                e.1 = v.to_owned();
            }
        }
        let (st, _, body) = send(f).await;
        assert_eq!(
            (st, err(&body).as_str()),
            (StatusCode::BAD_REQUEST, "invalid_grant"),
            "{k}={v}"
        );
    }
    let code = browser_code(&app).await;
    let mut f = base(&code);
    f.retain(|(k, _)| *k != "code_verifier");
    let (_, _, body) = send(f).await;
    assert_eq!(err(&body), "invalid_grant", "a missing verifier");
    // Another client cannot redeem edge's code.
    let code = browser_code(&app).await;
    let mut f = base(&code);
    f[1].1 = "grafana".into();
    f[2].1 = "grafana-secret-123".into();
    let (_, _, body) = send(f).await;
    assert_eq!(err(&body), "invalid_grant");
    // A disabled person gets nothing for a fresh code.
    let code = browser_code(&app).await;
    let mut u = store
        .get_user_by_username_or_email("dev", None, "demo.orgmember@hel.fi")
        .await
        .unwrap()
        .unwrap();
    u.enabled = false;
    store.update_user(u).await.unwrap();
    let (_, _, body) = send(base(&code)).await;
    assert_eq!(err(&body), "invalid_grant");
    let (st, _, body) = send(vec![
        ("grant_type", "urn:nope".into()),
        ("client_id", "edge".into()),
        ("client_secret", "edge-secret-123".into()),
    ])
    .await;
    assert_eq!(
        (st, err(&body).as_str()),
        (StatusCode::BAD_REQUEST, "unsupported_grant_type")
    );
}

#[tokio::test]
async fn client_credentials_refusals_and_introspection_and_revocation() {
    let (app, store, _) = app().await;
    let gw = [
        ("client_id", "apisix-gateway"),
        ("client_secret", "apisix-secret-123"),
    ];
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "client_credentials"),
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
        ],
        None,
    )
    .await;
    assert_eq!(
        (st, err(&body).as_str()),
        (StatusCode::BAD_REQUEST, "unauthorized_client")
    );
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[("grant_type", "client_credentials"), gw[0], gw[1]],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let access = serde_json::from_str::<Value>(&body).unwrap()["access_token"]
        .as_str()
        .unwrap()
        .to_owned();

    // Introspection: a live token, an unknown one, without client authentication.
    let (st, _, body) =
        post_form(&app, INTROSPECT, &[("token", &access), gw[0], gw[1]], None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["active"],
        true
    );
    for t in ["garbage", ""] {
        let (st, _, body) = post_form(&app, INTROSPECT, &[("token", t), gw[0], gw[1]], None).await;
        assert!(
            st == StatusCode::OK || st == StatusCode::BAD_REQUEST,
            "{t}: {st}"
        );
        if st == StatusCode::OK {
            assert_eq!(
                serde_json::from_str::<Value>(&body).unwrap()["active"],
                false
            );
        }
    }
    let (st, _, _) = post_form(&app, INTROSPECT, &[("token", &access)], None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // Revocation: unauthenticated refused, unknown accepted (RFC 7009), then the token is dead.
    let (st, _, _) = post_form(&app, REVOKE, &[("token", &access)], None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _, _) = post_form(&app, REVOKE, &[("token", "unknown"), gw[0], gw[1]], None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = post_form(
        &app,
        REVOKE,
        &[
            ("token", &access),
            ("token_type_hint", "access_token"),
            gw[0],
            gw[1],
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, _, body) = post_form(&app, INTROSPECT, &[("token", &access), gw[0], gw[1]], None).await;
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["active"],
        false
    );

    // The service account disabled, or its name held by a person.
    let sa = store
        .get_user_by_username_or_email("dev", None, "service-account-apisix-gateway")
        .await
        .unwrap()
        .unwrap();
    let mut off = sa.clone();
    off.enabled = false;
    store.update_user(off).await.unwrap();
    let (_, _, body) = post_form(
        &app,
        TOKEN,
        &[("grant_type", "client_credentials"), gw[0], gw[1]],
        None,
    )
    .await;
    assert_eq!(err(&body), "unauthorized_client");
    let mut held = sa;
    held.enabled = true;
    held.service_account_client_id = None;
    store.update_user(held).await.unwrap();
    let (_, _, body) = post_form(
        &app,
        TOKEN,
        &[("grant_type", "client_credentials"), gw[0], gw[1]],
        None,
    )
    .await;
    assert_eq!(err(&body), "unauthorized_client");
}
