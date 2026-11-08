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
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some(l) = lang {
        b = b.header(header::ACCEPT_LANGUAGE, l);
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

/// One login attempt: the authorization request's login page URL, its CSRF token and session.
struct Attempt {
    url: String,
    csrf: String,
    session: String,
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
    let (st, _, html) = get(app, &url, None).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    Attempt {
        csrf: extract_hidden_input(&html, "csrf_token"),
        session: extract_hidden_input(&html, "session_code"),
        url,
    }
}

impl Attempt {
    async fn post(&self, app: &Router, fields: &[(&str, &str)]) -> (StatusCode, HeaderMap, String) {
        let mut all = vec![
            ("csrf_token", self.csrf.as_str()),
            ("session_code", self.session.as_str()),
        ];
        all.extend_from_slice(fields);
        post_form(app, &self.url, &all, None).await
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
    let (_, _, html) = get(&app, &foreign, None).await;
    assert_eq!(page_id(&html), "expired");
    let (st, _, _) = post_form(
        &app,
        &foreign,
        &[
            ("csrf_token", &a.csrf),
            ("username", MEMBER),
            ("password", MEMBER_PW),
        ],
        None,
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
            None,
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
        None,
    )
    .await;
    assert!(is_code(st, &h));
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
    assert!(html.contains(&secret), "the secret is shown once");
    let (_, _, html) = a.post(&app, &[("otp", "123456")]).await;
    assert!(html.contains("role=\"alert\""));
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
    assert!(html.contains(&fresh));
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
        assert!(html.contains("role=\"alert\""), "{bad}");
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
    let (st, _, html) = get(&app, &a.url, None).await;
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
