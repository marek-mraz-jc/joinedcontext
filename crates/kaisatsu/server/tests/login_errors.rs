#![forbid(unsafe_code)]
//! Error paths and less common branches of the login pages (W7, K7-K11): a person removed in the
//! middle of a login, malformed WebAuthn JSON, the action-token route (unknown realm, disabled
//! person, form actions), locale negotiation, the e-mail sender and outbox dispatcher, and the
//! passkey ceremony helpers called directly.

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::{Extension, Router};
use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_login::*;
use kaisatsu_server::router;
use kaisatsu_store_memory::MemoryStore;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

mod common;
use common::*;

const AUTH: &str = "/realms/dev/protocol/openid-connect/auth";
const ACTION: &str = "/realms/dev/login-actions/action-token";
const ISSUER: &str = "https://idm.dev.example.test/realms/dev";
const CALLBACK: &str = "https://portal.dev.example.test/callback";
const VERIFIER: &str = "pkce-verifier-string-with-at-least-43-characters-long-012345";
const MEMBER: &str = "demo.orgmember@hel.fi";
const MEMBER_PW: &str = "MemberPass123!Valid";
const STEWARD: &str = "demo.steward@hel.fi";
const STEWARD_PW: &str = "StewardPass123!Valid";
const NEW_PW: &str = "FreshMemberPass456!";

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

async fn app() -> (Router, Arc<MemoryStore>) {
    let (state, store, _) = setup_dev_realm_state().await;
    (router(state, mount()).layer(Extension(mount())), store)
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    form: Option<&[(&str, &str)]>,
    cookie: Option<&str>,
    lang: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    if let Some(l) = lang {
        b = b.header(header::ACCEPT_LANGUAGE, l);
    }
    let body = match form {
        Some(pairs) => {
            b = b.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            Body::from(
                url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(pairs)
                    .finish(),
            )
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let st = resp.status();
    let h = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, h, String::from_utf8_lossy(&bytes).into_owned())
}

fn page_id(html: &str) -> String {
    let marker = "data-page-id=\"login-";
    html.find(marker)
        .map(|i| {
            let rest = &html[i + marker.len()..];
            rest[..rest.find('"').unwrap()].to_owned()
        })
        .unwrap_or_default()
}

fn first_cookie(h: &HeaderMap) -> String {
    h[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

struct Attempt {
    url: String,
    csrf: String,
    session: String,
    cookie: String,
}

async fn start(app: &Router, lang: Option<&str>) -> Attempt {
    let uri = format!(
        "{AUTH}?client_id=edge&redirect_uri={}&response_type=code&scope=openid&state=st&code_challenge={}&code_challenge_method=S256",
        url::form_urlencoded::byte_serialize(CALLBACK.as_bytes()).collect::<String>(),
        sha256_base64url(VERIFIER),
    );
    let (st, h, _) = send(app, Method::GET, &uri, None, None, None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let url = h[header::LOCATION].to_str().unwrap().to_owned();
    let cookie = first_cookie(&h);
    let (st, _, html) = send(app, Method::GET, &url, None, Some(&cookie), lang).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    if let Some(l) = lang {
        assert!(html.contains(&format!("lang=\"{l}\"")), "{html}");
    }
    Attempt {
        csrf: extract_hidden_input(&html, "csrf_token"),
        session: extract_hidden_input(&html, "session_code"),
        url,
        cookie,
    }
}

impl Attempt {
    async fn post(&self, app: &Router, fields: &[(&str, &str)]) -> (StatusCode, HeaderMap, String) {
        self.post_lang(app, fields, None).await
    }

    async fn post_lang(
        &self,
        app: &Router,
        fields: &[(&str, &str)],
        lang: Option<&str>,
    ) -> (StatusCode, HeaderMap, String) {
        let mut all = vec![
            ("csrf_token", self.csrf.as_str()),
            ("session_code", self.session.as_str()),
        ];
        all.extend_from_slice(fields);
        send(
            app,
            Method::POST,
            &self.url,
            Some(&all),
            Some(&self.cookie),
            lang,
        )
        .await
    }
}

fn is_code(st: StatusCode, h: &HeaderMap) -> bool {
    st == StatusCode::SEE_OTHER && h[header::LOCATION].to_str().unwrap().starts_with(CALLBACK)
}

async fn user(store: &MemoryStore, name: &str) -> UserRepresentation {
    store
        .get_user_by_username_or_email("dev", None, name)
        .await
        .unwrap()
        .unwrap()
}

async fn with_actions(store: &MemoryStore, name: &str, actions: &[&str]) -> UserRepresentation {
    let mut u = user(store, name).await;
    u.required_actions = actions.iter().map(|a| (*a).to_owned()).collect();
    store.update_user(u).await.unwrap()
}

async fn steward_totp(store: &MemoryStore) -> String {
    let s = user(store, STEWARD).await;
    let creds = store.list_credentials("dev", &s.id).await.unwrap();
    let otp = creds.iter().find(|c| c.credential_type == "otp").unwrap();
    generate_totp_code_for_credential(otp, Utc::now().timestamp() as u64).unwrap()
}

async fn remove(store: &MemoryStore, u: &UserRepresentation) {
    assert!(store.delete_user("dev", None, &u.id).await.unwrap());
}

/// The login page, a refused POST, the reset page and the action-token page all answer in the
/// language the browser asks for; the client's display name is translated on the page.
#[tokio::test]
async fn pages_follow_accept_language_and_the_client_display_name() {
    let (app, store) = app().await;
    let mut edge = store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .unwrap();
    edge.name = Some("Edge Portal".into());
    store.upsert_client(edge).await.unwrap();

    let a = start(&app, Some("sk")).await;
    let (st, _, html) = a
        .post_lang(
            &app,
            &[("username", MEMBER), ("password", "WrongPass123!")],
            Some("cs"),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(html.contains("role=\"alert\""), "{html}");
    assert!(html.contains("Edge Portal"), "{html}");

    let (st, _, html) = send(
        &app,
        Method::POST,
        "/realms/dev/login-actions/reset-credentials",
        Some(&[("username", MEMBER)]),
        None,
        Some("hu"),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(html.contains("lang=\"hu\""), "{html}");

    let (st, _, html) = send(
        &app,
        Method::GET,
        &format!("{ACTION}?key=not-a-token"),
        None,
        None,
        Some("de"),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(page_id(&html), "expired");
}

/// Organization memberships feed the picker, and an organization pinned on the session that
/// the person belongs to survives (K2); the login still ends in a code or the picker.
#[tokio::test]
async fn memberships_and_a_pinned_session_organization_shape_the_login() {
    let (app, store) = app().await;
    let member = user(&store, MEMBER).await;
    store
        .upsert_membership(Membership {
            id: "m-member-hel".into(),
            realm_id: "dev".into(),
            org_id: "helsinki".into(),
            person_id: member.id.clone(),
            org_roles: Vec::new(),
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    let a = start(&app, None).await;
    let mut session = store
        .get_auth_session_by_id("dev", &a.session)
        .await
        .unwrap()
        .unwrap();
    session.org_id = Some("helsinki".into());
    store.put_auth_session(session).await.unwrap();
    let (st, h, html) = a
        .post(&app, &[("username", MEMBER), ("password", MEMBER_PW)])
        .await;
    assert!(
        is_code(st, &h) || page_id(&html) == "org-choice",
        "{st} {html}"
    );
}

/// A person deleted while their login waits on a step is refused at that step (401), never
/// signed in as a ghost.
#[tokio::test]
async fn a_person_removed_mid_login_is_refused_at_each_required_action_step() {
    for (action, page) in [
        ("UPDATE_PASSWORD", "update-password"),
        ("TERMS_AND_CONDITIONS", "terms"),
        ("VERIFY_EMAIL", "verify-email"),
        ("webauthn-register", "webauthn-register"),
    ] {
        let (app, store) = app().await;
        let member = with_actions(&store, MEMBER, &[action]).await;
        let a = start(&app, None).await;
        let (st, _, html) = a
            .post(&app, &[("username", MEMBER), ("password", MEMBER_PW)])
            .await;
        assert_eq!(st, StatusCode::OK, "{action}: {html}");
        assert_eq!(page_id(&html), page, "{action}");
        remove(&store, &member).await;
        let (st, _, _) = a
            .post(
                &app,
                &[
                    ("new_password", NEW_PW),
                    ("accept", "on"),
                    ("email_code", "x"),
                    ("webauthn_register_response", "{}"),
                ],
            )
            .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{action}");
    }
}

#[tokio::test]
async fn a_person_removed_at_consent_recovery_or_organization_choice_is_refused() {
    // Consent.
    let (app, store) = app().await;
    let mut edge = store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .unwrap();
    edge.consent_required = true;
    store.upsert_client(edge).await.unwrap();
    let a = start(&app, None).await;
    let (_, _, html) = a
        .post(&app, &[("username", MEMBER), ("password", MEMBER_PW)])
        .await;
    assert_eq!(page_id(&html), "consent", "{html}");
    remove(&store, &user(&store, MEMBER).await).await;
    assert_eq!(a.post(&app, &[]).await.0, StatusCode::UNAUTHORIZED);

    // Recovery code.
    let (app, store) = crate::app().await;
    let a = start(&app, None).await;
    a.post(&app, &[("username", STEWARD), ("password", STEWARD_PW)])
        .await;
    let (_, _, html) = a.post(&app, &[("switch_to", "recovery_code")]).await;
    assert_eq!(page_id(&html), "recovery-code", "{html}");
    remove(&store, &user(&store, STEWARD).await).await;
    let (st, _, _) = a.post(&app, &[("recovery_code", "ABCD-EFGH")]).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Organization choice (the steward belongs to several organizations).
    let (app, store) = crate::app().await;
    let a = start(&app, None).await;
    a.post(&app, &[("username", STEWARD), ("password", STEWARD_PW)])
        .await;
    let code = steward_totp(&store).await;
    let (st, h, html) = a.post(&app, &[("otp", &code)]).await;
    if page_id(&html) == "org-choice" {
        let org = extract_hidden_input(&html, "org_id");
        remove(&store, &user(&store, STEWARD).await).await;
        let (st, _, _) = a.post(&app, &[("org_id", &org)]).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    } else {
        assert!(is_code(st, &h), "{st} {html}");
    }
}

/// The UPDATE_PASSWORD required action is cleared once the new password is accepted.
#[tokio::test]
async fn update_password_required_action_is_cleared_after_a_good_password() {
    let (app, store) = app().await;
    with_actions(&store, MEMBER, &["UPDATE_PASSWORD"]).await;
    let a = start(&app, None).await;
    let (_, _, html) = a
        .post(&app, &[("username", MEMBER), ("password", MEMBER_PW)])
        .await;
    assert_eq!(page_id(&html), "update-password");
    let (st, h, html) = a.post(&app, &[("new_password", NEW_PW)]).await;
    assert!(is_code(st, &h), "{st} {html}");
    assert!(user(&store, MEMBER).await.required_actions.is_empty());
}

/// An OTP step whose credential vanished (removed by an admin meanwhile) is refused.
#[tokio::test]
async fn the_otp_step_without_an_otp_credential_is_refused() {
    let (app, store) = app().await;
    let a = start(&app, None).await;
    let (_, _, html) = a
        .post(&app, &[("username", STEWARD), ("password", STEWARD_PW)])
        .await;
    assert!(html.contains("name=\"otp\""), "{html}");
    let steward = user(&store, STEWARD).await;
    for c in store.list_credentials("dev", &steward.id).await.unwrap() {
        if c.credential_type == "otp" {
            store
                .delete_credential("dev", &steward.id, &c.id)
                .await
                .unwrap();
        }
    }
    let (st, _, _) = a.post(&app, &[("otp", "123456")]).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

/// Malformed WebAuthn JSON is refused with 400 at the passkey login and at registration.
#[tokio::test]
async fn malformed_webauthn_json_is_refused() {
    let (app, store) = app().await;
    let a = start(&app, None).await;
    let (_, _, html) = a.post(&app, &[("switch_to", "passkey")]).await;
    assert_eq!(page_id(&html), "webauthn-authenticate");
    let (st, _, _) = a
        .post(&app, &[("webauthn_assertion_response", "{not json")])
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    with_actions(&store, MEMBER, &["webauthn-register"]).await;
    let b = start(&app, None).await;
    let (_, _, html) = b
        .post(&app, &[("username", MEMBER), ("password", MEMBER_PW)])
        .await;
    assert_eq!(page_id(&html), "webauthn-register");
    let (st, _, _) = b
        .post(&app, &[("webauthn_register_response", "{not json")])
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

async fn mint(store: &Arc<MemoryStore>, user_id: &str, actions: &[&str]) -> String {
    let dynstore: Arc<dyn Store> = store.clone();
    let token = EmailSender::mint_action_token(
        &dynstore,
        store.kek(),
        "dev",
        ISSUER,
        user_id,
        actions.iter().map(|a| (*a).to_owned()).collect(),
        Some("edge".into()),
        Some(CALLBACK.into()),
        None,
    )
    .await
    .unwrap();
    format!(
        "{ACTION}?key={}",
        url::form_urlencoded::byte_serialize(token.as_bytes()).collect::<String>()
    )
}

/// Action links: an unknown realm is 404; VERIFY_EMAIL and TERMS apply at once and redirect to
/// the validated URI; a disabled person's link is refused.
#[tokio::test]
async fn action_links_apply_direct_actions_and_refuse_disabled_people() {
    let (app, store) = app().await;
    let (st, _, _) = send(
        &app,
        Method::GET,
        "/realms/nope/login-actions/action-token?key=x",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = send(
        &app,
        Method::POST,
        "/realms/nope/login-actions/action-token",
        Some(&[("session_code", "x")]),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let mut member = with_actions(&store, MEMBER, &["VERIFY_EMAIL", "TERMS_AND_CONDITIONS"]).await;
    member.email_verified = false;
    let member = store.update_user(member).await.unwrap();
    let link = mint(
        &store,
        &member.id,
        &["VERIFY_EMAIL", "TERMS_AND_CONDITIONS"],
    )
    .await;
    let (st, h, _) = send(&app, Method::GET, &link, None, None, None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(h[header::LOCATION], CALLBACK);
    let after = user(&store, MEMBER).await;
    assert!(after.email_verified);
    assert!(after.required_actions.is_empty());

    let link = mint(&store, &member.id, &["VERIFY_EMAIL"]).await;
    let mut disabled = user(&store, MEMBER).await;
    disabled.enabled = false;
    store.update_user(disabled).await.unwrap();
    let (st, _, _) = send(&app, Method::GET, &link, None, None, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

/// An UPDATE_PASSWORD + CONFIGURE_TOTP link renders one form; a wrong code is refused, the
/// right one stores the authenticator; a form whose person or pending secret vanished is refused.
#[tokio::test]
async fn action_link_forms_need_a_valid_code_a_person_and_a_pending_secret() {
    let (app, store) = app().await;
    let member = user(&store, MEMBER).await;
    let open = |link: String| {
        let app = app.clone();
        async move {
            let (st, h, html) = send(&app, Method::GET, &link, None, None, None).await;
            assert_eq!(st, StatusCode::OK, "{html}");
            assert_eq!(page_id(&html), "action-token-form");
            (
                first_cookie(&h),
                extract_hidden_input(&html, "csrf_token"),
                extract_hidden_input(&html, "session_code"),
            )
        }
    };
    let link = mint(&store, &member.id, &["UPDATE_PASSWORD", "CONFIGURE_TOTP"]).await;
    let (cookie, csrf, session) = open(link).await;
    let submit = |otp: String| {
        let (app, cookie, csrf, session) =
            (app.clone(), cookie.clone(), csrf.clone(), session.clone());
        async move {
            send(
                &app,
                Method::POST,
                ACTION,
                Some(&[
                    ("csrf_token", csrf.as_str()),
                    ("session_code", session.as_str()),
                    ("new_password", NEW_PW),
                    ("otp", otp.as_str()),
                ]),
                Some(&cookie),
                None,
            )
            .await
        }
    };
    assert_eq!(submit("000000".into()).await.0, StatusCode::BAD_REQUEST);
    let secret = store
        .get_auth_session_by_id("dev", &session)
        .await
        .unwrap()
        .unwrap()
        .pending_totp_secret
        .unwrap();
    let realm = store.get_realm("dev").await.unwrap().unwrap();
    let probe = CredentialRecord {
        credential_type: "otp".into(),
        secret_data: json!({ "value": secret }).to_string(),
        credential_data:
            json!({ "digits": realm.otp_policy_digits, "period": realm.otp_policy_period,
            "algorithm": realm.otp_policy_algorithm, "secretEncoding": "BASE32" })
            .to_string(),
        ..Default::default()
    };
    let code = generate_totp_code_for_credential(&probe, Utc::now().timestamp() as u64).unwrap();
    let (st, h, _) = submit(code).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(h[header::LOCATION], CALLBACK);
    let creds = store.list_credentials("dev", &member.id).await.unwrap();
    assert!(creds.iter().any(|c| c.credential_type == "otp"));

    // A TOTP form whose pending secret is gone.
    let link = mint(&store, &member.id, &["CONFIGURE_TOTP"]).await;
    let (cookie, csrf, session) = open(link).await;
    let mut s = store
        .get_auth_session_by_id("dev", &session)
        .await
        .unwrap()
        .unwrap();
    s.pending_totp_secret = None;
    store.put_auth_session(s).await.unwrap();
    let form = [
        ("csrf_token", csrf.as_str()),
        ("session_code", session.as_str()),
        ("otp", "123456"),
    ];
    let (st, _, _) = send(&app, Method::POST, ACTION, Some(&form), Some(&cookie), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // A password form whose person was deleted meanwhile.
    let link = mint(&store, &member.id, &["UPDATE_PASSWORD"]).await;
    let (cookie, csrf, session) = open(link).await;
    remove(&store, &member).await;
    let form = [
        ("csrf_token", csrf.as_str()),
        ("session_code", session.as_str()),
        ("new_password", NEW_PW),
    ];
    let (st, _, _) = send(&app, Method::POST, ACTION, Some(&form), Some(&cookie), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

fn smtp_sender(host: &str, port: &str) -> EmailSender {
    let dir = std::env::temp_dir().join(format!("wk-login-errors-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("smtp-word");
    std::fs::write(&file, "smtp-word-for-tests\n").unwrap();
    let sender = EmailSender::from_secret_mount(
        SmtpServerConfig {
            host: host.into(),
            from: "idm@example.org".into(),
            port: port.into(),
            ..SmtpServerConfig::default()
        },
        &file,
    )
    .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    sender
}

/// The SMTP sender refuses a bad recipient and a bad port before any network call, never
/// echoing its credentials; the `MailSender` impl refuses when SMTP is not configured; minting
/// a link with a key the given KEK cannot unwrap fails.
#[tokio::test]
async fn the_email_sender_refuses_bad_input_without_leaking() {
    let sender = smtp_sender("smtp.invalid", "not-a-port");
    assert!(
        sender
            .build_message("not an address", "s", "b".into())
            .is_err()
    );
    let msg = sender
        .build_message("anna@example.org", "s", "b".into())
        .unwrap();
    let err = sender.send(msg).await.unwrap_err();
    assert!(err.to_string().contains("SMTP port"), "{err}");
    assert!(!err.to_string().contains("smtp-word-for-tests"));

    let unconfigured: Arc<dyn MailSender> = Arc::new(EmailSender::default());
    assert!(
        unconfigured
            .send_mail("dev", "anna@example.org", "s", "b", "https://link")
            .await
            .is_err()
    );

    let store: Arc<dyn Store> = Arc::new(MemoryStore::new(Kek::generate_for_tests()));
    let kek = Kek::generate_for_tests();
    let r = EmailSender::mint_action_token(
        &store,
        &kek,
        "no-keys",
        ISSUER,
        "u1",
        vec!["VERIFY_EMAIL".into()],
        None,
        None,
        None,
    )
    .await;
    assert!(r.is_err());
}

fn outbox_row(id: &str, realm_id: &str) -> EmailOutboxRecord {
    EmailOutboxRecord {
        id: id.into(),
        realm_id: realm_id.into(),
        org_id: None,
        user_id: "u1".into(),
        to_address: "anna@example.org".into(),
        locale: "en".into(),
        template_kind: "UPDATE_PASSWORD".into(),
        action_jti: format!("jti-{id}"),
        actions: vec!["UPDATE_PASSWORD".into()],
        client_id: None,
        redirect_uri: None,
        lifespan_secs: 900,
        issuer: ISSUER.into(),
        status: "pending".into(),
        attempts: 0,
        last_error: None,
        next_attempt_at: Utc::now() - chrono::Duration::seconds(1),
        created_at: Utc::now(),
        sent_at: None,
    }
}

/// The dispatcher gives up after `max_attempts` failures (status `failed`, visible to admins);
/// a row whose key cannot be unwrapped is not sent; a key without a realm row still builds a link.
#[tokio::test]
async fn the_outbox_dispatcher_fails_rows_after_max_attempts_and_without_a_usable_key() {
    let kek = Kek::generate_for_tests();
    let mem = Arc::new(MemoryStore::new(kek.clone()));
    let store: Arc<dyn Store> = mem.clone();
    store
        .upsert_signing_key(KeyManager::generate_es256("keyonly", &kek, 100, "ACTIVE").unwrap())
        .await
        .unwrap();
    let sink = RecordingSender::new();
    sink.set_failures(1);
    let dispatcher = OutboxDispatcher::new(store.clone(), kek.clone(), Arc::new(sink.clone()))
        .with_max_attempts(1);

    store
        .enqueue_outbox(outbox_row("ob-fail", "keyonly"))
        .await
        .unwrap();
    assert_eq!(
        dispatcher
            .dispatch_realm_batch(&mount(), "keyonly", 10)
            .await
            .unwrap(),
        0
    );
    let rows = store.list_outbox_rows("keyonly", None).await.unwrap();
    assert_eq!(rows[0].status, "failed", "{rows:?}");
    assert!(sink.messages().is_empty());

    // No realm row: the link names the realm id.
    let link = dispatcher
        .mint_link_for_row(&mount(), &outbox_row("ob-link", "keyonly"))
        .await
        .unwrap();
    assert!(
        link.contains("/realms/keyonly/login-actions/action-token?key="),
        "{link}"
    );

    // A key this dispatcher cannot unwrap (another KEK): the row is not sent and records why.
    let foreign = OutboxDispatcher::new(
        store.clone(),
        Kek::generate_for_tests(),
        Arc::new(sink.clone()),
    );
    store
        .enqueue_outbox(outbox_row("ob-nokey", "keyonly"))
        .await
        .unwrap();
    assert_eq!(
        foreign
            .dispatch_realm_batch(&mount(), "keyonly", 10)
            .await
            .unwrap(),
        0
    );
    let row = store
        .list_outbox_rows("keyonly", None)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.id == "ob-nokey")
        .unwrap();
    assert_eq!(row.status, "pending", "{row:?}");
    assert!(row.last_error.is_some());
    assert!(sink.messages().is_empty());
}

struct Passkey {
    pair: aws_lc_rs::signature::EcdsaKeyPair,
    cred_id_b64: String,
    attestation_b64: String,
}

fn passkey(cred_id: &[u8], rp_id: &str) -> Passkey {
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
    use sha2::{Digest, Sha256};
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new())
        .unwrap();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref()).unwrap();
    let pubk = pair.public_key().as_ref().to_vec();
    let mut cose = vec![0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21, 0x58, 0x20];
    cose.extend_from_slice(&pubk[1..33]);
    cose.extend_from_slice(&[0x22, 0x58, 0x20]);
    cose.extend_from_slice(&pubk[33..65]);
    let mut ad = Sha256::digest(rp_id.as_bytes()).to_vec();
    ad.push(AUTH_DATA_FLAG_UP | AUTH_DATA_FLAG_UV | AUTH_DATA_FLAG_AT);
    ad.extend_from_slice(&1u32.to_be_bytes());
    ad.extend_from_slice(&[0u8; 16]);
    ad.extend_from_slice(&(cred_id.len() as u16).to_be_bytes());
    ad.extend_from_slice(cred_id);
    ad.extend_from_slice(&cose);
    let mut att = vec![0xa3, 0x63, b'f', b'm', b't', 0x64, b'n', b'o', b'n', b'e'];
    att.extend_from_slice(&[0x67, b'a', b't', b't', b'S', b't', b'm', b't', 0xa0]);
    att.extend_from_slice(&[0x68, b'a', b'u', b't', b'h', b'D', b'a', b't', b'a', 0x58]);
    att.push(ad.len() as u8);
    att.extend_from_slice(&ad);
    Passkey {
        pair,
        cred_id_b64: URL_SAFE_NO_PAD.encode(cred_id),
        attestation_b64: URL_SAFE_NO_PAD.encode(&att),
    }
}

fn registration_json(pk: &Passkey, challenge: &str, origin: &str) -> serde_json::Value {
    json!({
        "id": pk.cred_id_b64, "rawId": pk.cred_id_b64, "type": "public-key",
        "response": {
            "clientDataJSON": URL_SAFE_NO_PAD.encode(
                json!({ "type": "webauthn.create", "challenge": challenge, "origin": origin }).to_string()),
            "attestationObject": pk.attestation_b64,
        }
    })
}

fn assertion_json(
    pk: &Passkey,
    challenge: &str,
    rp_id: &str,
    handle: Option<&str>,
) -> serde_json::Value {
    use aws_lc_rs::rand::SystemRandom;
    use sha2::{Digest, Sha256};
    let cdata = json!({ "type": "webauthn.get", "challenge": challenge, "origin": format!("https://{rp_id}") })
        .to_string()
        .into_bytes();
    let mut ad = Sha256::digest(rp_id.as_bytes()).to_vec();
    ad.push(AUTH_DATA_FLAG_UP | AUTH_DATA_FLAG_UV);
    ad.extend_from_slice(&2u32.to_be_bytes());
    let mut signed = ad.clone();
    signed.extend_from_slice(&Sha256::digest(&cdata));
    let sig = pk.pair.sign(&SystemRandom::new(), &signed).unwrap();
    let mut resp = json!({
        "clientDataJSON": URL_SAFE_NO_PAD.encode(&cdata),
        "authenticatorData": URL_SAFE_NO_PAD.encode(&ad),
        "signature": URL_SAFE_NO_PAD.encode(sig.as_ref()),
    });
    if let Some(h) = handle {
        resp["userHandle"] = json!(h);
    }
    json!({ "id": pk.cred_id_b64, "rawId": pk.cred_id_b64, "type": "public-key", "response": resp })
}

/// The passkey helpers: extra allowed origins from the realm, creation options listing the
/// person's existing credentials, malformed JSON, unknown credential, the usernameless userHandle
/// rule, a disabled person and a corrupted stored key are each refused.
#[tokio::test]
async fn passkey_helpers_refuse_every_broken_input() {
    let (_, mem) = app().await;
    let store: Arc<dyn Store> = mem.clone();
    let m = mount();
    let mut realm = store.get_realm("dev").await.unwrap().unwrap();
    realm.attributes.insert(
        "webauthn.allowed.origins".into(),
        "https://app.example.test, ,https://idm.dev.example.test".into(),
    );
    let origins = allowed_origins_for_realm(&realm, &m);
    assert!(origins.contains(&"https://app.example.test".to_owned()));
    assert_eq!(
        origins
            .iter()
            .filter(|o| *o == "https://idm.dev.example.test")
            .count(),
        1
    );

    let rp_id = "idm.dev.example.test";
    let mut member = user(&mem, MEMBER).await;
    let handle = ensure_user_handle(&store, "dev", &mut member)
        .await
        .unwrap();
    assert_eq!(
        ensure_user_handle(&store, "dev", &mut member)
            .await
            .unwrap(),
        handle
    );

    let pk = passkey(b"helper-cred-1", rp_id);
    assert!(
        complete_passkey_registration(
            &store,
            &m,
            &realm,
            &mut member,
            "c1",
            &json!({ "id": 1 }),
            None,
            true
        )
        .await
        .is_err()
    );
    let challenge = URL_SAFE_NO_PAD.encode(b"registration-challenge-1");
    let reg = registration_json(&pk, &challenge, "https://app.example.test");
    let rec = complete_passkey_registration(
        &store,
        &m,
        &realm,
        &mut member,
        &challenge,
        &reg,
        None,
        true,
    )
    .await
    .unwrap();
    assert_eq!(rec.user_label.as_deref(), Some("Passkey"));

    let opts = build_creation_options(&store, &m, &realm, &mut member, "opts-challenge")
        .await
        .unwrap();
    assert_eq!(opts["user"]["id"], handle.as_str());
    assert_eq!(opts["excludeCredentials"][0]["id"], pk.cred_id_b64.as_str());
    assert_eq!(opts["challenge"], "opts-challenge");

    let ch = URL_SAFE_NO_PAD.encode(b"login-challenge-1");
    let auth = |expected: Option<String>, body: serde_json::Value| {
        let (store, m, realm, ch) = (store.clone(), m.clone(), realm.clone(), ch.clone());
        async move {
            complete_passkey_authentication(
                &store,
                &m,
                &realm,
                expected.as_deref(),
                &ch,
                &body,
                true,
            )
            .await
        }
    };
    assert!(auth(None, json!({ "id": "x" })).await.is_err(), "malformed");
    let unknown = passkey(b"never-registered", rp_id);
    assert!(
        auth(None, assertion_json(&unknown, &ch, rp_id, Some(&handle)))
            .await
            .is_err()
    );
    assert!(
        auth(
            Some("someone-else".into()),
            assertion_json(&pk, &ch, rp_id, Some(&handle))
        )
        .await
        .is_err(),
        "credential of another person"
    );
    assert!(
        auth(None, assertion_json(&pk, &ch, rp_id, None))
            .await
            .is_err(),
        "usernameless needs userHandle"
    );
    let (u, uv) = auth(
        Some(member.id.clone()),
        assertion_json(&pk, &ch, rp_id, Some(&handle)),
    )
    .await
    .unwrap();
    assert_eq!(u.id, member.id);
    assert!(uv);

    let mut disabled = user(&mem, MEMBER).await;
    disabled.enabled = false;
    store.update_user(disabled.clone()).await.unwrap();
    assert!(
        auth(None, assertion_json(&pk, &ch, rp_id, Some(&handle)))
            .await
            .is_err()
    );
    disabled.enabled = true;
    store.update_user(disabled).await.unwrap();

    let mut stored = store
        .get_webauthn_by_credential_id("dev", &pk.cred_id_b64)
        .await
        .unwrap()
        .unwrap();
    stored.public_key_cose_json = "not json".into();
    store.update_webauthn_credential(stored).await.unwrap();
    assert!(
        auth(None, assertion_json(&pk, &ch, rp_id, Some(&handle)))
            .await
            .is_err()
    );
}

/// Completing passkey registration drops both passkey required-action names (machine edge).
#[test]
fn register_webauthn_completion_clears_both_passkey_actions() {
    let ctx = LoginPolicyContext {
        pending_actions: vec![
            "webauthn-register".into(),
            "CONFIGURE_PASSKEY".into(),
            "TERMS_AND_CONDITIONS".into(),
        ],
        ..Default::default()
    };
    let next = transition(
        LoginState::RegisterWebAuthn {
            user_id: "u1".into(),
            first_factor_passed: true,
            second_factor_passed: false,
        },
        LoginInput::CompleteRegisterWebAuthnOk,
        &ctx,
    )
    .unwrap();
    assert!(
        matches!(next, LoginState::TermsAndConditions { .. }),
        "{next:?}"
    );
}
