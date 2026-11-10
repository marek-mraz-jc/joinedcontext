#![forbid(unsafe_code)]
//! RFC 8628 device authorization grant (D1), as prod's Keycloak serves `jcctl`: discovery, the
//! device authorization endpoint, the verification page a signed-in person answers on, and the
//! token endpoint's polls. Every refusal: a client without the flag, another client's poll, an
//! expired, denied or reused code, a poll inside the interval, a page without a session or with a
//! foreign form, and the cap on wrong user codes.

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::{Extension, Router};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use kaisatsu_store_memory::MemoryStore;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use tower::ServiceExt;

mod common;
use common::*;

const DEVICE: &str = "/realms/dev/protocol/openid-connect/auth/device";
const TOKEN: &str = "/realms/dev/protocol/openid-connect/token";
const PAGE: &str = "/realms/dev/device";
const GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

async fn add_public(store: &MemoryStore, client_id: &str, device: bool) {
    let mut attributes =
        BTreeMap::from([("oauth2.device.polling.interval".to_owned(), "1".to_owned())]);
    if device {
        attributes.insert(
            "oauth2.device.authorization.grant.enabled".into(),
            "true".into(),
        );
    }
    store
        .upsert_client(ClientRepresentation {
            id: format!("uuid-{client_id}"),
            realm_id: "dev".into(),
            client_id: client_id.into(),
            name: Some(client_id.into()),
            enabled: true,
            public_client: true,
            attributes,
            ..Default::default()
        })
        .await
        .unwrap();
}

async fn app() -> (Router, Arc<MemoryStore>) {
    let (state, store, _) = setup_dev_realm_state().await;
    add_public(&store, "jcctl", true).await;
    add_public(&store, "jcctl-twin", true).await;
    add_public(&store, "no-device", false).await;
    (router(state, mount()).layer(Extension(mount())), store)
}

async fn get(app: &Router, uri: &str, cookie: Option<&str>) -> (StatusCode, HeaderMap, String) {
    let mut b = Request::builder().method(Method::GET).uri(uri);
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
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (st, h, String::from_utf8_lossy(&body).into_owned())
}

fn json(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn error(body: &str) -> String {
    json(body)["error"].as_str().unwrap_or_default().to_owned()
}

fn location(h: &HeaderMap) -> String {
    h[header::LOCATION].to_str().unwrap().to_owned()
}

fn set_cookie(h: &HeaderMap, prefix: &str) -> String {
    h.get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(prefix))
        .map(|v| v.split(';').next().unwrap().to_owned())
        .unwrap_or_else(|| panic!("no {prefix} cookie"))
}

/// A device authorization for `client`: (device code, user code).
async fn authorize(app: &Router, client: &str) -> (String, String) {
    let (st, _, body) = post_form(app, DEVICE, &[("client_id", client)], None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let v = json(&body);
    assert_eq!(
        v["verification_uri"],
        "https://idm.dev.example.test/realms/dev/device"
    );
    let user_code = v["user_code"].as_str().unwrap().to_owned();
    assert_eq!(
        v["verification_uri_complete"].as_str().unwrap(),
        format!("https://idm.dev.example.test/realms/dev/device?user_code={user_code}")
    );
    assert_eq!(v["interval"], 1);
    assert_eq!(v["expires_in"], 600);
    (v["device_code"].as_str().unwrap().to_owned(), user_code)
}

async fn poll(app: &Router, client: &str, device_code: &str) -> (StatusCode, String) {
    let (st, _, body) = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", GRANT),
            ("client_id", client),
            ("device_code", device_code),
        ],
        None,
    )
    .await;
    (st, body)
}

/// Signs `demo.orgmember` in through the verification page's own login; answers the SSO cookie.
async fn sign_in_at_page(app: &Router, user_code: &str) -> String {
    let (st, h, _) = get(app, &format!("{PAGE}?user_code={user_code}"), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "no session: the login first");
    let login = location(&h);
    assert!(login.contains("/login-actions/authenticate"), "{login}");
    let auth = set_cookie(&h, "__Host-wk_auth=");
    let (st, _, html) = get(app, &login, Some(&auth)).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    let (st, h, body) = post_form(
        app,
        &login,
        &[
            ("csrf_token", &extract_hidden_input(&html, "csrf_token")),
            ("session_code", &extract_hidden_input(&html, "session_code")),
            ("username", "demo.orgmember@hel.fi"),
            ("password", "MemberPass123!Valid"),
        ],
        Some(&auth),
    )
    .await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    let back = location(&h);
    assert!(
        back.starts_with(&format!(
            "https://idm.dev.example.test/realms/dev/device?user_code={user_code}"
        )),
        "the login comes back to the page: {back}"
    );
    set_cookie(&h, "__Host-wk_sso=")
}

/// The page's form for the signed-in person, the user code prefilled: its CSRF token.
async fn page_form(app: &Router, sso: &str, user_code: &str) -> String {
    let (st, _, html) = get(app, &format!("{PAGE}?user_code={user_code}"), Some(sso)).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    assert!(html.contains(&format!("value=\"{user_code}\"")), "{html}");
    extract_hidden_input(&html, "csrf")
}

async fn answer(
    app: &Router,
    sso: &str,
    csrf: &str,
    user_code: &str,
    decision: Option<&str>,
) -> (StatusCode, String) {
    let mut form = vec![("csrf", csrf), ("user_code", user_code)];
    if let Some(d) = decision {
        form.push(("decision", d));
    }
    let (st, _, body) = post_form(app, PAGE, &form, Some(sso)).await;
    (st, body)
}

#[tokio::test]
async fn discovery_names_the_device_endpoint_and_grant() {
    let (app, _) = app().await;
    let (st, _, body) = get(&app, "/realms/dev/.well-known/openid-configuration", None).await;
    assert_eq!(st, StatusCode::OK);
    let v = json(&body);
    assert_eq!(
        v["device_authorization_endpoint"],
        "https://idm.dev.example.test/realms/dev/protocol/openid-connect/auth/device"
    );
    assert!(
        v["grant_types_supported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g == GRANT)
    );
}

#[tokio::test]
async fn a_signed_in_person_allows_a_device_once() {
    let (app, store) = app().await;
    let (device_code, user_code) = authorize(&app, "jcctl").await;

    // Nobody answered yet; a poll inside the interval is slowed down.
    let (st, body) = poll(&app, "jcctl", &device_code).await;
    assert_eq!(
        (st, error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "authorization_pending")
    );
    let (st, body) = poll(&app, "jcctl", &device_code).await;
    assert_eq!(
        (st, error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "slow_down")
    );

    // Another client's poll of this code is no poll and costs the device nothing.
    let (st, body) = poll(&app, "jcctl-twin", &device_code).await;
    assert_eq!(
        (st, error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_grant")
    );

    let sso = sign_in_at_page(&app, &user_code).await;
    let csrf = page_form(&app, &sso, &user_code).await;
    let (st, html) = answer(&app, &sso, &csrf, &user_code, None).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    assert!(
        html.contains("Grant Access to jcctl")
            && html.contains("Do you grant these access privileges?"),
        "{html}"
    );
    let (st, html) = answer(&app, &sso, &csrf, &user_code, Some("allow")).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    assert!(html.contains("Device Login Successful"), "{html}");
    // A code answers once.
    let (st, _) = answer(&app, &sso, &csrf, &user_code, Some("deny")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "the code is no longer pending");

    // slow_down made the interval 6 s: the store's clock decides, so wait it out.
    tokio::time::sleep(std::time::Duration::from_millis(6100)).await;
    let (st, body) = poll(&app, "jcctl", &device_code).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let tokens = json(&body);
    let claims = decode_jwt_payload(tokens["access_token"].as_str().unwrap());
    let person = store
        .get_user_by_username_or_email("dev", None, "demo.orgmember@hel.fi")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claims["sub"], person.id.as_str());
    assert_eq!(claims["azp"], "jcctl");
    assert!(tokens["refresh_token"].as_str().is_some());

    // The tokens go once.
    tokio::time::sleep(std::time::Duration::from_millis(6100)).await;
    let (st, body) = poll(&app, "jcctl", &device_code).await;
    assert_eq!(
        (st, error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_grant"),
        "{body}"
    );
}

#[tokio::test]
async fn a_denied_device_gets_access_denied() {
    let (app, _) = app().await;
    let (device_code, user_code) = authorize(&app, "jcctl").await;
    let sso = sign_in_at_page(&app, &user_code).await;
    let csrf = page_form(&app, &sso, &user_code).await;
    let (st, html) = answer(&app, &sso, &csrf, &user_code, Some("deny")).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    assert!(
        html.contains("Consent denied for connecting the device."),
        "{html}"
    );
    let (st, body) = poll(&app, "jcctl", &device_code).await;
    assert_eq!(
        (st, error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "access_denied")
    );
}

#[tokio::test]
async fn only_a_client_the_realm_enables_starts_or_polls_a_device_grant() {
    let (app, store) = app().await;
    // A public client and a confidential one that proves itself: neither carries the flag.
    for (client, secret) in [("no-device", None), ("edge", Some("edge-secret-123"))] {
        let mut form = vec![("client_id", client)];
        if let Some(s) = secret {
            form.push(("client_secret", s));
        }
        let (st, _, body) = post_form(&app, DEVICE, &form, None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{client}: {body}");
        assert_eq!(error(&body), "unauthorized_client", "{client}: {body}");
    }
    // Unknown client: no device code either.
    let (st, _, _) = post_form(&app, DEVICE, &[("client_id", "nobody")], None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    // A client without the flag cannot redeem a code at the token endpoint.
    let raw = "a-device-code-of-no-device";
    store
        .put_device_code(record(raw, "BCDFGHJK", "no-device", 600))
        .await
        .unwrap();
    let (st, body) = poll(&app, "no-device", raw).await;
    assert_eq!(
        (st, error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "unauthorized_client")
    );
    // No device code at all.
    let (st, body) = poll(&app, "jcctl", "").await;
    assert_eq!(
        (st, error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
}

fn record(raw: &str, user_code: &str, client: &str, ttl: i64) -> DeviceCodeRecord {
    let now = Utc::now();
    DeviceCodeRecord {
        device_code_hash: sha256_hex(raw),
        user_code_hash: sha256_hex(user_code),
        realm_id: "dev".into(),
        client_id: client.into(),
        scope: "openid".into(),
        mapper_scopes: Vec::new(),
        status: DeviceStatus::Pending,
        user_id: None,
        sso_session_id: None,
        acr: String::new(),
        amr: Vec::new(),
        auth_time: 0,
        interval_secs: 1,
        last_poll_at: None,
        consumed: false,
        created_at: now - Duration::seconds(700),
        expires_at: now + Duration::seconds(ttl),
    }
}

#[tokio::test]
async fn an_expired_code_is_expired_at_the_token_endpoint_and_on_the_page() {
    let (app, store) = app().await;
    store
        .put_device_code(record("expired-device-code", "CDFGHJKL", "jcctl", -1))
        .await
        .unwrap();
    let (st, body) = poll(&app, "jcctl", "expired-device-code").await;
    assert_eq!(
        (st, error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "expired_token")
    );
    let (_, user_code) = authorize(&app, "jcctl").await;
    let sso = sign_in_at_page(&app, &user_code).await;
    let csrf = page_form(&app, &sso, &user_code).await;
    let (st, html) = answer(&app, &sso, &csrf, "CDFG-HJKL", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{html}");
    assert!(html.contains("Invalid code, please try again."), "{html}");
}

#[tokio::test]
async fn the_page_needs_a_session_and_its_own_form() {
    let (app, _) = app().await;
    let (_, user_code) = authorize(&app, "jcctl").await;
    // No session: back to the page (and its login), nothing answered.
    let (st, h, _) = post_form(
        &app,
        PAGE,
        &[
            ("csrf", "x"),
            ("user_code", &user_code),
            ("decision", "allow"),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(location(&h), PAGE);
    // A forged session cookie is no session.
    let (st, _, _) = post_form(
        &app,
        PAGE,
        &[
            ("csrf", "x"),
            ("user_code", &user_code),
            ("decision", "allow"),
        ],
        Some("__Host-wk_sso=forged"),
    )
    .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    // A session, but a form this session did not load.
    let sso = sign_in_at_page(&app, &user_code).await;
    let (st, html) = answer(&app, &sso, "not-the-token", &user_code, Some("allow")).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{html}");
}

#[tokio::test]
async fn wrong_user_codes_are_capped_per_person() {
    let (app, _) = app().await;
    let (device_code, user_code) = authorize(&app, "jcctl").await;
    let sso = sign_in_at_page(&app, &user_code).await;
    let csrf = page_form(&app, &sso, &user_code).await;
    for wrong in ["", "short", "ZZZZ-ZZZZ", "BBBB-BBBB", "XXXX-XXXX"] {
        let (st, _) = answer(&app, &sso, &csrf, wrong, None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{wrong}");
    }
    // The cap holds even for the right code now, and nothing was answered.
    let (st, html) = answer(&app, &sso, &csrf, &user_code, Some("allow")).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{html}");
    let (st, body) = poll(&app, "jcctl", &device_code).await;
    assert_eq!(
        (st, error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "authorization_pending")
    );
}
