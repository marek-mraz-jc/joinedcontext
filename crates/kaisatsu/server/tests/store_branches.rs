#![forbid(unsafe_code)]
//! Branches of the OIDC and login endpoints that need a store that fails or loses a row, or state a normal
//! flow does not leave (T-7802): a realm lookup that fails, a code page whose realm went away, a
//! person deleted under an SSO session, a login hint the client already gave, a staged signing
//! key that takes over at its activation time, a pinned algorithm whose key never appears, the
//! SSO session's organization, PKCE `plain` stored on a code, an expired offline session, a
//! refresh whose session is gone, and the store failing while a reused refresh token's family is
//! revoked; a reset mail whose counter or outbox fails, and organization pins by id or alias.
//! Every failure answers a 500 that names nothing of the store.

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::{Extension, Router};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use kaisatsu_store_memory::MemoryStore;
use serde_json::Value;
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

async fn app() -> (Router, Arc<MemoryStore>, Arc<fault_store::FaultStore>, Kek) {
    let kek = Kek::generate_for_tests();
    let memory = Arc::new(MemoryStore::new(kek.clone()));
    let faults = fault_store::FaultStore::new(memory.clone());
    let state = setup_dev_realm_on(faults.clone(), kek.clone()).await;
    let app = router(state, mount()).layer(Extension(mount()));
    (app, memory, faults, kek)
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

const MEMBER: &str = "demo.orgmember@hel.fi";

/// The refresh family id of a refresh token (`{family}.{secret}`).
fn family_of(refresh_token: &str) -> String {
    refresh_token.split_once('.').unwrap().0.to_owned()
}

async fn refresh(app: &Router, refresh_token: &str) -> (StatusCode, String) {
    let (st, _, body) = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("refresh_token", refresh_token),
        ],
        None,
    )
    .await;
    (st, body)
}

fn oauth_error(body: &str) -> String {
    serde_json::from_str::<Value>(body).unwrap_or(Value::Null)["error"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// A store failure is a 500 whose body says nothing of the store.
fn assert_store_failure(st: StatusCode, body: &str) {
    assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(
        !body.contains("injected"),
        "the store's message leaked: {body}"
    );
}

async fn sso_session(store: &MemoryStore) -> SsoSession {
    let member = store
        .get_user_by_username_or_email("dev", None, MEMBER)
        .await
        .unwrap()
        .unwrap();
    store
        .list_user_sso_sessions("dev", &member.id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the login left an SSO session")
}

#[tokio::test]
async fn a_realm_lookup_that_fails_is_a_500_on_every_realm_route() {
    let (app, _, faults, _) = app().await;
    faults.fail_always("get_realm");
    for uri in [
        "/realms/dev",
        "/realms/dev/.well-known/openid-configuration",
        "/realms/dev/protocol/openid-connect/certs",
    ] {
        let (st, _, body) = get(&app, uri, None, None).await;
        assert_store_failure(st, &body);
    }
    faults.clear();
    let (st, _, _) = get(&app, "/realms/dev", None, None).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn an_acr_claim_without_a_usable_value_asks_for_no_step_up() {
    let (app, _, _, _) = app().await;
    for claims in [
        r#"{"id_token":{"acr":{"essential":true}}}"#,
        r#"{"id_token":{"acr":{"values":[1,"mfa"]}}}"#,
        r#"{"userinfo":{"acr":{"values":[]}}}"#,
    ] {
        let q = url::form_urlencoded::byte_serialize(claims.as_bytes()).collect::<String>();
        let (st, h, body) = get(&app, &auth_query(&format!("&claims={q}")), None, None).await;
        assert_eq!(st, StatusCode::SEE_OTHER, "{claims}: {body}");
        assert!(
            location(&h).contains("/login-actions/authenticate"),
            "{claims}: {}",
            location(&h)
        );
    }
}

#[tokio::test]
async fn an_error_page_whose_realm_went_away_meanwhile_keeps_the_error() {
    let (app, _, faults, _) = app().await;
    // The realm check and the handler read the realm; the error page's own read finds none.
    faults.empty("get_realm", 2, 1);
    let mut req = Request::builder()
        .uri(format!(
            "{AUTH}?client_id=no-such-client&response_type=code"
        ))
        .header(header::ACCEPT, "text/html");
    req = req.header(header::USER_AGENT, "Mozilla/5.0");
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(st.is_client_error(), "{st} {body}");
    assert!(
        !body.contains("<html"),
        "no page without a realm to brand it: {body}"
    );
}

#[tokio::test]
async fn a_silent_consent_refusal_without_state_carries_no_state() {
    let (app, store, _, _) = app().await;
    let (_, sso) = browser_login(&app).await;
    let mut edge = store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .unwrap();
    edge.consent_required = true;
    store.upsert_client(edge).await.unwrap();
    let q = auth_query("&prompt=none").replace("&state=st-1", "");
    let (st, h, _) = get(&app, &q, Some(&sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let loc = location(&h);
    assert_eq!(
        query_param(&loc, "error").as_deref(),
        Some("consent_required")
    );
    assert_eq!(query_param(&loc, "state"), None, "{loc}");
}

#[tokio::test]
async fn a_person_deleted_under_a_live_sso_session_signs_in_again() {
    let (app, store, faults, _) = app().await;
    let (_, sso) = browser_login(&app).await;
    // The session check finds the person; the step-up read right after finds none.
    faults.empty("get_user_by_id", 1, 1);
    let (st, h, body) = get(&app, &auth_query("&prompt=login"), Some(&sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    assert!(
        location(&h).contains("/login-actions/authenticate"),
        "{}",
        location(&h)
    );
    // Deleted for good: the session no longer stands in for a sign-in, even silently.
    let member = store
        .get_user_by_username_or_email("dev", None, MEMBER)
        .await
        .unwrap()
        .unwrap();
    faults.empty("get_user_by_id", 0, u32::MAX);
    let (st, h, _) = get(&app, &auth_query("&prompt=none"), Some(&sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        query_param(&location(&h), "error").as_deref(),
        Some("login_required"),
        "{}",
        member.id
    );
}

#[tokio::test]
async fn a_login_hint_the_client_gave_wins_over_the_sessions_name() {
    let (app, _, _, _) = app().await;
    let (_, sso) = browser_login(&app).await;
    for extra in [
        // An application-initiated action that must sign in again first.
        "&kc_action=UPDATE_PASSWORD&prompt=login&login_hint=hint%40example.test",
        // A step-up the person has no authenticator for.
        "&acr_values=mfa&login_hint=hint%40example.test",
    ] {
        let (st, h, body) = get(&app, &auth_query(extra), Some(&sso), None).await;
        assert_eq!(st, StatusCode::SEE_OTHER, "{extra}: {body}");
        let login = location(&h);
        assert!(
            login.contains("/login-actions/authenticate"),
            "{extra}: {login}"
        );
        let (st, _, html) = get(&app, &login, Some(&auth_cookie(&h)), None).await;
        assert_eq!(st, StatusCode::OK, "{extra}: {html}");
        assert!(
            html.contains("hint@example.test"),
            "{extra}: the client's hint is shown"
        );
        assert!(
            !html.contains(&format!("value=\"{MEMBER}\"")),
            "{extra}: {html}"
        );
    }
}

#[tokio::test]
async fn a_staged_signing_key_signs_from_its_activation_time_on() {
    let (app, store, _, kek) = app().await;
    let (code, _) = browser_login(&app).await;
    let keys = store.list_signing_keys("dev").await.unwrap();
    // The realm signs edge's tokens with RS256: stage the next RS256 key.
    let active = keys
        .iter()
        .find(|k| k.status == "ACTIVE" && k.algorithm == "RS256")
        .unwrap()
        .clone();
    let mut staged = KeyManager::generate_rs256("dev", &kek, active.priority, "PASSIVE").unwrap();
    staged.activate_at = Some(jc_time::TimestampTz::now() - jiff::SignedDuration::from_secs(1));
    let staged_kid = staged.kid.clone();
    let staged_id = staged.id.clone();
    store.upsert_signing_key(staged).await.unwrap();
    let tokens = exchange(&app, &code).await;
    let header = tokens["access_token"]
        .as_str()
        .unwrap()
        .split('.')
        .next()
        .unwrap();
    let header: Value = serde_json::from_slice(
        &base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, header).unwrap(),
    )
    .unwrap();
    assert_eq!(header["kid"], staged_kid.as_str(), "{header}");
    let keys = store.list_signing_keys("dev").await.unwrap();
    let status = |id: &str| keys.iter().find(|k| k.id == id).unwrap().status.clone();
    assert_eq!(status(&staged_id), "ACTIVE");
    assert_eq!(
        status(&active.id),
        "PASSIVE",
        "the old key stays to verify, it no longer signs"
    );
}

#[tokio::test]
async fn a_pinned_algorithm_whose_key_never_appears_is_a_500() {
    let (app, store, faults, _) = app().await;
    let (code, _) = browser_login(&app).await;
    let mut edge = store
        .get_client_by_client_id("dev", "edge")
        .await
        .unwrap()
        .unwrap();
    edge.attributes
        .insert("access.token.signed.response.alg".into(), "EdDSA".into());
    store.upsert_client(edge).await.unwrap();
    // The store claims to have generated the key but holds none.
    faults.empty("ensure_signing_key", 0, 1);
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("code", &code),
            ("redirect_uri", CALLBACK),
            ("code_verifier", VERIFIER),
        ],
        None,
    )
    .await;
    assert_store_failure(st, &body);
    assert!(!body.contains("access_token"), "{body}");
}

async fn pin_session_org(store: &MemoryStore, org_id: &str) {
    let mut sso = sso_session(store).await;
    sso.org_id = Some(org_id.into());
    store.upsert_sso_session(sso).await.unwrap();
}

async fn silent_code(app: &Router, sso: &str) -> String {
    let (st, h, body) = get(app, &auth_query(""), Some(sso), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    query_param(&location(&h), "code").expect("a silent code")
}

#[tokio::test]
async fn the_sso_sessions_organization_is_kept_for_a_member_and_for_the_persons_own() {
    let (app, store, _, _) = app().await;
    let (_, sso) = browser_login(&app).await;
    store
        .upsert_organization(Organization {
            id: "org-acme".into(),
            realm_id: "dev".into(),
            name: "Acme".into(),
            alias: "acme".into(),
            enabled: true,
            description: String::new(),
            domains: Vec::new(),
            attributes: Default::default(),
            managed_by: None,
        })
        .await
        .unwrap();
    let member = store
        .get_user_by_username_or_email("dev", None, MEMBER)
        .await
        .unwrap()
        .unwrap();
    store
        .upsert_membership(Membership {
            id: "m-acme".into(),
            realm_id: "dev".into(),
            org_id: "org-acme".into(),
            person_id: member.id.clone(),
            org_roles: Vec::new(),
            created_at: jc_time::TimestampTz::now(),
        })
        .await
        .unwrap();
    // A member of the session's organization: the token family carries its alias.
    pin_session_org(&store, "org-acme").await;
    let tokens = exchange(&app, &silent_code(&app, &sso).await).await;
    let claims = decode_jwt_payload(tokens["access_token"].as_str().unwrap());
    assert!(
        claims["jc_org"].to_string().contains("\"acme\""),
        "{claims}"
    );

    // An organization the realm does not hold, but the person's own: kept as the person's.
    let mut person = member.clone();
    person.org_id = Some("own-org".into());
    store.update_user(person).await.unwrap();
    pin_session_org(&store, "own-org").await;
    let tokens = exchange(&app, &silent_code(&app, &sso).await).await;
    let claims = decode_jwt_payload(tokens["access_token"].as_str().unwrap());
    assert!(
        claims["jc_org"].to_string().contains("\"own-org\""),
        "{claims}"
    );
}

#[tokio::test]
async fn a_code_stored_with_plain_pkce_is_refused_at_the_exchange() {
    let (app, store, _, _) = app().await;
    let member = store
        .get_user_by_username_or_email("dev", None, MEMBER)
        .await
        .unwrap()
        .unwrap();
    let now = jc_time::TimestampTz::now();
    store
        .put_auth_code(AuthorizationCodeRecord {
            code_hash: sha256_hex("plain-code"),
            realm_id: "dev".into(),
            org_id: None,
            client_id: "edge".into(),
            user_id: member.id,
            sso_session_id: "no-session".into(),
            redirect_uri: CALLBACK.into(),
            scope: "openid".into(),
            mapper_scopes: Vec::new(),
            code_challenge: VERIFIER.into(),
            code_challenge_method: "plain".into(),
            nonce: None,
            acr: "1".into(),
            amr: vec!["pwd".into()],
            dpop_jkt: None,
            auth_time: now.as_second(),
            consumed: false,
            created_at: now,
            expires_at: now + jiff::SignedDuration::from_secs(60),
        })
        .await
        .unwrap();
    let (st, _, body) = post_form(
        &app,
        TOKEN,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("code", "plain-code"),
            ("redirect_uri", CALLBACK),
            ("code_verifier", VERIFIER),
        ],
        None,
    )
    .await;
    assert_eq!(
        (st, oauth_error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_grant"),
        "{body}"
    );
    assert!(body.contains("PKCE S256 required"), "{body}");
}

async fn set_offline(
    store: &MemoryStore,
    family_id: &str,
    edit: impl FnOnce(&mut RefreshTokenFamily),
) {
    let mut family = store
        .get_refresh_family("dev", family_id)
        .await
        .unwrap()
        .unwrap();
    family.offline = true;
    edit(&mut family);
    store.upsert_refresh_family(family).await.unwrap();
}

async fn set_offline_max(store: &MemoryStore, enabled: bool, secs: i64) {
    let mut realm = store.get_realm("dev").await.unwrap().unwrap();
    realm.offline_session_max_lifespan_enabled = enabled;
    realm.offline_session_max_lifespan = secs;
    store.upsert_realm(realm).await.unwrap();
}

#[tokio::test]
async fn an_offline_session_past_the_realm_maximum_gets_no_refresh() {
    let (app, store, _, _) = app().await;
    set_offline_max(&store, true, 60).await;
    let hour_ago = jc_time::TimestampTz::now() - jiff::SignedDuration::from_hours(1);
    // Counted from offline_since.
    let (code, _) = browser_login(&app).await;
    let rt = exchange(&app, &code).await["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();
    set_offline(&store, &family_of(&rt), |f| {
        f.offline_since = Some(hour_ago)
    })
    .await;
    let (st, body) = refresh(&app, &rt).await;
    assert_eq!(
        (st, oauth_error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_grant")
    );
    assert!(body.contains("Offline session expired"), "{body}");
    // A family stored before offline_since: counted from its sign-in.
    let (code, _) = browser_login(&app).await;
    let rt = exchange(&app, &code).await["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();
    set_offline(&store, &family_of(&rt), |f| {
        f.offline_since = None;
        f.auth_time = hour_ago.as_second();
    })
    .await;
    let (st, body) = refresh(&app, &rt).await;
    assert!(body.contains("Offline session expired"), "{st} {body}");
    // The realm turns the maximum off: the same family refreshes.
    set_offline_max(&store, false, 60).await;
    let (st, body) = refresh(&app, &rt).await;
    assert_eq!(st, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_refresh_whose_session_is_gone_is_refused() {
    let (app, _, faults, _) = app().await;
    let (code, _) = browser_login(&app).await;
    let rt = exchange(&app, &code).await["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();
    faults.empty("get_sso_session", 0, 1);
    let (st, body) = refresh(&app, &rt).await;
    assert_eq!(
        (st, oauth_error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_grant"),
        "{body}"
    );
    assert!(body.contains("Session not active"), "{body}");
}

/// A login, one rotation; answers the rotated-away and the current refresh token.
async fn rotated(app: &Router) -> (String, String) {
    let (code, _) = browser_login(app).await;
    let first = exchange(app, &code).await["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let (st, body) = refresh(app, &first).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    (first, v["refresh_token"].as_str().unwrap().to_owned())
}

#[tokio::test]
async fn a_reused_refresh_token_revokes_its_family_even_when_the_reread_finds_none() {
    let (app, store, faults, _) = app().await;
    let (old, current) = rotated(&app).await;
    // The lookup finds the family; the re-read under the revocation finds none (deleted by a
    // concurrent logout): the family as presented is revoked.
    faults.empty("get_refresh_family", 1, 1);
    let (st, body) = refresh(&app, &old).await;
    assert_eq!(
        (st, oauth_error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_grant")
    );
    let family = store
        .get_refresh_family("dev", &family_of(&old))
        .await
        .unwrap()
        .unwrap();
    assert!(family.revoked);
    let (st, body) = refresh(&app, &current).await;
    assert_eq!(
        (st, oauth_error(&body).as_str()),
        (StatusCode::BAD_REQUEST, "invalid_grant")
    );
}

#[tokio::test]
async fn a_store_failure_while_revoking_a_reused_family_is_a_500() {
    let (app, _, faults, _) = app().await;
    for (method, skip) in [
        ("get_refresh_family", 1),
        ("upsert_refresh_family", 0),
        ("append_event", 0),
    ] {
        let (old, _) = rotated(&app).await;
        faults.fail(method, skip, 1, fault_store::storage);
        let (st, body) = refresh(&app, &old).await;
        assert_store_failure(st, &body);
        faults.clear();
    }
}

#[tokio::test]
async fn userinfo_for_a_person_gone_with_a_session_gone_meanwhile_is_401() {
    let (app, _, faults, _) = app().await;
    let (code, _) = browser_login(&app).await;
    let access = exchange(&app, &code).await["access_token"]
        .as_str()
        .unwrap()
        .to_owned();
    // The liveness check finds the session; then the person is gone, and so is the session
    // the handler falls back to.
    faults.empty("get_user_by_id", 0, 1);
    faults.empty("get_sso_session", 1, 1);
    let (st, _, body) = get(
        &app,
        "/realms/dev/protocol/openid-connect/userinfo",
        None,
        Some(&format!("Bearer {access}")),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
    assert!(!body.contains(MEMBER), "{body}");
}

const RESET: &str = "/realms/dev/login-actions/reset-credentials";

#[tokio::test]
async fn a_reset_mail_is_not_queued_when_its_counter_or_the_outbox_fails_and_the_answer_is_the_same()
 {
    let (app, store, faults, _) = app().await;
    for method in ["update_login_failure", "enqueue_outbox"] {
        faults.fail(method, 0, 1, fault_store::storage);
        let (st, _, html) = post_form(&app, RESET, &[("username", MEMBER)], None).await;
        assert_eq!(st, StatusCode::OK, "{method}: {html}");
        assert!(html.contains("reset-credentials-sent"), "{method}: {html}");
        assert!(!html.contains("injected"), "{method}");
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        faults.clear();
    }
    let queued = store.list_outbox_rows("dev", None).await.unwrap();
    assert!(
        !queued
            .iter()
            .any(|r| r.template_kind == "RESET_CREDENTIALS"),
        "{queued:?}"
    );
}

async fn acme(store: &MemoryStore) {
    store
        .upsert_organization(Organization {
            id: "org-acme".into(),
            realm_id: "dev".into(),
            name: "Acme".into(),
            alias: "acme".into(),
            enabled: true,
            description: String::new(),
            domains: Vec::new(),
            attributes: Default::default(),
            managed_by: None,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_sign_in_pins_an_organization_named_by_id_or_alias_and_survives_an_organization_list_failure()
 {
    let (app, store, faults, _) = app().await;
    acme(&store).await;
    let mut member = store
        .get_user_by_username_or_email("dev", None, MEMBER)
        .await
        .unwrap()
        .unwrap();
    // The person's own organization named by its alias: pinned by the scope that names its id.
    member.org_id = Some("acme".into());
    store.update_user(member.clone()).await.unwrap();
    let (st, h, _) = get(
        &app,
        &auth_query("").replace("scope=openid&", "scope=openid%20organization:org-acme&"),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let (st, h2, body) = login_from(&app, &location(&h), &auth_cookie(&h)).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    let code = query_param(&location(&h2), "code").expect("code");
    let tokens = exchange(&app, &code).await;
    assert!(
        tokens["scope"]
            .as_str()
            .unwrap()
            .contains("organization:acme"),
        "{tokens}"
    );

    // A membership by id, the scope naming the alias: pinned too.
    member.org_id = None;
    store.update_user(member.clone()).await.unwrap();
    store
        .upsert_membership(Membership {
            id: "m-acme".into(),
            realm_id: "dev".into(),
            org_id: "org-acme".into(),
            person_id: member.id.clone(),
            org_roles: Vec::new(),
            created_at: jc_time::TimestampTz::now(),
        })
        .await
        .unwrap();
    let (_, h, _) = get(
        &app,
        &auth_query("").replace("scope=openid&", "scope=openid%20organization:acme&"),
        None,
        None,
    )
    .await;
    let (st, h2, body) = login_from(&app, &location(&h), &auth_cookie(&h)).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    assert!(query_param(&location(&h2), "code").is_some());

    // The organization list fails: the sign-in still ends, pinning only what the membership
    // itself names, never a foreign organization.
    // Without it the alias cannot be matched to the membership's id: the pin is not honoured
    // and the person picks, failing closed.
    faults.fail_always("list_organizations");
    let pinned = auth_query("").replace("scope=openid&", "scope=openid%20organization:acme&");
    let (st, h, _) = get(&app, &pinned, None, None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let (st, _, body) = login_from(&app, &location(&h), &auth_cookie(&h)).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.contains("data-page-id=\"login-org-choice\""), "{body}");
}
