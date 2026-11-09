#![forbid(unsafe_code)]
//! The login pages step by step (W7, W12): expired and foreign sessions, CSRF, the timing-equal
//! refusals, legacy hash migration, TOTP (wrong, replayed, enrolled), temporary passwords, terms,
//! consent, the organization picker and the reset-credentials pages.

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::{Extension, Router};
use base64::engine::{Engine, general_purpose::STANDARD as B64};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use kaisatsu_store_memory::MemoryStore;
use std::sync::Arc;
use tower::ServiceExt;

mod common;
use common::*;

const AUTH: &str = "/realms/dev/protocol/openid-connect/auth";
const CALLBACK: &str = "https://portal.dev.example.test/callback";
const VERIFIER: &str = "pkce-verifier-string-with-at-least-43-characters-long-012345";
const MEMBER: &str = "demo.orgmember@hel.fi";
const MEMBER_PW: &str = "MemberPass123!Valid";
const STEWARD: &str = "demo.steward@hel.fi";
const STEWARD_PW: &str = "StewardPass123!Valid";

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

async fn app() -> (Router, Arc<MemoryStore>) {
    let (state, store, _) = setup_dev_realm_state().await;
    (router(state, mount()).layer(Extension(mount())), store)
}

async fn get(app: &Router, uri: &str, lang: Option<&str>) -> (StatusCode, HeaderMap, String) {
    get_with_cookie(app, uri, lang, None).await
}

async fn get_with_cookie(
    app: &Router,
    uri: &str,
    lang: Option<&str>,
    cookie: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some(l) = lang {
        b = b.header(header::ACCEPT_LANGUAGE, l);
    }
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

fn location(h: &HeaderMap) -> String {
    h[header::LOCATION].to_str().unwrap().to_owned()
}

fn page_id(html: &str) -> String {
    let marker = "data-page-id=\"login-";
    match html.find(marker) {
        Some(i) => {
            let rest = &html[i + marker.len()..];
            rest[..rest.find('"').unwrap()].to_owned()
        }
        None => String::new(),
    }
}

/// One login attempt: the authorization request's login page URL, its CSRF token, session and auth cookie.
struct Attempt {
    url: String,
    csrf: String,
    session: String,
    cookie: String,
}

async fn start(app: &Router, extra: &str) -> Attempt {
    let uri = format!(
        "{AUTH}?client_id=edge&redirect_uri={}&response_type=code&scope=openid&state=st&code_challenge={}&code_challenge_method=S256{extra}",
        url::form_urlencoded::byte_serialize(CALLBACK.as_bytes()).collect::<String>(),
        sha256_base64url(VERIFIER),
    );
    let (st, h, _) = get(app, &uri, None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let url = location(&h);
    let cookie = h[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let (st, _, html) = get_with_cookie(app, &url, None, Some(&cookie)).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    Attempt {
        csrf: extract_hidden_input(&html, "csrf_token"),
        session: extract_hidden_input(&html, "session_code"),
        url,
        cookie,
    }
}

impl Attempt {
    async fn post(&self, app: &Router, fields: &[(&str, &str)]) -> (StatusCode, HeaderMap, String) {
        let mut all = vec![
            ("csrf_token", self.csrf.as_str()),
            ("session_code", self.session.as_str()),
        ];
        all.extend_from_slice(fields);
        post_form(app, &self.url, &all, Some(&self.cookie)).await
    }

    async fn password(
        &self,
        app: &Router,
        user: &str,
        pw: &str,
    ) -> (StatusCode, HeaderMap, String) {
        self.post(app, &[("username", user), ("password", pw)])
            .await
    }
}

fn is_code(st: StatusCode, h: &HeaderMap) -> bool {
    st == StatusCode::SEE_OTHER
        && location(h).starts_with(CALLBACK)
        && location(h).contains("code=")
}

async fn user(store: &MemoryStore, name: &str) -> UserRepresentation {
    store
        .get_user_by_username_or_email("dev", None, name)
        .await
        .unwrap()
        .unwrap()
}

/// jc-multiorg-portal: a client on another host than kaisatsu (an Organization's Portal host).
/// Browsers apply `form-action` to the redirect after the sign-in POST, so a login page allowing
/// `'self'` alone left Chromium on the form: its CSP names the flow's own redirect origin, and
/// only that one; a page that ends no flow keeps `'self'`.
#[tokio::test]
async fn the_sign_in_form_may_end_at_its_own_flows_redirect_origin_alone() {
    let (app, _) = app().await;
    let a = start(&app, "").await;
    let (_, h, html) = get_with_cookie(&app, &a.url, None, Some(&a.cookie)).await;
    assert_eq!(page_id(&html), "login", "{html}");
    let csp = h[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(
        csp.ends_with("form-action 'self' https://portal.dev.example.test"),
        "{csp}"
    );
    let (st, h, _) = a.password(&app, MEMBER, MEMBER_PW).await;
    assert!(is_code(st, &h), "the redirect the form-action now allows");
    let (_, h, _) = get(
        &app,
        "/realms/dev/login-actions/authenticate?session_code=nope",
        None,
    )
    .await;
    let csp = h[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    assert!(csp.ends_with("form-action 'self'"), "{csp}");
}

#[tokio::test]
async fn expired_foreign_and_forged_sessions() {
    let (app, store) = app().await;
    let (st, h, html) = get(
        &app,
        "/realms/dev/login-actions/authenticate?session_code=nope",
        Some("sk"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page_id(&html), "expired", "{html}");
    assert!(!html.contains("csrf_token"));
    assert!(
        h[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    assert_eq!(h[header::X_FRAME_OPTIONS], "DENY");

    let a = start(&app, "").await;
    // A link naming another client is an expired page, and the POST is refused.
    let foreign = format!(
        "/realms/dev/login-actions/authenticate?session_code={}&client_id=apisix-gateway",
        a.session
    );
    let (_, _, html) = get_with_cookie(&app, &foreign, None, Some(&a.cookie)).await;
    assert_eq!(page_id(&html), "expired");
    let (st, _, _) = post_form(
        &app,
        &foreign,
        &[
            ("csrf_token", &a.csrf),
            ("username", MEMBER),
            ("password", MEMBER_PW),
        ],
        Some(&a.cookie),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // A wrong or missing CSRF token is refused before any password is looked at.
    for csrf in ["", "forged"] {
        let (st, _, _) = post_form(
            &app,
            &a.url,
            &[
                ("csrf_token", csrf),
                ("session_code", &a.session),
                ("username", MEMBER),
                ("password", MEMBER_PW),
            ],
            Some(&a.cookie),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }
    let failures = store
        .get_login_failure("dev", &format!("user:{}", user(&store, MEMBER).await.id))
        .await
        .unwrap();
    assert!(
        failures.is_none_or(|f| f.num_failures == 0),
        "a CSRF refusal counts nothing"
    );
    for path in [
        "/realms/nope/login-actions/authenticate?session_code=x",
        "/realms/nope/login-actions/reset-credentials",
    ] {
        let (st, _, _) = get(&app, path, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (st, _, _) = post_form(&app, path, &[("username", "x")], None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
    // The session in the form wins over a stale one in the link.
    let (st, h, _) = post_form(
        &app,
        "/realms/dev/login-actions/authenticate?session_code=stale",
        &[
            ("csrf_token", &a.csrf),
            ("session_code", &a.session),
            ("username", MEMBER),
            ("password", MEMBER_PW),
        ],
        Some(&a.cookie),
    )
    .await;
    assert!(is_code(st, &h));
}

/// K1: Proved red against the code before K1, where `authenticate_page_get` and
/// `authenticate_page_post` never checked the `__Host-wk_auth` / `__Secure-wk_auth` cookie
/// against `auth_session.handle_hash`, allowing a stolen `session_code` from a URL/log to
/// continue someone else's login from a second browser.
#[tokio::test]
async fn k1_session_code_replayed_from_second_browser_is_refused() {
    let (app, _) = app().await;
    let first_browser = start(&app, "").await;
    let second_browser = start(&app, "").await;

    // 1. GET with no cookie or with second browser's cookie is refused with 400 expired page.
    let (st_no_ck, _, html_no_ck) = get(&app, &first_browser.url, None).await;
    assert_eq!(st_no_ck, StatusCode::BAD_REQUEST);
    assert_eq!(page_id(&html_no_ck), "expired");
    assert!(!html_no_ck.contains("csrf_token"));

    let (st_wrong_ck, _, html_wrong_ck) =
        get_with_cookie(&app, &first_browser.url, None, Some(&second_browser.cookie)).await;
    assert_eq!(st_wrong_ck, StatusCode::BAD_REQUEST);
    assert_eq!(page_id(&html_wrong_ck), "expired");

    // 2. POST with missing or second browser's cookie is refused with 400 expired page.
    let (st_post_no_ck, _, html_post_no_ck) = post_form(
        &app,
        &first_browser.url,
        &[
            ("csrf_token", &first_browser.csrf),
            ("session_code", &first_browser.session),
            ("username", MEMBER),
            ("password", MEMBER_PW),
        ],
        None,
    )
    .await;
    assert_eq!(st_post_no_ck, StatusCode::BAD_REQUEST);
    assert_eq!(page_id(&html_post_no_ck), "expired");

    let (st_post_wrong_ck, _, html_post_wrong_ck) = post_form(
        &app,
        &first_browser.url,
        &[
            ("csrf_token", &first_browser.csrf),
            ("session_code", &first_browser.session),
            ("username", MEMBER),
            ("password", MEMBER_PW),
        ],
        Some(&second_browser.cookie),
    )
    .await;
    assert_eq!(st_post_wrong_ck, StatusCode::BAD_REQUEST);
    assert_eq!(page_id(&html_post_wrong_ck), "expired");

    // 3. The original browser with the matching cookie still completes the login.
    let (st_ok, h_ok, _) = first_browser.password(&app, MEMBER, MEMBER_PW).await;
    assert!(is_code(st_ok, &h_ok));
}

#[tokio::test]
async fn refusals_look_the_same_and_legacy_hashes_migrate() {
    let (app, store) = app().await;
    let a = start(&app, "").await;
    let (st, _, html) = a.password(&app, "  ", "x").await;
    assert_eq!(st, StatusCode::OK);
    assert!(html.contains("alert"), "a missing username is said: {html}");
    let (_, _, unknown) = a.password(&app, "nobody@hel.fi", "x").await;
    let (_, _, wrong) = a.password(&app, MEMBER, "WrongPass123!").await;
    let mut disabled = user(&store, "demo.analyst@hel.fi").await;
    disabled.enabled = false;
    store.update_user(disabled).await.unwrap();
    let (_, _, off) = a
        .password(&app, "demo.analyst@hel.fi", "AnalystPass123!Valid")
        .await;
    // A person without a password credential.
    let mut nopw = user(&store, MEMBER).await;
    nopw.id = "nopw-1".into();
    nopw.username = "nopw@hel.fi".into();
    nopw.email = Some("nopw@hel.fi".into());
    store.create_user(nopw).await.unwrap();
    let (_, _, nocred) = a.password(&app, "nopw@hel.fi", "Whatever123!x").await;
    let alert = |html: &str| {
        let i = html.find("role=\"alert\"").expect("an alert");
        html[i..i + 200].to_owned()
    };
    for other in [&wrong, &off, &nocred] {
        assert_eq!(
            alert(&unknown),
            alert(other),
            "one answer for every refusal"
        );
    }

    // A Keycloak pbkdf2-sha256 hash logs in and is rehashed to argon2id.
    let m = user(&store, MEMBER).await;
    let salt = b"0123456789abcdef";
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2::<hmac::Hmac<sha2::Sha256>>(MEMBER_PW.as_bytes(), salt, 1000, &mut out).unwrap();
    store
        .upsert_credential(CredentialRecord {
            id: "legacy-pw".into(),
            realm_id: "dev".into(),
            user_id: m.id.clone(),
            credential_type: "password".into(),
            secret_data: serde_json::json!({"value": B64.encode(out), "salt": B64.encode(salt)})
                .to_string(),
            credential_data: r#"{"hashIterations":1000,"algorithm":"pbkdf2-sha256"}"#.into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let (st, h, _) = a.password(&app, MEMBER, MEMBER_PW).await;
    assert!(is_code(st, &h));
    let creds = store.list_credentials("dev", &m.id).await.unwrap();
    let pw = creds
        .iter()
        .find(|c| c.credential_type == "password")
        .unwrap();
    assert!(pw.secret_data.starts_with("$argon2id$"));
    assert!(pw.credential_data.is_empty());
}

async fn steward_totp(store: &MemoryStore, at: i64) -> String {
    let s = user(store, STEWARD).await;
    let creds = store.list_credentials("dev", &s.id).await.unwrap();
    let otp = creds.iter().find(|c| c.credential_type == "otp").unwrap();
    generate_totp_code_for_credential(otp, at as u64).unwrap()
}

#[tokio::test]
async fn totp_wrong_replayed_and_combined() {
    let (app, store) = app().await;
    let a = start(&app, "").await;
    let (st, _, html) = a.password(&app, STEWARD, STEWARD_PW).await;
    assert_eq!(st, StatusCode::OK);
    assert!(html.contains("name=\"otp\""), "{html}");
    let now = chrono::Utc::now().timestamp();
    let code = steward_totp(&store, now).await;
    let (st, h, html) = a.post(&app, &[("otp", &code)]).await;
    // The steward belongs to several organizations: the picker comes next.
    let picked = if st == StatusCode::OK {
        assert_eq!(page_id(&html), "org-choice", "{html}");
        let org = extract_input_value(&html, "org_id");
        let (st, _, _) = a.post(&app, &[("org_id", "not-a-member-org")]).await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "an organization outside the choices"
        );
        let (st, h, _) = a.post(&app, &[("org_id", &org)]).await;
        (st, h)
    } else {
        (st, h)
    };
    assert!(is_code(picked.0, &picked.1));

    // The same code again, in a new login with password and code in one POST: refused.
    let b = start(&app, "").await;
    let (st, _, html) = b
        .post(
            &app,
            &[
                ("username", STEWARD),
                ("password", STEWARD_PW),
                ("otp", &code),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        html.contains("role=\"alert\""),
        "a used code is refused: {html}"
    );
    let next = steward_totp(&store, now + 30).await;
    // A refused combined POST keeps nothing: password and code are sent again.
    let (st, _, html) = b
        .post(
            &app,
            &[
                ("username", STEWARD),
                ("password", STEWARD_PW),
                ("otp", &next),
            ],
        )
        .await;
    assert!(
        st == StatusCode::SEE_OTHER || page_id(&html) == "org-choice",
        "the next step's code passes: {st} {html}"
    );

    // Wrong and malformed codes are refused with the same answer.
    let c = start(&app, "").await;
    c.password(&app, STEWARD, STEWARD_PW).await;
    for wrong in ["000000", "not-a-code"] {
        let (st, _, html) = c.post(&app, &[("otp", wrong)]).await;
        assert_eq!(st, StatusCode::OK);
        assert!(
            html.contains("name=\"otp\"") && html.contains("role=\"alert\""),
            "{wrong}"
        );
    }
}

fn extract_input_value(html: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\"");
    let i = html.find(&marker).expect("input");
    let rest = &html[i..];
    let v = rest.find("value=\"").expect("value") + 7;
    rest[v..v + rest[v..].find('"').unwrap()].to_owned()
}

#[tokio::test]
async fn step_up_enrols_a_totp_credential() {
    let (app, store) = app().await;
    let a = start(&app, "&acr_values=mfa").await;
    let (st, _, html) = a.password(&app, MEMBER, MEMBER_PW).await;
    assert_eq!(st, StatusCode::OK);
    assert!(html.contains("name=\"otp\""));
    let session = store
        .get_auth_session_by_id("dev", &a.session)
        .await
        .unwrap()
        .unwrap();
    let secret = session
        .pending_totp_secret
        .clone()
        .expect("the page generated a secret");
    let grouped = |secret: &str| {
        secret
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert!(
        html.contains(&grouped(&secret)),
        "the secret is shown once, in groups of four"
    );
    let (_, _, html) = a.post(&app, &[("otp", "123456")]).await;
    // The wrong code is shown on the code field.
    assert!(html.contains("id=\"input-error-otp\""), "{html}");
    // Without a generated secret (page never rendered) the page is shown again, with one.
    let mut no_secret = session.clone();
    no_secret.pending_totp_secret = None;
    store.put_auth_session(no_secret).await.unwrap();
    let (st, _, html) = a.post(&app, &[("otp", "123456")]).await;
    assert_eq!(st, StatusCode::OK);
    let fresh = store
        .get_auth_session_by_id("dev", &a.session)
        .await
        .unwrap()
        .unwrap()
        .pending_totp_secret
        .unwrap();
    assert!(html.contains(&grouped(&fresh)));
    let cred = CredentialRecord {
        secret_data: serde_json::json!({ "value": fresh }).to_string(),
        credential_data:
            r#"{"digits":6,"period":30,"algorithm":"HmacSHA256","secretEncoding":"BASE32"}"#.into(),
        ..Default::default()
    };
    let code =
        generate_totp_code_for_credential(&cred, chrono::Utc::now().timestamp() as u64).unwrap();
    let (st, h, _) = a.post(&app, &[("otp", &code)]).await;
    assert!(is_code(st, &h));
    let m = user(&store, MEMBER).await;
    assert!(
        store
            .list_credentials("dev", &m.id)
            .await
            .unwrap()
            .iter()
            .any(|c| c.credential_type == "otp")
    );
}

#[tokio::test]
async fn temporary_password_terms_and_consent() {
    let (app, store) = app().await;
    let mut m = user(&store, MEMBER).await;
    m.required_actions = vec!["TERMS_AND_CONDITIONS".into()];
    store.update_user(m.clone()).await.unwrap();
    let creds = store.list_credentials("dev", &m.id).await.unwrap();
    let mut pw = creds
        .into_iter()
        .find(|c| c.credential_type == "password")
        .unwrap();
    pw.temporary = true;
    store.upsert_credential(pw).await.unwrap();
    let mut edge = store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .unwrap();
    edge.consent_required = true;
    store.upsert_client(edge).await.unwrap();

    let a = start(&app, "").await;
    let (st, _, html) = a.password(&app, MEMBER, MEMBER_PW).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page_id(&html), "terms", "{html}");
    let (st, _, html) = a.post(&app, &[]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page_id(&html), "update-password", "{html}");
    // The policy and the history apply: too short, contains the username, the old password.
    for bad in ["Short1!", "demo.orgmember@hel.fiX1!", MEMBER_PW] {
        let (st, _, html) = a.post(&app, &[("new_password", bad)]).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(page_id(&html), "update-password", "{bad}");
        assert!(html.contains("id=\"input-error-new_password\""), "{bad}");
    }
    let (st, _, html) = a.post(&app, &[("new_password", "BrandNewPass456!")]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page_id(&html), "consent", "{html}");
    let (st, h, _) = a.post(&app, &[]).await;
    assert!(is_code(st, &h));

    let m = user(&store, MEMBER).await;
    assert!(m.required_actions.is_empty());
    let creds = store.list_credentials("dev", &m.id).await.unwrap();
    let pw = creds
        .iter()
        .find(|c| c.credential_type == "password")
        .unwrap();
    assert!(!pw.temporary);
    assert!(
        verify_and_maybe_rehash("BrandNewPass456!", pw)
            .unwrap()
            .valid
    );
    assert!(
        store
            .list_consents("dev", &m.id)
            .await
            .unwrap()
            .iter()
            .any(|c| c.client_id == "edge")
    );
    // The next login goes straight through with the new password.
    let b = start(&app, "").await;
    let (st, h, _) = b.password(&app, MEMBER, "BrandNewPass456!").await;
    assert!(is_code(st, &h));
    let (_, _, html) = start(&app, "")
        .await
        .password(&app, MEMBER, MEMBER_PW)
        .await;
    assert!(
        html.contains("role=\"alert\""),
        "the temporary password is gone"
    );
}

#[tokio::test]
async fn a_state_without_a_form_action_is_refused() {
    let (app, store) = app().await;
    let a = start(&app, "").await;
    let mut s = store
        .get_auth_session_by_id("dev", &a.session)
        .await
        .unwrap()
        .unwrap();
    s.step_state_json = serde_json::to_string(&kaisatsu_login::LoginState::LoggedOut).unwrap();
    store.put_auth_session(s).await.unwrap();
    let (st, _, html) = get_with_cookie(&app, &a.url, None, Some(&a.cookie)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page_id(&html), "info");
    let (st, _, _) = a.post(&app, &[]).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    // A person deleted mid-login is not found at the next step.
    let a = start(&app, "&acr_values=mfa").await;
    a.password(&app, MEMBER, MEMBER_PW).await;
    let m = user(&store, MEMBER).await;
    store.delete_user("dev", None, &m.id).await.unwrap();
    let (st, _, _) = a.post(&app, &[("otp", "123456")]).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

/// K7, K8, K10, K11: Tests recovery code login (once ok, twice refused, wrong code counted),
/// verify-email step completion, combined password+OTP failure keeping the password step,
/// execute-actions-email + action-token consumption, and passkey registration + login.
#[tokio::test]
async fn k7_k8_k10_k11_recovery_codes_verify_email_combined_pw_otp_action_links_and_passkeys() {
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
    use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
    use kaisatsu_login::{OutboxDispatcher, RecordingSender};
    use serde_json::json;
    use sha2::{Digest, Sha256};

    let (app, store) = app().await;

    // --- K8: Failed combined password+OTP POST keeps the session on the password step ---
    let comb = start(&app, "").await;
    let (st_fail, _, html_fail) = comb
        .post(
            &app,
            &[
                ("username", STEWARD),
                ("password", STEWARD_PW),
                ("otp", "000000"),
            ],
        )
        .await;
    assert_eq!(st_fail, StatusCode::OK);
    // Page stays on login (password step) and never echoes the password.
    assert_eq!(page_id(&html_fail), "login", "{html_fail}");
    assert!(!html_fail.contains(STEWARD_PW));
    // Brute force counted the failure once.
    let steward_user = user(&store, STEWARD).await;
    let fail_rec = store
        .get_login_failure("dev", &format!("user:{}", steward_user.id))
        .await
        .unwrap()
        .expect("failure recorded");
    assert_eq!(fail_rec.num_failures, 1);

    // Next combined submit with the right OTP succeeds from the same step.
    let valid_otp = steward_totp(&store, chrono::Utc::now().timestamp()).await;
    let (st_next, h_next, html_next) = comb
        .post(
            &app,
            &[
                ("username", STEWARD),
                ("password", STEWARD_PW),
                ("otp", &valid_otp),
            ],
        )
        .await;
    // Signed in: the code redirect, or the organization picker when several apply.
    assert!(
        is_code(st_next, &h_next) || page_id(&html_next) == "org-choice",
        "{st_next} {html_next}"
    );

    // --- K7: Recovery code step (once ok, twice refused, wrong code counted) ---
    let (plain_codes, code_hashes) = generate_recovery_codes();
    store
        .upsert_credential(CredentialRecord {
            id: "steward-rec-codes".into(),
            realm_id: "dev".into(),
            user_id: steward_user.id.clone(),
            credential_type: "recovery-authn-codes".into(),
            user_label: Some("Recovery codes".into()),
            created_date: chrono::Utc::now().timestamp_millis(),
            secret_data: json!({ "hashes": code_hashes }).to_string(),
            credential_data: json!({ "remaining": 10 }).to_string(),
            temporary: false,
        })
        .await
        .unwrap();

    let rec_attempt = start(&app, "").await;
    rec_attempt.password(&app, STEWARD, STEWARD_PW).await;
    // Switch from SecondFactorOtp to RecoveryCode
    let (st_sw, _, html_sw) = rec_attempt
        .post(&app, &[("switch_to", "recovery_code")])
        .await;
    assert_eq!(st_sw, StatusCode::OK);
    assert_eq!(page_id(&html_sw), "recovery-code");

    // Wrong recovery code is refused and counted
    let (st_bad_rc, _, html_bad_rc) = rec_attempt
        .post(&app, &[("recovery_code", "WRONG-CODE1")])
        .await;
    assert_eq!(st_bad_rc, StatusCode::OK);
    assert!(html_bad_rc.contains("role=\"alert\""));
    let failures_after_wrong = store
        .get_login_failure("dev", &format!("user:{}", steward_user.id))
        .await
        .unwrap()
        .unwrap()
        .num_failures;
    assert!(failures_after_wrong >= 1);

    // Valid recovery code works once
    let first_code = &plain_codes[0];
    let (st_ok_rc, h_ok_rc, html_ok_rc) = rec_attempt
        .post(&app, &[("recovery_code", first_code)])
        .await;
    assert!(
        is_code(st_ok_rc, &h_ok_rc) || page_id(&html_ok_rc) == "org-choice",
        "{st_ok_rc} {html_ok_rc}"
    );

    // Using the same recovery code a second time is refused
    let rec_attempt2 = start(&app, "").await;
    rec_attempt2.password(&app, STEWARD, STEWARD_PW).await;
    rec_attempt2
        .post(&app, &[("switch_to", "recovery_code")])
        .await;
    let (_, _, html_reuse_rc) = rec_attempt2
        .post(&app, &[("recovery_code", first_code)])
        .await;
    assert_eq!(page_id(&html_reuse_rc), "recovery-code");
    assert!(html_reuse_rc.contains("role=\"alert\""));

    // --- K7: VerifyEmail step ---
    let mut member_u = user(&store, MEMBER).await;
    member_u.email_verified = false;
    member_u.required_actions = vec!["VERIFY_EMAIL".into()];
    store.update_user(member_u.clone()).await.unwrap();

    let ve_attempt = start(&app, "").await;
    let (st_ve, _, html_ve) = ve_attempt.password(&app, MEMBER, MEMBER_PW).await;
    assert_eq!(st_ve, StatusCode::OK);
    assert_eq!(page_id(&html_ve), "verify-email");
    // Outbox has the VERIFY_EMAIL row queued
    let store_dyn: Arc<dyn Store> = store.clone();
    assert!(
        store_dyn
            .list_outbox_rows("dev", None)
            .await
            .unwrap()
            .iter()
            .any(|r| r.template_kind == "VERIFY_EMAIL")
    );
    // The fixed code of the first write ("654321") no longer verifies anything.
    let (st_ve_bad, _, html_ve_bad) = ve_attempt.post(&app, &[("email_code", "654321")]).await;
    assert_eq!(st_ve_bad, StatusCode::OK);
    assert_eq!(page_id(&html_ve_bad), "verify-email");
    assert!(!user(&store, MEMBER).await.email_verified);
    // The mailed action link verifies the address; the next POST of the step completes it.
    let ve_sink = RecordingSender::new();
    OutboxDispatcher::new(
        store_dyn.clone(),
        store.kek().clone(),
        Arc::new(ve_sink.clone()),
    )
    .dispatch_realm_batch(&mount(), "dev", 50)
    .await
    .unwrap();
    let ve_mail = ve_sink.drain().pop().unwrap();
    let ve_link = ve_mail
        .action_link
        .strip_prefix("https://idm.dev.example.test")
        .unwrap()
        .to_owned();
    let (st_link, _, _) = get(&app, &ve_link, None).await;
    assert!(
        st_link.is_success() || st_link == StatusCode::SEE_OTHER,
        "{st_link}"
    );
    let (st_ve_done, h_ve_done, _) = ve_attempt.post(&app, &[("email_code", "")]).await;
    assert!(is_code(st_ve_done, &h_ve_done));
    let verified_member = user(&store, MEMBER).await;
    assert!(verified_member.email_verified);
    assert!(
        !verified_member
            .required_actions
            .contains(&"VERIFY_EMAIL".into())
    );

    // --- K9 & K10: Reset credentials queues outbox row, dispatcher sends link, action-token works once ---
    let sink = RecordingSender::new();
    let dispatcher = OutboxDispatcher::new(
        store_dyn.clone(),
        store.kek().clone(),
        Arc::new(sink.clone()),
    );
    let _ = dispatcher
        .dispatch_realm_batch(&mount(), "dev", 50)
        .await
        .unwrap();
    let _ = sink.drain();

    // Unknown user on reset-credentials queues nothing
    let (_, _, _) = post_form(
        &app,
        "/realms/dev/login-actions/reset-credentials",
        &[("username", "unknown-person@hel.fi")],
        None,
    )
    .await;
    assert_eq!(
        dispatcher
            .dispatch_realm_batch(&mount(), "dev", 50)
            .await
            .unwrap(),
        0
    );

    // Known user on reset-credentials queues one mail
    let (_, _, _) = post_form(
        &app,
        "/realms/dev/login-actions/reset-credentials",
        &[("username", MEMBER)],
        None,
    )
    .await;
    assert_eq!(
        dispatcher
            .dispatch_realm_batch(&mount(), "dev", 50)
            .await
            .unwrap(),
        1
    );
    let mails = sink.drain();
    assert_eq!(mails.len(), 1);
    assert!(!mails[0].body.contains(MEMBER_PW));
    let link_path = mails[0]
        .action_link
        .strip_prefix("https://idm.dev.example.test")
        .unwrap();

    // Consume action-token once -> renders password form + sets auth cookie
    let (st_at, h_at, html_at) = get(&app, link_path, None).await;
    assert_eq!(st_at, StatusCode::OK);
    assert_eq!(page_id(&html_at), "action-token-form");
    let at_cookie = h_at[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let at_csrf = extract_hidden_input(&html_at, "csrf_token");
    let at_session = extract_hidden_input(&html_at, "session_code");

    // Second use of the same action-token link is refused
    let (st_at_replay, _, html_at_replay) = get(&app, link_path, None).await;
    assert_eq!(st_at_replay, StatusCode::BAD_REQUEST);
    assert_eq!(page_id(&html_at_replay), "expired");

    // Submit the new password on the action-token form
    let (st_at_post, _, html_at_post) = post_form(
        &app,
        "/realms/dev/login-actions/action-token",
        &[
            ("csrf_token", &at_csrf),
            ("session_code", &at_session),
            ("new_password", "ResetByLinkPass789!"),
        ],
        Some(&at_cookie),
    )
    .await;
    assert_eq!(st_at_post, StatusCode::OK);
    assert_eq!(page_id(&html_at_post), "account-updated");

    // --- K11: Passkey registration (`webauthn-register`) then passwordless login (`LoginState::Passkey`) ---
    let mut member_u = user(&store, MEMBER).await;
    member_u.required_actions = vec!["webauthn-register".into()];
    store.update_user(member_u).await.unwrap();

    let pk_attempt = start(&app, "").await;
    let (st_pk_reg_page, _, html_pk_reg) = pk_attempt
        .password(&app, MEMBER, "ResetByLinkPass789!")
        .await;
    assert_eq!(st_pk_reg_page, StatusCode::OK);
    assert_eq!(page_id(&html_pk_reg), "webauthn-register");

    let reg_session = store
        .get_auth_session_by_id("dev", &pk_attempt.session)
        .await
        .unwrap()
        .unwrap();
    let reg_challenge = reg_session.webauthn_challenge.unwrap();

    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref()).unwrap();
    let pub_bytes = pair.public_key().as_ref();
    let x = &pub_bytes[1..33];
    let y = &pub_bytes[33..65];
    let cred_id = b"passkey-browser-cred-1";
    let cred_id_b64 = URL_SAFE_NO_PAD.encode(cred_id);

    // Build COSE_Key and attestationObject CBOR
    let mut cose = vec![0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21, 0x58, 0x20];
    cose.extend_from_slice(x);
    cose.extend_from_slice(&[0x22, 0x58, 0x20]);
    cose.extend_from_slice(y);

    let rp_id = "idm.dev.example.test";
    let mut auth_data = Vec::new();
    auth_data.extend_from_slice(&Sha256::digest(rp_id.as_bytes()));
    auth_data.push(
        kaisatsu_crypto::AUTH_DATA_FLAG_UP
            | kaisatsu_crypto::AUTH_DATA_FLAG_UV
            | kaisatsu_crypto::AUTH_DATA_FLAG_AT,
    );
    auth_data.extend_from_slice(&1u32.to_be_bytes());
    auth_data.extend_from_slice(&[0u8; 16]);
    auth_data.extend_from_slice(&(cred_id.len() as u16).to_be_bytes());
    auth_data.extend_from_slice(cred_id);
    auth_data.extend_from_slice(&cose);

    let mut att_obj = vec![0xa3, 0x63, b'f', b'm', b't', 0x64, b'n', b'o', b'n', b'e'];
    att_obj.extend_from_slice(&[0x67, b'a', b't', b't', b'S', b't', b'm', b't', 0xa0]);
    att_obj.extend_from_slice(&[
        0x68,
        b'a',
        b'u',
        b't',
        b'h',
        b'D',
        b'a',
        b't',
        b'a',
        0x58,
        auth_data.len() as u8,
    ]);
    att_obj.extend_from_slice(&auth_data);

    let cdata_create = URL_SAFE_NO_PAD.encode(
        json!({
            "type": "webauthn.create",
            "challenge": reg_challenge,
            "origin": "https://idm.dev.example.test"
        })
        .to_string(),
    );
    let reg_json = json!({
        "id": cred_id_b64,
        "rawId": cred_id_b64,
        "type": "public-key",
        "response": {
            "clientDataJSON": cdata_create,
            "attestationObject": URL_SAFE_NO_PAD.encode(&att_obj),
            "transports": ["internal"]
        }
    })
    .to_string();

    let (st_reg_ok, h_reg_ok, html_reg_ok) = pk_attempt
        .post(&app, &[("webauthn_register_response", &reg_json)])
        .await;
    assert!(is_code(st_reg_ok, &h_reg_ok), "{st_reg_ok} {html_reg_ok}");

    // Now sign in passwordlessly using the registered passkey!
    let pk_login = start(&app, "").await;
    let (st_sw_pk, _, html_sw_pk) = pk_login.post(&app, &[("switch_to", "passkey")]).await;
    assert_eq!(st_sw_pk, StatusCode::OK);
    assert_eq!(page_id(&html_sw_pk), "webauthn-authenticate");

    let login_session = store
        .get_auth_session_by_id("dev", &pk_login.session)
        .await
        .unwrap()
        .unwrap();
    let login_challenge = login_session.webauthn_challenge.unwrap();
    let member_after = user(&store, MEMBER).await;
    let user_handle = member_after.attributes["webauthn.user.handle"][0].clone();

    let cdata_get_bytes = json!({
        "type": "webauthn.get",
        "challenge": login_challenge,
        "origin": "https://idm.dev.example.test"
    })
    .to_string()
    .into_bytes();
    let mut ad_get = Vec::new();
    ad_get.extend_from_slice(&Sha256::digest(rp_id.as_bytes()));
    ad_get.push(kaisatsu_crypto::AUTH_DATA_FLAG_UP | kaisatsu_crypto::AUTH_DATA_FLAG_UV);
    ad_get.extend_from_slice(&2u32.to_be_bytes());
    let mut signed = ad_get.clone();
    signed.extend_from_slice(&Sha256::digest(&cdata_get_bytes));
    let sig = pair.sign(&rng, &signed).unwrap();

    let assertion_json = json!({
        "id": cred_id_b64,
        "rawId": cred_id_b64,
        "type": "public-key",
        "response": {
            "clientDataJSON": URL_SAFE_NO_PAD.encode(&cdata_get_bytes),
            "authenticatorData": URL_SAFE_NO_PAD.encode(&ad_get),
            "signature": URL_SAFE_NO_PAD.encode(sig.as_ref()),
            "userHandle": user_handle
        }
    })
    .to_string();

    let (st_pk_ok, h_pk_ok, _) = pk_login
        .post(&app, &[("webauthn_assertion_response", &assertion_json)])
        .await;
    assert!(is_code(st_pk_ok, &h_pk_ok));
}

#[tokio::test]
async fn reset_credentials_never_says_whether_the_account_exists() {
    let (app, _) = app().await;
    let path = "/realms/dev/login-actions/reset-credentials";
    let (st, h, html) = get(&app, path, Some("de")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page_id(&html), "reset-credentials");
    assert!(html.contains("lang=\"de\""));
    assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    let (_, _, known) = post_form(&app, path, &[("username", MEMBER)], None).await;
    let (_, _, unknown) = post_form(&app, path, &[("username", "nobody@hel.fi")], None).await;
    assert_eq!(page_id(&known), "reset-credentials-sent");
    let strip = |h: &str| {
        let i = h.find("nonce-").unwrap_or(0);
        h.replace(&h[i..(i + 22).min(h.len())], "")
    };
    assert_eq!(strip(&known).len(), strip(&unknown).len());
}

/// K7: a recovery code whose single use is already recorded (a concurrent submit that won the
/// race) is refused even while the stored list still holds its hash. Red before: the stored list
/// alone decided, so two parallel submits of one code both signed in.
#[tokio::test]
async fn k7_recovery_code_single_use_holds_under_a_concurrent_submit() {
    let (app, store) = app().await;
    let steward = user(&store, STEWARD).await;
    let (plain, hashes) = generate_recovery_codes();
    store
        .upsert_credential(CredentialRecord {
            id: "steward-rec-race".into(),
            realm_id: "dev".into(),
            user_id: steward.id.clone(),
            credential_type: "recovery-authn-codes".into(),
            user_label: None,
            created_date: chrono::Utc::now().timestamp_millis(),
            secret_data: serde_json::json!({ "hashes": hashes }).to_string(),
            credential_data: serde_json::json!({ "remaining": 10 }).to_string(),
            temporary: false,
        })
        .await
        .unwrap();
    // The other request recorded the use of code 0 but has not rewritten the list yet.
    store
        .mark_token_revoked(RevokedTokenRecord {
            jti_hash: sha256_hex(&format!("recovery:{}:{}", steward.id, hashes[0])),
            realm_id: "dev".into(),
            expires_at: chrono::Utc::now() + chrono::Duration::days(1),
        })
        .await
        .unwrap();
    let a = start(&app, "").await;
    a.password(&app, STEWARD, STEWARD_PW).await;
    a.post(&app, &[("switch_to", "recovery_code")]).await;
    let (st, _, html) = a.post(&app, &[("recovery_code", &plain[0])]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(page_id(&html), "recovery-code", "{html}");
    assert!(html.contains("role=\"alert\""));
    // An unused code still passes.
    let (st, h, html) = a.post(&app, &[("recovery_code", &plain[1])]).await;
    assert!(
        is_code(st, &h) || page_id(&html) == "org-choice",
        "{st} {html}"
    );
}
