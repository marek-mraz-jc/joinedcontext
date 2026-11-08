#![forbid(unsafe_code)]
//! Refusals of the upstream OIDC broker routes (K12): every request that cannot continue a
//! broker login (unknown realm, session or IdP, a missing or foreign cookie, a broken or foreign
//! broker state, an IdP missing configuration, a failing or lying upstream) answers an error and
//! signs nobody in; the link confirmation POST refuses a bad CSRF token and a broken pending
//! state. Also the linked-account and attribute-linked sign-in paths and HTML escaping of the
//! account name on the link confirmation page.

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
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tower::ServiceExt;

mod common;
use common::*;

const CLIENT: &str = "kaisatsu-broker-client";

/// A local upstream whose token answer and JWKS each test can replace.
#[derive(Clone)]
struct Upstream {
    issuer: String,
    key: SigningKeyRecord,
    kek: Kek,
    claims: Arc<Mutex<Value>>,
    token_override: Arc<Mutex<Option<(u16, String)>>>,
    jwks_override: Arc<Mutex<Option<Value>>>,
}

impl Upstream {
    fn claims(&self, sub: &str, email: Option<&str>, verified: bool) {
        let now = chrono::Utc::now().timestamp();
        let mut c = json!({ "iss": self.issuer, "sub": sub, "aud": CLIENT, "nonce": "n-1",
            "email_verified": verified, "iat": now, "exp": now + 300 });
        if let Some(e) = email {
            c["email"] = json!(e);
        }
        *self.claims.lock().unwrap() = c;
    }
    fn token(&self, answer: Option<(u16, &str)>) {
        *self.token_override.lock().unwrap() = answer.map(|(s, b)| (s, b.to_owned()));
    }
    fn jwks(&self, jwks: Option<Value>) {
        *self.jwks_override.lock().unwrap() = jwks;
    }
}

async fn start_upstream() -> Upstream {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let kek = Kek::generate_for_tests();
    let up = Upstream {
        issuer: format!("http://127.0.0.1:{port}"),
        key: KeyManager::generate_es256("upstream", &kek, 100, "ACTIVE").unwrap(),
        kek,
        claims: Arc::new(Mutex::new(json!({}))),
        token_override: Arc::new(Mutex::new(None)),
        jwks_override: Arc::new(Mutex::new(None)),
    };
    let app = Router::new()
        .route(
            "/jwks",
            get(|State(s): State<Upstream>| async move {
                if let Some(j) = s.jwks_override.lock().unwrap().clone() {
                    return Json(j);
                }
                let jwk: Value = serde_json::from_str(&s.key.public_jwk_json).unwrap();
                Json(json!({ "keys": [jwk] }))
            }),
        )
        .route(
            "/token",
            post(
                |State(s): State<Upstream>, Form(_f): Form<BTreeMap<String, String>>| async move {
                    if let Some((status, body)) = s.token_override.lock().unwrap().clone() {
                        return (StatusCode::from_u16(status).unwrap(), body);
                    }
                    let claims = s.claims.lock().unwrap().clone();
                    let id_token = sign_jwt(&s.key, &s.kek, Some("JWT"), &claims).unwrap();
                    (
                        StatusCode::OK,
                        json!({ "access_token": "up-at", "token_type": "Bearer", "id_token": id_token })
                            .to_string(),
                    )
                },
            ),
        )
        .with_state(up.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    up
}

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

fn idp(up: &Upstream, alias: &str, enabled: bool, drop: &[&str]) -> IdentityProviderRepresentation {
    let mut config = BTreeMap::from([
        ("issuer".to_owned(), up.issuer.clone()),
        (
            "authorizationUrl".to_owned(),
            format!("{}/authorize", up.issuer),
        ),
        ("tokenUrl".to_owned(), format!("{}/token", up.issuer)),
        ("jwksUrl".to_owned(), format!("{}/jwks", up.issuer)),
        ("clientId".to_owned(), CLIENT.to_owned()),
        ("clientSecret".to_owned(), "broker-pass-value".to_owned()),
    ]);
    for k in drop {
        config.remove(*k);
    }
    IdentityProviderRepresentation {
        internal_id: format!("idp-{alias}"),
        realm_id: "dev".into(),
        org_id: None,
        alias: alias.into(),
        provider_id: "oidc".into(),
        enabled,
        trust_email: true,
        first_broker_login_flow_alias: "first broker login".into(),
        config,
        managed_by: None,
    }
}

async fn setup() -> (Router, Arc<MemoryStore>, Upstream) {
    let up = start_upstream().await;
    let (_, store, kek) = setup_dev_realm_state().await;
    for (alias, enabled, drop) in [
        ("entra", true, &[][..]),
        ("off", false, &[][..]),
        ("notoken", true, &["tokenUrl"][..]),
        ("noclient", true, &["clientId"][..]),
        ("nojwks", true, &["jwksUrl"][..]),
    ] {
        store
            .upsert_idp(idp(&up, alias, enabled, drop))
            .await
            .unwrap();
    }
    let guard = OutboundGuard::new(OutboundPolicy {
        in_cluster_hosts: vec!["127.0.0.1".into()],
        ..OutboundPolicy::default()
    });
    let state = KaisatsuState::new(
        store.clone() as Arc<dyn Store>,
        kek,
        guard,
        Arc::new(LocalAuthorizer),
    );
    (router(state, mount()).layer(Extension(mount())), store, up)
}

/// An authorization request that reached the login page: (session code, auth cookie).
async fn start_session(app: &Router) -> (String, String) {
    let uri = format!(
        "/realms/dev/protocol/openid-connect/auth?client_id=edge&redirect_uri={}&response_type=code&scope=openid&code_challenge={}&code_challenge_method=S256",
        url::form_urlencoded::byte_serialize(b"https://portal.dev.example.test/callback")
            .collect::<String>(),
        sha256_base64url("pkce-verifier-string-with-at-least-43-characters-long-012345"),
    );
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = auth_cookie(resp.headers());
    let loc = resp.headers()[header::LOCATION].to_str().unwrap();
    let parsed = url::Url::parse(&format!("https://idm.dev.example.test{loc}")).unwrap();
    let code = parsed
        .query_pairs()
        .find(|(k, _)| k == "session_code")
        .unwrap()
        .1
        .into_owned();
    (code, cookie)
}

/// Puts a pending broker login for `alias` on the session; returns the broker `state`.
async fn pend(store: &MemoryStore, session: &str, pending: Value) {
    let mut s = store
        .get_auth_session_by_id("dev", session)
        .await
        .unwrap()
        .unwrap();
    s.broker_state_json = Some(if pending.is_string() {
        pending.as_str().unwrap().to_owned()
    } else {
        pending.to_string()
    });
    store.put_auth_session(s).await.unwrap();
}

fn pending(session: &str, alias: &str) -> (Value, String) {
    let state = format!("{session}.s-1");
    (
        json!({ "idp_alias": alias, "state": state, "nonce": "n-1", "pkce_verifier": "v-1" }),
        state,
    )
}

async fn get_ck(app: &Router, uri: &str, cookie: Option<&str>) -> (StatusCode, HeaderMap, String) {
    let mut b = Request::builder().uri(uri);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    let h = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, h, String::from_utf8_lossy(&bytes).into_owned())
}

/// Runs the callback for `alias` with a fresh session pending on it.
async fn callback(
    app: &Router,
    store: &MemoryStore,
    alias: &str,
) -> (StatusCode, HeaderMap, String) {
    let (s, ck) = start_session(app).await;
    let (p, state) = pending(&s, alias);
    pend(store, &s, p).await;
    get_ck(
        app,
        &format!("/realms/dev/broker/{alias}/endpoint?code=c-1&state={state}"),
        Some(&ck),
    )
    .await
}

#[tokio::test]
async fn broker_login_refusals() {
    let (app, _, _) = setup().await;
    let (s, ck) = start_session(&app).await;
    let (_, other_ck) = start_session(&app).await;
    for (uri, cookie, want) in [
        (
            format!("/realms/nope/broker/entra/login?session_code={s}"),
            Some(ck.as_str()),
            StatusCode::NOT_FOUND,
        ),
        (
            "/realms/dev/broker/entra/login?session_code=unknown".to_owned(),
            Some(ck.as_str()),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/realms/dev/broker/entra/login?session_code={s}"),
            None,
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/realms/dev/broker/entra/login?session_code={s}"),
            Some(other_ck.as_str()),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/realms/dev/broker/off/login?session_code={s}"),
            Some(ck.as_str()),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/realms/dev/broker/ghost/login?session_code={s}"),
            Some(ck.as_str()),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (st, h, _) = get_ck(&app, &uri, cookie).await;
        assert_eq!(st, want, "{uri}");
        assert!(h.get(header::SET_COOKIE).is_none(), "{uri}: no session");
    }
}

#[tokio::test]
async fn broker_callback_refuses_broken_state() {
    let (app, store, up) = setup().await;
    up.claims("ext-1", Some("x@hel.fi"), true);
    let (s, ck) = start_session(&app).await;
    let (p, state) = pending(&s, "entra");
    let ep = |q: &str| format!("/realms/dev/broker/entra/endpoint?{q}");

    // Unknown realm, no state, a state without a session part, an unknown session.
    for (uri, want) in [
        (
            format!("/realms/nope/broker/entra/endpoint?code=c&state={state}"),
            StatusCode::NOT_FOUND,
        ),
        (ep("code=c"), StatusCode::BAD_REQUEST),
        (ep("code=c&state="), StatusCode::BAD_REQUEST),
        (ep("code=c&state=nodot"), StatusCode::BAD_REQUEST),
        (ep("code=c&state=unknown.s-1"), StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(get_ck(&app, &uri, Some(&ck)).await.0, want, "{uri}");
    }
    // No broker login in flight on this session yet.
    assert_eq!(
        get_ck(&app, &ep(&format!("code=c&state={state}")), Some(&ck))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    // Garbage in the stored broker state.
    pend(&store, &s, json!("not json")).await;
    assert_eq!(
        get_ck(&app, &ep(&format!("code=c&state={state}")), Some(&ck))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    pend(&store, &s, p).await;
    // The callback of another IdP, and an upstream error without a code.
    let (st, _, _) = get_ck(
        &app,
        &format!("/realms/dev/broker/nojwks/endpoint?code=c&state={state}"),
        Some(&ck),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _, _) = get_ck(
        &app,
        &ep(&format!("error=access_denied&state={state}")),
        Some(&ck),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _, _) = get_ck(&app, &ep(&format!("code=&state={state}")), Some(&ck)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn broker_callback_refuses_idp_config_and_upstream_failures() {
    let (app, store, up) = setup().await;
    up.claims("ext-1", Some("new.user@hel.fi"), true);
    // A disabled IdP, and IdPs missing tokenUrl, clientId or jwksUrl.
    assert_eq!(callback(&app, &store, "off").await.0, StatusCode::NOT_FOUND);
    for alias in ["notoken", "noclient", "nojwks"] {
        assert_eq!(
            callback(&app, &store, alias).await.0,
            StatusCode::BAD_REQUEST,
            "{alias}"
        );
    }
    // The upstream token endpoint fails, answers no JSON, or answers without an ID token.
    for answer in [
        (500, "{}"),
        (200, "not json"),
        (200, r#"{"access_token":"a"}"#),
    ] {
        up.token(Some(answer));
        assert_eq!(
            callback(&app, &store, "entra").await.0,
            StatusCode::UNAUTHORIZED,
            "{answer:?}"
        );
    }
    up.token(None);
    // The JWKS holds no keys, or only a key that did not sign the ID token.
    up.jwks(Some(json!({ "nokeys": [] })));
    assert_eq!(
        callback(&app, &store, "entra").await.0,
        StatusCode::UNAUTHORIZED
    );
    let other_kek = Kek::generate_for_tests();
    let other = KeyManager::generate_es256("other", &other_kek, 100, "ACTIVE").unwrap();
    let other_jwk: Value = serde_json::from_str(&other.public_jwk_json).unwrap();
    up.jwks(Some(json!({ "keys": [other_jwk] })));
    assert_eq!(
        callback(&app, &store, "entra").await.0,
        StatusCode::UNAUTHORIZED
    );
    up.jwks(None);
    // A verified upstream identity without an e-mail cannot be linked or created.
    up.claims("ext-noemail", None, true);
    assert_eq!(
        callback(&app, &store, "entra").await.0,
        StatusCode::FORBIDDEN
    );
    assert!(
        store
            .get_idp_link_by_external("dev", "entra", "ext-noemail")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn broker_signs_in_linked_and_attribute_linked_people() {
    let (app, store, up) = setup().await;
    // A new person: created with the session's organization (the IdP names none) and linked.
    up.claims("ext-new", Some("fresh.person@hel.fi"), true);
    let (st, h, body) = callback(&app, &store, "entra").await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    assert!(h[header::LOCATION].to_str().unwrap().contains("code="));
    let link = store
        .get_idp_link_by_external("dev", "entra", "ext-new")
        .await
        .unwrap()
        .expect("link stored");
    // The same upstream subject again signs in through the link, whatever e-mail it now says.
    up.claims("ext-new", Some("renamed@hel.fi"), false);
    let (st, _, body) = callback(&app, &store, "entra").await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    // A disabled linked person is not signed in through the link.
    let mut person = store
        .get_user_by_id("dev", None, &link.user_id)
        .await
        .unwrap()
        .unwrap();
    person.enabled = false;
    store.update_user(person).await.unwrap();
    let (st, _, _) = callback(&app, &store, "entra").await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "unverified e-mail and no usable link"
    );

    // An account carrying the link attribute (no link row) signs in directly.
    store
        .create_user(UserRepresentation {
            id: "attr-linked".into(),
            realm_id: "dev".into(),
            username: "attr.linked@hel.fi".into(),
            email: Some("attr.linked@hel.fi".into()),
            enabled: true,
            attributes: HashMap::from([(
                "kaisatsu.idp.entra.sub".to_owned(),
                vec!["ext-attr".to_owned()],
            )]),
            ..Default::default()
        })
        .await
        .unwrap();
    up.claims("ext-attr", Some("attr.linked@hel.fi"), true);
    let (st, h, body) = callback(&app, &store, "entra").await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    assert!(h[header::LOCATION].to_str().unwrap().contains("code="));
}

#[tokio::test]
async fn broker_link_confirmation_refusals_and_escaping() {
    let (app, store, up) = setup().await;
    // An existing account whose name holds markup and which has no password.
    store
        .create_user(UserRepresentation {
            id: "markup-user".into(),
            realm_id: "dev".into(),
            username: "<i>link</i>".into(),
            email: Some("markup@hel.fi".into()),
            enabled: true,
            ..Default::default()
        })
        .await
        .unwrap();
    up.claims("ext-markup", Some("markup@hel.fi"), true);
    let (s, ck) = start_session(&app).await;
    let (p, state) = pending(&s, "entra");
    pend(&store, &s, p).await;
    let (st, _, html) = get_ck(
        &app,
        &format!("/realms/dev/broker/entra/endpoint?code=c&state={state}"),
        Some(&ck),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{html}");
    assert!(html.contains("&lt;i&gt;link&lt;/i&gt;"), "{html}");
    assert!(!html.contains("<i>link</i>"), "the account name is escaped");
    assert!(
        !html.contains(r#"name="password""#),
        "no password to re-enter"
    );
    let csrf = extract_hidden_input(&html, "csrf_token");
    let ep = "/realms/dev/broker/entra/endpoint";

    // Unknown realm, unknown session, a missing cookie, a wrong CSRF token.
    for (uri, pairs, cookie, want) in [
        (
            "/realms/nope/broker/entra/endpoint",
            vec![("csrf_token", csrf.as_str()), ("session_code", s.as_str())],
            Some(ck.as_str()),
            StatusCode::NOT_FOUND,
        ),
        (
            ep,
            vec![("csrf_token", csrf.as_str()), ("session_code", "unknown")],
            Some(ck.as_str()),
            StatusCode::BAD_REQUEST,
        ),
        (
            ep,
            vec![("csrf_token", csrf.as_str()), ("session_code", s.as_str())],
            None,
            StatusCode::BAD_REQUEST,
        ),
        (
            ep,
            vec![("csrf_token", "forged"), ("session_code", s.as_str())],
            Some(ck.as_str()),
            StatusCode::FORBIDDEN,
        ),
        (
            "/realms/dev/broker/nojwks/endpoint",
            vec![("csrf_token", csrf.as_str()), ("session_code", s.as_str())],
            Some(ck.as_str()),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (st, _, body) = post_form(&app, uri, &pairs, cookie).await;
        assert_eq!(st, want, "{uri} {body}");
    }
    let saved = store
        .get_auth_session_by_id("dev", &s)
        .await
        .unwrap()
        .unwrap()
        .broker_state_json
        .unwrap();
    let form = [("csrf_token", csrf.as_str()), ("session_code", s.as_str())];
    // Broken pending states: none, garbage, no claims, no account, a deleted account.
    let mut no_account: Value = serde_json::from_str(&saved).unwrap();
    no_account
        .as_object_mut()
        .unwrap()
        .remove("pending_existing_user_id");
    let mut gone: Value = serde_json::from_str(&saved).unwrap();
    gone["pending_existing_user_id"] = json!("deleted-account");
    for (pending_state, want) in [
        (None, StatusCode::BAD_REQUEST),
        (Some(json!("garbage")), StatusCode::BAD_REQUEST),
        (Some(pending(&s, "entra").0), StatusCode::BAD_REQUEST),
        (Some(no_account), StatusCode::BAD_REQUEST),
        (Some(gone), StatusCode::UNAUTHORIZED),
    ] {
        let mut sess = store
            .get_auth_session_by_id("dev", &s)
            .await
            .unwrap()
            .unwrap();
        sess.broker_state_json = pending_state.map(|v| match v {
            Value::String(t) => t,
            other => other.to_string(),
        });
        store.put_auth_session(sess).await.unwrap();
        let (st, _, body) = post_form(&app, ep, &form, Some(&ck)).await;
        assert_eq!(st, want, "{body}");
    }
    assert!(
        store
            .get_idp_link_by_external("dev", "entra", "ext-markup")
            .await
            .unwrap()
            .is_none(),
        "nothing linked by a refused confirmation"
    );
    // The saved pending state confirms without a password (the account has none).
    pend(&store, &s, json!(saved)).await;
    let (st, h, body) = post_form(&app, ep, &form, Some(&ck)).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    assert!(h[header::LOCATION].to_str().unwrap().contains("code="));
    assert!(
        store
            .get_idp_link_by_external("dev", "entra", "ext-markup")
            .await
            .unwrap()
            .is_some()
    );
}
