#![forbid(unsafe_code)]
//! Browser-facing OIDC flows on the RI `dev` realm, served with the mount the `jc` router hands
//! over as an extension (W1, W7, W8, W9): realm info, JWKS, the authorization endpoint (errors
//! that never redirect, PKCE, PAR, SSO reuse, `prompt`, `max_age`, step-up, consent, response
//! modes), UserInfo and RP-initiated logout with back-channel logout.

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::{Extension, Router};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

mod common;
use common::*;

const AUTH: &str = "/realms/dev/protocol/openid-connect/auth";
const TOKEN: &str = "/realms/dev/protocol/openid-connect/token";
const CALLBACK: &str = "https://portal.dev.example.test/callback";
const VERIFIER: &str = "pkce-verifier-string-with-at-least-43-characters-long-012345";

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

async fn app() -> (
    Router,
    Arc<MemoryStore>,
    kaisatsu_server::KaisatsuState,
    Kek,
) {
    let (state, store, kek) = setup_dev_realm_state().await;
    let app = router(state.clone(), mount()).layer(Extension(mount()));
    (app, store, state, kek)
}

async fn get(
    app: &Router,
    uri: &str,
    cookie: Option<&str>,
    bearer: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    if let Some(t) = bearer {
        b = b.header(header::AUTHORIZATION, t);
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

fn location(h: &HeaderMap) -> String {
    h.get(header::LOCATION)
        .expect("location header")
        .to_str()
        .unwrap()
        .to_owned()
}

fn query_param(url: &str, name: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let pairs: Vec<(String, String)> = match u.fragment() {
        Some(f) if !f.is_empty() && u.query().is_none() => {
            url::form_urlencoded::parse(f.as_bytes())
                .into_owned()
                .collect()
        }
        _ => u.query_pairs().into_owned().collect(),
    };
    pairs.into_iter().find(|(k, _)| k == name).map(|(_, v)| v)
}

/// The `__Host-wk_sso=...` pair a response set, as a Cookie header value.
fn sso_cookie(h: &HeaderMap) -> String {
    h.get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("__Host-wk_sso="))
        .map(|v| v.split(';').next().unwrap().to_owned())
        .expect("sso cookie")
}

/// The `__Host-wk_auth=...` auth-session cookie a response set, as a Cookie header value (K1).
fn auth_cookie(h: &HeaderMap) -> String {
    h.get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("__Host-wk_auth="))
        .map(|v| v.split(';').next().unwrap().to_owned())
        .expect("auth cookie")
}

/// K5: logout without `id_token_hint` shows the confirmation page; this submits its form (the
/// single-use `logout_csrf_token` and the echoed hidden fields) as the browser would.
async fn confirm_logout(
    app: &Router,
    query: &str,
    cookie: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let logout = "/realms/dev/protocol/openid-connect/logout";
    let (st, h, html) = get(app, &format!("{logout}{query}"), cookie, None).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    assert!(html.contains("login-logout-confirm"), "{html}");
    assert!(
        h[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("nonce-")
    );
    let mut pairs = vec![(
        "logout_csrf_token".to_owned(),
        extract_hidden_input(&html, "logout_csrf_token"),
    )];
    for name in ["client_id", "post_logout_redirect_uri", "state"] {
        if html.contains(&format!("name=\"{name}\" value=\"")) {
            pairs.push((
                name.to_owned(),
                extract_hidden_input(&html, name).replace("&amp;", "&"),
            ));
        }
    }
    let refs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    post_form(app, logout, &refs, cookie).await
}

fn auth_query(extra: &str) -> String {
    format!(
        "{AUTH}?client_id=edge&redirect_uri={}&response_type=code&scope=openid&state=st-1&nonce=n-1&code_challenge={}&code_challenge_method=S256{extra}",
        url::form_urlencoded::byte_serialize(CALLBACK.as_bytes()).collect::<String>(),
        sha256_base64url(VERIFIER),
    )
}

/// Walks the login form for `demo.orgmember@hel.fi` from an authorization request that
/// redirected to the login page; returns the final response.
async fn login_from(app: &Router, login_loc: &str, auth: &str) -> (StatusCode, HeaderMap, String) {
    let (st, _, html) = get(app, login_loc, Some(auth), None).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    let csrf = extract_hidden_input(&html, "csrf_token");
    let session = extract_hidden_input(&html, "session_code");
    post_form(
        app,
        login_loc,
        &[
            ("csrf_token", &csrf),
            ("session_code", &session),
            ("username", "demo.orgmember@hel.fi"),
            ("password", "MemberPass123!Valid"),
        ],
        Some(auth),
    )
    .await
}

/// Logs in through the browser flow; returns the code and the SSO cookie.
async fn browser_login(app: &Router) -> (String, String) {
    let (st, h, _) = get(app, &auth_query(""), None, None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let (st, h2, body) = login_from(app, &location(&h), &auth_cookie(&h)).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    let loc = location(&h2);
    assert!(loc.starts_with(CALLBACK), "{loc}");
    assert_eq!(query_param(&loc, "state").as_deref(), Some("st-1"));
    (query_param(&loc, "code").expect("code"), sso_cookie(&h2))
}

async fn exchange(app: &Router, code: &str) -> Value {
    let (st, _, body) = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("code", code),
            ("redirect_uri", CALLBACK),
            ("code_verifier", VERIFIER),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    serde_json::from_str(&body).unwrap()
}

#[tokio::test]
async fn realm_info_discovery_and_jwks() {
    let (app, _, _, _) = app().await;
    let (st, _, body) = get(&app, "/realms/dev", None, None).await;
    assert_eq!(st, StatusCode::OK);
    let info: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(info["realm"], "dev");
    assert_eq!(
        info["token-service"],
        "https://idm.dev.example.test/realms/dev/protocol/openid-connect"
    );
    for path in [
        "/realms/dev/.well-known/openid-configuration",
        "/realms/dev/.well-known/oauth-authorization-server",
    ] {
        let (st, _, body) = get(&app, path, None, None).await;
        assert_eq!(st, StatusCode::OK, "{path}");
        let d: Value = serde_json::from_str(&body).unwrap();
        assert!(d.get("registration_endpoint").is_none());
        assert_eq!(d["code_challenge_methods_supported"], json!(["S256"]));
        // Public clients (CIMD claude.ai) authenticate with `none` (RFC 8414 2).
        assert!(
            d["token_endpoint_auth_methods_supported"]
                .as_array()
                .unwrap()
                .contains(&json!("none"))
        );
    }
    let (st, h, body) = get(
        &app,
        "/realms/dev/protocol/openid-connect/certs",
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h[header::CACHE_CONTROL], "public, max-age=300");
    let jwks: Value = serde_json::from_str(&body).unwrap();
    let keys = jwks["keys"].as_array().unwrap();
    assert!(!keys.is_empty());
    for k in keys {
        assert!(k.get("d").is_none(), "no private member in a JWKS: {k}");
        assert!(k["kid"].is_string());
    }
    for path in [
        "/realms/nope",
        "/realms/nope/.well-known/openid-configuration",
        "/realms/nope/protocol/openid-connect/certs",
        "/realms/nope/protocol/openid-connect/auth?client_id=edge",
        "/realms/nope/protocol/openid-connect/userinfo",
        "/realms/nope/protocol/openid-connect/logout",
    ] {
        let (st, _, _) = get(&app, path, None, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn authorization_errors_are_shown_never_redirected() {
    let (app, store, _, _) = app().await;
    let cb = url::form_urlencoded::byte_serialize(CALLBACK.as_bytes()).collect::<String>();
    let challenge = sha256_base64url(VERIFIER);
    let cases = [
        format!("{AUTH}?redirect_uri={cb}&response_type=code"),
        format!("{AUTH}?client_id=no-such-client&redirect_uri={cb}&response_type=code"),
        format!(
            "{AUTH}?client_id=edge&redirect_uri={cb}&response_type=token&code_challenge={challenge}&code_challenge_method=S256"
        ),
        format!(
            "{AUTH}?client_id=edge&response_type=code&code_challenge={challenge}&code_challenge_method=S256"
        ),
        format!(
            "{AUTH}?client_id=edge&redirect_uri=https%3A%2F%2Fevil.example%2Fcb&response_type=code&code_challenge={challenge}&code_challenge_method=S256"
        ),
        format!("{AUTH}?client_id=edge&redirect_uri={cb}&response_type=code"),
        format!(
            "{AUTH}?client_id=edge&redirect_uri={cb}&response_type=code&code_challenge={challenge}&code_challenge_method=plain"
        ),
        format!(
            "{AUTH}?client_id=edge&redirect_uri={cb}&response_type=code&code_challenge=short&code_challenge_method=S256"
        ),
        format!(
            "{AUTH}?client_id=edge&redirect_uri={cb}&response_type=code&code_challenge={challenge}&code_challenge_method=S256&scope=openid%20gateway-hub"
        ),
        format!(
            "{AUTH}?client_id=apisix-gateway&redirect_uri={cb}&response_type=code&code_challenge={challenge}&code_challenge_method=S256"
        ),
        format!(
            "{AUTH}?client_id=edge&request_uri=urn%3Aietf%3Aparams%3Aoauth%3Arequest_uri%3Anope"
        ),
    ];
    for uri in &cases {
        let (st, h, body) = get(&app, uri, None, None).await;
        assert!(
            st.is_client_error(),
            "{uri}: {st} {body} {:?}",
            h.get(header::LOCATION)
        );
        assert!(h.get(header::LOCATION).is_none(), "{uri}");
    }
    // A disabled client is refused like an unknown one.
    let mut edge = store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .unwrap();
    edge.enabled = false;
    store.upsert_client(edge).await.unwrap();
    let (st, _, _) = get(&app, &auth_query(""), None, None).await;
    assert!(st.is_client_error());
}

#[tokio::test]
async fn sso_session_prompt_max_age_step_up_and_response_modes() {
    let (app, store, _, _) = app().await;
    let (code, sso) = browser_login(&app).await;
    let tokens = exchange(&app, &code).await;
    assert!(tokens["id_token"].is_string());

    // SSO: a second request with the cookie gets a code without the login page.
    let (st, h, _) = get(&app, &auth_query(""), Some(&sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let loc = location(&h);
    assert!(loc.starts_with(CALLBACK), "{loc}");
    exchange(&app, &query_param(&loc, "code").unwrap()).await;

    // prompt=none with the cookie: a code; without it: login_required back to the client.
    let (st, h, _) = get(&app, &auth_query("&prompt=none"), Some(&sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(query_param(&location(&h), "code").is_some());
    let (st, h, _) = get(&app, &auth_query("&prompt=none"), None, None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let loc = location(&h);
    assert_eq!(
        query_param(&loc, "error").as_deref(),
        Some("login_required")
    );
    assert_eq!(query_param(&loc, "state").as_deref(), Some("st-1"));
    // An unknown cookie is no session.
    let (st, h, _) = get(
        &app,
        &auth_query("&prompt=none"),
        Some("__Host-wk_sso=forged; other=1"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        query_param(&location(&h), "error").as_deref(),
        Some("login_required")
    );

    // prompt=login, prompt=select_account and an exceeded max_age force the login page.
    for extra in ["&prompt=login", "&prompt=select_account", "&max_age=-1"] {
        let (st, h, _) = get(&app, &auth_query(extra), Some(&sso), None).await;
        assert_eq!(st, StatusCode::SEE_OTHER, "{extra}");
        assert!(
            location(&h).contains("/login-actions/authenticate"),
            "{extra}"
        );
        let (st, h, _) = get(
            &app,
            &auth_query(&format!("{extra}&prompt=none")),
            Some(&sso),
            None,
        )
        .await;
        if extra != "&max_age=-1" {
            continue;
        }
        assert_eq!(st, StatusCode::SEE_OTHER);
        assert_eq!(
            query_param(&location(&h), "error").as_deref(),
            Some("login_required")
        );
    }
    let (st, h, _) = get(&app, &auth_query("&max_age=3600"), Some(&sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(
        location(&h).starts_with(CALLBACK),
        "a recent login satisfies max_age"
    );

    // Step-up: acr_values=mfa, or an essential acr in `claims`, on a password session.
    let claims_param = url::form_urlencoded::byte_serialize(
        br#"{"id_token":{"acr":{"essential":true,"values":["mfa"]}}}"#,
    )
    .collect::<String>();
    for extra in [
        "&acr_values=mfa".to_owned(),
        format!("&claims={claims_param}"),
        format!(
            "&claims={}",
            url::form_urlencoded::byte_serialize(br#"{"userinfo":{"acr":{"value":"mfa"}}}"#)
                .collect::<String>()
        ),
    ] {
        let (st, h, _) = get(&app, &auth_query(&extra), Some(&sso), None).await;
        assert_eq!(st, StatusCode::SEE_OTHER, "{extra}");
        let login = location(&h);
        assert!(
            login.contains("/login-actions/authenticate"),
            "{extra}: {login}"
        );
        let (st, _, html) = login_from(&app, &login, &auth_cookie(&h)).await;
        assert_eq!(st, StatusCode::OK, "{extra}: {html}");
        assert!(
            html.contains(r#"name="otp""#),
            "the second factor is asked for after the password: {extra}"
        );
        let (st, h, _) = get(
            &app,
            &auth_query(&format!("{extra}&prompt=none")),
            Some(&sso),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::SEE_OTHER);
        assert_eq!(
            query_param(&location(&h), "error").as_deref(),
            Some("login_required")
        );
    }
    // A malformed `claims` parameter asks for nothing.
    let (st, h, _) = get(&app, &auth_query("&claims=not-json"), Some(&sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(location(&h).starts_with(CALLBACK));

    // Response modes on the SSO path.
    let (st, h, _) = get(
        &app,
        &auth_query("&response_mode=fragment"),
        Some(&sso),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let loc = location(&h);
    assert!(loc.contains("#code="), "{loc}");
    let (st, h, html) = get(
        &app,
        &auth_query("&response_mode=form_post&state=%22%3Cx%3E"),
        Some(&sso),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    assert!(html.contains(r#"name="code""#), "{html}");
    assert!(
        html.contains("&quot;&lt;x&gt;"),
        "the state is escaped: {html}"
    );
    assert!(!html.contains("\"<x>"));

    // Consent: a client that needs it gets no silent code until consent is given.
    let mut edge = store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .unwrap();
    edge.consent_required = true;
    store.upsert_client(edge).await.unwrap();
    let (st, h, _) = get(&app, &auth_query("&prompt=none"), Some(&sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        query_param(&location(&h), "error").as_deref(),
        Some("consent_required")
    );
    let (st, h, _) = get(&app, &auth_query(""), Some(&sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(location(&h).contains("/login-actions/authenticate"));
    let user = store
        .get_user_by_username_or_email("dev", None, "demo.orgmember@hel.fi")
        .await
        .unwrap()
        .unwrap();
    store
        .upsert_consent(ConsentRecord {
            id: String::new(),
            realm_id: "dev".into(),
            org_id: None,
            user_id: user.id.clone(),
            client_id: "edge".into(),
            granted_client_scopes: vec!["openid".into()],
            created_date: 1,
            last_updated_date: 1,
        })
        .await
        .unwrap();
    let (st, h, _) = get(&app, &auth_query("&prompt=none"), Some(&sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(query_param(&location(&h), "code").is_some());
    let (st, h, _) = get(&app, &auth_query(""), Some(&sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(location(&h).starts_with(CALLBACK));
    let (st, h, _) = get(&app, &auth_query("&prompt=consent"), Some(&sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(location(&h).contains("/login-actions/authenticate"));
}

#[tokio::test]
async fn interactive_login_answers_in_the_requested_response_mode() {
    let (app, _, _, _) = app().await;
    let (st, h, _) = get(&app, &auth_query("&response_mode=form_post"), None, None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let (st, h2, html) = login_from(&app, &location(&h), &auth_cookie(&h)).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    assert!(html.contains(&format!(r#"action="{CALLBACK}""#)), "{html}");
    assert!(html.contains(r#"name="code""#));
    assert!(html.contains(r#"name="state" value="st-1""#));
    let _ = sso_cookie(&h2);

    let (st, h, _) = get(&app, &auth_query("&response_mode=fragment"), None, None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let (st, h2, _) = login_from(&app, &location(&h), &auth_cookie(&h)).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let loc = location(&h2);
    assert!(
        loc.contains("#code=") && loc.contains("state=st-1"),
        "{loc}"
    );
    exchange(&app, &query_param(&loc, "code").unwrap()).await;
}

#[tokio::test]
async fn pushed_authorization_requests() {
    let (app, _, _, _) = app().await;
    let par = "/realms/dev/protocol/openid-connect/ext/par/request";
    let challenge = sha256_base64url(VERIFIER);
    let base = [
        ("client_id", "edge"),
        ("client_secret", "edge-secret-123"),
        ("response_type", "code"),
        ("redirect_uri", CALLBACK),
        ("scope", "openid"),
        ("state", "par-state"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
    ];
    let (st, h, body) = post_form(&app, par, &base, None).await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    let ans: Value = serde_json::from_str(&body).unwrap();
    let request_uri = ans["request_uri"].as_str().unwrap().to_owned();
    assert!(request_uri.starts_with("urn:ietf:params:oauth:request_uri:"));
    assert_eq!(ans["expires_in"], 60);
    let enc = url::form_urlencoded::byte_serialize(request_uri.as_bytes()).collect::<String>();

    // Another client cannot use it; the owner can, once.
    let (st, _, _) = get(
        &app,
        &format!("{AUTH}?client_id=apisix-gateway&request_uri={enc}"),
        None,
        None,
    )
    .await;
    assert!(st.is_client_error());
    let (st, _, body) = post_form(&app, par, &base, None).await;
    assert_eq!(st, StatusCode::CREATED);
    let second: Value = serde_json::from_str(&body).unwrap();
    let enc2 =
        url::form_urlencoded::byte_serialize(second["request_uri"].as_str().unwrap().as_bytes())
            .collect::<String>();
    let (st, h, _) = get(
        &app,
        &format!("{AUTH}?client_id=edge&request_uri={enc2}"),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let (st, h2, _) = login_from(&app, &location(&h), &auth_cookie(&h)).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        query_param(&location(&h2), "state").as_deref(),
        Some("par-state")
    );
    let (st, _, _) = get(
        &app,
        &format!("{AUTH}?client_id=edge&request_uri={enc2}"),
        None,
        None,
    )
    .await;
    assert!(st.is_client_error(), "a request_uri is single-use");

    // A bad client secret, a bad redirect and a missing PKCE challenge are refused at PAR.
    let mut wrong = base.to_vec();
    wrong[1] = ("client_secret", "wrong");
    let (st, _, _) = post_form(&app, par, &wrong, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let mut bad_redirect = base.to_vec();
    bad_redirect[3] = ("redirect_uri", "https://evil.example/cb");
    let (st, _, _) = post_form(&app, par, &bad_redirect, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let no_pkce: Vec<_> = base
        .iter()
        .filter(|(k, _)| !k.starts_with("code_challenge"))
        .copied()
        .collect();
    let (st, _, _) = post_form(&app, par, &no_pkce, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _, _) = post_form(
        &app,
        "/realms/nope/protocol/openid-connect/ext/par/request",
        &base,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn userinfo_needs_a_live_access_token() {
    let (app, store, _, _) = app().await;
    let (code, sso) = browser_login(&app).await;
    let tokens = exchange(&app, &code).await;
    let access = tokens["access_token"].as_str().unwrap();
    let ui = "/realms/dev/protocol/openid-connect/userinfo";
    let (st, _, body) = get(&app, ui, None, Some(&format!("Bearer {access}"))).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let info: Value = serde_json::from_str(&body).unwrap();
    assert!(info["sub"].is_string());
    assert_eq!(info["email"], "demo.orgmember@hel.fi");
    let (st, _, body) = post_form(&app, ui, &[], None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
    for auth in [
        "Bearer ".to_owned(),
        "Bearer not-a-jwt".to_owned(),
        format!("Bearer {}", tokens["id_token"].as_str().unwrap()),
        format!("Basic {access}"),
    ] {
        let (st, _, _) = get(&app, ui, None, Some(&auth)).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{auth}");
    }
    // A DPoP scheme without a proof is refused.
    let (st, _, _) = get(&app, ui, None, Some(&format!("DPoP {access}"))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // A disabled person gets nothing.
    let mut user = store
        .get_user_by_username_or_email("dev", None, "demo.orgmember@hel.fi")
        .await
        .unwrap()
        .unwrap();
    user.enabled = false;
    store.update_user(user.clone()).await.unwrap();
    let (st, _, _) = get(&app, ui, None, Some(&format!("Bearer {access}"))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    user.enabled = true;
    store.update_user(user).await.unwrap();

    // After logout the token's session is gone, and so are its claims.
    let (st, _, _) = confirm_logout(&app, "", Some(&sso)).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = get(&app, ui, None, Some(&format!("Bearer {access}"))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn rp_initiated_logout_and_back_channel_logout() {
    let (app, store, state, kek) = app().await;
    let backchannel = "https://portal.dev.example.test/backchannel-logout";
    state.guard().insert_mock(backchannel, 204, Vec::new());
    let mut edge = store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .unwrap();
    edge.attributes
        .insert("backchannel.logout.url".into(), backchannel.into());
    edge.attributes.insert(
        "post.logout.redirect.uris".into(),
        "https://portal.dev.example.test/signed-out".into(),
    );
    store.upsert_client(edge).await.unwrap();
    let logout = "/realms/dev/protocol/openid-connect/logout";

    let (code, sso) = browser_login(&app).await;
    let tokens = exchange(&app, &code).await;
    let id_token = tokens["id_token"].as_str().unwrap().to_owned();
    let sid = decode_jwt_payload(&id_token)["sid"]
        .as_str()
        .unwrap()
        .to_owned();
    let signed_out =
        url::form_urlencoded::byte_serialize(b"https://portal.dev.example.test/signed-out")
            .collect::<String>();

    // Refused: an unregistered target, a target without a client, an unknown client, a bad hint.
    for q in [
        format!("?id_token_hint={id_token}&post_logout_redirect_uri=https%3A%2F%2Fevil.example%2F"),
        format!("?post_logout_redirect_uri={signed_out}"),
        format!("?client_id=nope&post_logout_redirect_uri={signed_out}"),
        "?id_token_hint=not-a-jwt".to_owned(),
    ] {
        let (st, _, _) = get(&app, &format!("{logout}{q}"), None, None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{q}");
    }
    assert!(
        store.get_sso_session("dev", &sid).await.unwrap().is_some(),
        "nothing ended yet"
    );

    let (st, h, _) = get(
        &app,
        &format!(
            "{logout}?id_token_hint={id_token}&post_logout_redirect_uri={signed_out}&state=bye"
        ),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        location(&h),
        "https://portal.dev.example.test/signed-out?state=bye"
    );
    let cleared = h[header::SET_COOKIE].to_str().unwrap();
    assert!(
        cleared.starts_with("__Host-wk_sso=;") && cleared.contains("Max-Age=0"),
        "{cleared}"
    );
    assert!(store.get_sso_session("dev", &sid).await.unwrap().is_none());
    // The old cookie no longer opens a session.
    let (st, h, _) = get(&app, &auth_query("&prompt=none"), Some(&sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        query_param(&location(&h), "error").as_deref(),
        Some("login_required")
    );

    // client_id instead of a hint, and POST.
    let (code, _) = browser_login(&app).await;
    let tokens = exchange(&app, &code).await;
    let sid = decode_jwt_payload(tokens["id_token"].as_str().unwrap())["sid"]
        .as_str()
        .unwrap()
        .to_owned();
    let (st, h, _) = confirm_logout(
        &app,
        &format!("?client_id=edge&post_logout_redirect_uri={signed_out}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(location(&h), "https://portal.dev.example.test/signed-out");
    assert!(
        store.get_sso_session("dev", &sid).await.unwrap().is_some(),
        "no hint, no cookie: no session to end"
    );
    // K5: a POST without the confirmation token ends nothing.
    let (st, _, _) = post_form(&app, logout, &[], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // A hint without `sid` ends the person's session found by `sub`.
    let key = store.list_signing_keys("dev").await.unwrap().remove(0);
    let user = store
        .get_user_by_username_or_email("dev", None, "demo.orgmember@hel.fi")
        .await
        .unwrap()
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    let hint = sign_jwt(
        &key,
        &kek,
        Some("JWT"),
        &json!({ "iss": "https://idm.dev.example.test/realms/dev", "sub": user.id, "azp": "edge", "typ": "ID", "iat": now, "exp": now + 60 }),
    )
    .unwrap();
    let (st, _, _) = get(&app, &format!("{logout}?id_token_hint={hint}"), None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(store.get_sso_session("dev", &sid).await.unwrap().is_none());
}

async fn admin_token(store: &MemoryStore, kek: &Kek, roles: Value, extra: Value) -> String {
    let key = store
        .list_signing_keys("dev")
        .await
        .unwrap()
        .into_iter()
        .find(|k| k.status == "ACTIVE")
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    let mut claims = json!({ "iss": "https://idm.dev.example.test/realms/dev", "sub": "admin-1",
        "azp": "admin-cli", "typ": "Bearer", "iat": now, "exp": now + 600, "realm_access": { "roles": roles } });
    for (k, v) in extra.as_object().unwrap() {
        claims[k] = v.clone();
    }
    sign_jwt(&key, kek, Some("JWT"), &claims).unwrap()
}

#[tokio::test]
async fn every_entity_set_reads_the_rows_a_login_leaves() {
    let (app, store, _, kek) = app().await;
    let (code, _) = browser_login(&app).await;
    exchange(&app, &code).await;
    let user = store
        .get_user_by_username_or_email("dev", None, "demo.orgmember@hel.fi")
        .await
        .unwrap()
        .unwrap();
    let now = chrono::Utc::now();
    store
        .upsert_consent(ConsentRecord {
            id: "consent-1".into(),
            realm_id: "dev".into(),
            org_id: None,
            user_id: user.id.clone(),
            client_id: "edge".into(),
            granted_client_scopes: vec!["openid".into()],
            created_date: 1,
            last_updated_date: 2,
        })
        .await
        .unwrap();
    store
        .upsert_offline_session(OfflineSession {
            id: "off-1".into(),
            realm_id: "dev".into(),
            org_id: None,
            user_id: user.id.clone(),
            client_id: "edge".into(),
            scopes: vec!["offline_access".into()],
            created_at: now,
            last_access: now,
            idle_expires_at: now + chrono::Duration::days(1),
            max_expires_at: now + chrono::Duration::days(2),
        })
        .await
        .unwrap();
    store
        .upsert_idp(IdentityProviderRepresentation {
            internal_id: "idp-1".into(),
            realm_id: "dev".into(),
            org_id: None,
            alias: "upstream".into(),
            provider_id: "oidc".into(),
            enabled: true,
            trust_email: false,
            first_broker_login_flow_alias: "first broker login".into(),
            config: Default::default(),
            managed_by: None,
        })
        .await
        .unwrap();
    store
        .add_role_mapping(RoleMappingRecord {
            id: "rm-1".into(),
            realm_id: "dev".into(),
            org_id: None,
            principal_kind: "user".into(),
            principal_id: user.id.clone(),
            role_id: "role-1".into(),
            role_name: "auditor".into(),
            client_uuid: None,
        })
        .await
        .unwrap();
    let admin = admin_token(&store, &kek, json!(["realm-admin"]), json!({})).await;
    let expect_rows: &[(&str, &str, &str)] = &[
        ("Realms", "id", "dev"),
        ("People", "username", "demo.orgmember@hel.fi"),
        ("Groups", "", ""),
        ("Roles", "", ""),
        ("RoleMappings", "role_name", "auditor"),
        ("Applications", "clientId", "edge"),
        ("ClientScopes", "", ""),
        ("ProtocolMappers", "", ""),
        ("IdentityProviders", "alias", "upstream"),
        ("Credentials", "type", "password"),
        ("Sessions", "username", "demo.orgmember@hel.fi"),
        ("ClientSessions", "client_id", "edge"),
        ("OfflineSessions", "id", "off-1"),
        ("Consents", "clientId", "edge"),
        ("Events", "", ""),
        ("SigningKeys", "", ""),
    ];
    for (set, field, value) in expect_rows {
        let req = Request::builder()
            .uri(format!("/api/v1/realms/dev/odata/{set}"))
            .header(header::AUTHORIZATION, format!("Bearer {admin}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let st = resp.status();
        let body: Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(st, StatusCode::OK, "{set}: {body}");
        let rows = body["value"].as_array().unwrap();
        assert!(!rows.is_empty(), "{set} has rows");
        for r in rows {
            assert!(
                r["realm_id"].as_str().or(r["id"].as_str()).is_some(),
                "{set}"
            );
            for secret in [
                "secretData",
                "secret",
                "encryptedPrivateKey",
                "handle_hash",
                "passwordHash",
            ] {
                assert!(r.get(secret).is_none(), "{set} leaks {secret}");
            }
        }
        if !field.is_empty() {
            assert!(
                rows.iter().any(|r| r[*field] == *value),
                "{set}: no row with {field} = {value}: {rows:?}"
            );
        }
    }
    // An organization member sees only its organization's rows, and no realm-level sets.
    let member = admin_token(
        &store,
        &kek,
        json!([]),
        json!({ "sub": user.id, "jc_org": ["hel"] }),
    )
    .await;
    for set in [
        "Realms",
        "IdentityProviders",
        "SigningKeys",
        "Sessions",
        "Consents",
    ] {
        let req = Request::builder()
            .uri(format!("/api/v1/realms/dev/odata/{set}"))
            .header(header::AUTHORIZATION, format!("Bearer {member}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let st = resp.status();
        let body: Value =
            serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
        if st == StatusCode::OK {
            assert!(
                body["value"].as_array().unwrap().is_empty(),
                "{set} shows a member nothing: {body}"
            );
        } else {
            assert!(st.is_client_error(), "{set}: {st}");
        }
    }
}

#[tokio::test]
async fn an_empty_in_memory_state_knows_no_realm() {
    let state = kaisatsu_server::KaisatsuState::in_memory_for_tests();
    assert!(state.store().list_realms().await.unwrap().is_empty());
    let app = router(state, mount());
    let (st, _, _) = get(&app, "/realms/dev", None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

/// K5 (OIDC RP-Initiated Logout 1.0 §2, §6): without `id_token_hint` the logout endpoint shows a
/// confirmation page and leaves the session alive; only a POST carrying the single-use token
/// minted for that session ends it. Red before K5: the GET ended the session at once.
#[tokio::test]
async fn k5_logout_without_hint_needs_a_confirmed_single_use_token() {
    let (app, store, _, _) = app().await;
    let logout = "/realms/dev/protocol/openid-connect/logout";
    let (code, sso) = browser_login(&app).await;
    let tokens = exchange(&app, &code).await;
    let sid = decode_jwt_payload(tokens["id_token"].as_str().unwrap())["sid"]
        .as_str()
        .unwrap()
        .to_owned();
    let alive = || async { store.get_sso_session("dev", &sid).await.unwrap().is_some() };

    for lang in ["en", "sk", "cs", "de", "hu"] {
        let (st, _, html) = get(
            &app,
            &format!("{logout}?ui_locales={lang}"),
            Some(&sso),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(html.contains(&format!("lang=\"{lang}\"")), "{lang}: {html}");
    }
    assert!(alive().await, "the confirmation page ends nothing");

    let (_, _, html) = get(&app, logout, Some(&sso), None).await;
    let token = extract_hidden_input(&html, "logout_csrf_token");
    // A forged token, and a POST without one, are refused.
    for pairs in [vec![("logout_csrf_token", "Zm9yZ2Vk")], vec![]] {
        let (st, _, _) = post_form(&app, logout, &pairs, Some(&sso)).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }
    // A token minted for no session (another browser) does not end this one.
    let (_, _, anon) = get(&app, logout, None, None).await;
    let anon_token = extract_hidden_input(&anon, "logout_csrf_token");
    let (st, _, _) = post_form(
        &app,
        logout,
        &[("logout_csrf_token", &anon_token)],
        Some(&sso),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(alive().await);

    let (st, h, body) = post_form(&app, logout, &[("logout_csrf_token", &token)], Some(&sso)).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(
        body.contains("data-page-id=\"login-info\"") && body.contains("You are logged out"),
        "{body}"
    );
    assert!(
        h[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    assert!(!alive().await, "the confirmed POST ends the session");

    // Single use: the anonymous token works once, then is refused.
    let (st, _, _) = post_form(&app, logout, &[("logout_csrf_token", &anon_token)], None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = post_form(&app, logout, &[("logout_csrf_token", &anon_token)], None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

/// K4 (OIDC Core §8.1): with a pairwise mapper on the client, the ID token, the access token,
/// userinfo and introspection all carry one pairwise `sub`, never the local user id.
#[tokio::test]
async fn k4_pairwise_sub_agrees_across_tokens_userinfo_and_introspection() {
    let (app, store, _, _) = app().await;
    let mut edge = store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .unwrap();
    edge.protocol_mappers.push(ProtocolMapperRepresentation {
        id: "pm-pairwise".into(),
        name: "pairwise subject".into(),
        protocol: "openid-connect".into(),
        protocol_mapper: "oidc-sha256-pairwise-sub-mapper".into(),
        config: [
            (
                "sectorIdentifierUri",
                "https://sector.example.test/redirects.json",
            ),
            ("pairwiseSubAlgorithmSalt", "k4-salt"),
            ("id.token.claim", "true"),
            ("access.token.claim", "true"),
            ("userinfo.token.claim", "true"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect(),
        ..Default::default()
    });
    store.upsert_client(edge).await.unwrap();
    let user = store
        .get_user_by_username_or_email("dev", None, "demo.orgmember@hel.fi")
        .await
        .unwrap()
        .unwrap();

    let (code, _) = browser_login(&app).await;
    let tokens = exchange(&app, &code).await;
    let access = tokens["access_token"].as_str().unwrap();
    let id_sub = decode_jwt_payload(tokens["id_token"].as_str().unwrap())["sub"].clone();
    let at_sub = decode_jwt_payload(access)["sub"].clone();
    assert_ne!(id_sub, json!(user.id), "the local id stays hidden");
    assert_eq!(id_sub, at_sub);

    let (st, _, body) = get(
        &app,
        "/realms/dev/protocol/openid-connect/userinfo",
        None,
        Some(&format!("Bearer {access}")),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let ui: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(ui["sub"], id_sub);
    assert!(!body.contains(&user.id), "{body}");

    let (st, _, body) = post_form(
        &app,
        "/realms/dev/protocol/openid-connect/token/introspect",
        &[
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("token", tokens["refresh_token"].as_str().unwrap()),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let intro: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(intro["active"], true, "{intro}");
    assert_eq!(intro["sub"], id_sub);
}

/// K4: userinfo resolves a non-local `sub` through the token's session only when it equals the
/// pairwise subject the client would see. Red before: any `sub` with a live `sid` answered with
/// the session person's claims.
#[tokio::test]
async fn k4_userinfo_refuses_a_sub_that_is_not_the_clients_subject() {
    let (app, store, _, kek) = app().await;
    let (code, _) = browser_login(&app).await;
    let tokens = exchange(&app, &code).await;
    let real = decode_jwt_payload(tokens["access_token"].as_str().unwrap());
    let key = store
        .list_signing_keys("dev")
        .await
        .unwrap()
        .into_iter()
        .find(|k| k.status == "ACTIVE")
        .unwrap();
    let mut forged = real.clone();
    forged["sub"] = json!("someone-else-entirely");
    forged["jti"] = json!("k4-forged-sub");
    let token = sign_jwt(&key, &kek, Some("JWT"), &forged).unwrap();
    let ui = "/realms/dev/protocol/openid-connect/userinfo";
    let (st, _, body) = get(&app, ui, None, Some(&format!("Bearer {token}"))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
    // The untouched token still answers.
    let access = tokens["access_token"].as_str().unwrap();
    let (st, _, _) = get(&app, ui, None, Some(&format!("Bearer {access}"))).await;
    assert_eq!(st, StatusCode::OK);
}
