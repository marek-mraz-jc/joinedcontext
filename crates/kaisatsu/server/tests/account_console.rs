#![forbid(unsafe_code)]
//! The account console (G13): the console page, the account REST (personal info, credentials,
//! sessions) and the application-initiated actions it starts (`kc_action` UPDATE_PASSWORD and
//! CONFIGURE_TOTP, with re-authentication and "sign out other devices").
//!
//! Every guard has a test here: audience `account` only, a live session, CORS for the console's
//! origin only, read-only and claim-bearing attributes, an e-mail change clearing
//! `emailVerified`, the last authenticator kept where the realm requires one, other people's
//! credential and session ids, and another realm.

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
use std::collections::BTreeMap;
use std::sync::Arc;
use tower::ServiceExt;

mod common;
use common::*;

const ISSUER_ORIGIN: &str = "https://idm.dev.example.test";
const ACCOUNT: &str = "/realms/dev/account";
const AUTH: &str = "/realms/dev/protocol/openid-connect/auth";
const TOKEN: &str = "/realms/dev/protocol/openid-connect/token";
const CONSOLE_REDIRECT: &str = "https://idm.dev.example.test/realms/dev/account/";
const VERIFIER: &str = "pkce-verifier-string-with-at-least-43-characters-long-012345";
const MEMBER: &str = "demo.orgmember@hel.fi";
const MEMBER_PW: &str = "MemberPass123!Valid";
const ANALYST: &str = "demo.analyst@hel.fi";
const STEWARD: &str = "demo.steward@hel.fi";
const STEWARD_PW: &str = "StewardPass123!Valid";

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

/// The dev realm plus the `account-console` client jc provisions: public, PKCE, its redirect
/// under the account path, its origin from the redirect (`+`), audience `account`.
async fn app() -> (Router, Arc<MemoryStore>) {
    let (app, store, _) = app_and_kek().await;
    (app, store)
}

/// [`app`] plus the key-encryption key, for tests that sign their own tokens.
async fn app_and_kek() -> (Router, Arc<MemoryStore>, Kek) {
    let (state, store, kek) = setup_dev_realm_state().await;
    store
        .upsert_client(ClientRepresentation {
            id: "account-console-id".into(),
            realm_id: "dev".into(),
            client_id: "account-console".into(),
            name: Some("Account Console".into()),
            enabled: true,
            public_client: true,
            standard_flow_enabled: true,
            redirect_uris: vec![format!("{ISSUER_ORIGIN}/realms/dev/account/*")],
            web_origins: vec!["+".into()],
            default_client_scopes: vec!["basic".into(), "email".into(), "profile".into()],
            attributes: BTreeMap::from([
                ("pkce.code.challenge.method".into(), "S256".into()),
                ("post.logout.redirect.uris".into(), "+".into()),
            ]),
            protocol_mappers: vec![ProtocolMapperRepresentation {
                id: "aud-account".into(),
                name: "audience account".into(),
                protocol: "openid-connect".into(),
                protocol_mapper: "oidc-audience-mapper".into(),
                config: BTreeMap::from([
                    ("included.custom.audience".into(), "account".into()),
                    ("access.token.claim".into(), "true".into()),
                ]),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    (router(state, mount()).layer(Extension(mount())), store, kek)
}

async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<String>,
) -> (StatusCode, HeaderMap, String) {
    let mut b = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let body = match body {
        Some(s) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(s)
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let st = resp.status();
    let h = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (st, h, String::from_utf8_lossy(&bytes).into_owned())
}

fn location(h: &HeaderMap) -> String {
    h[header::LOCATION].to_str().unwrap().to_owned()
}

fn cookie_named(h: &HeaderMap, name: &str) -> Option<String> {
    h.get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap().to_owned())
        .find(|v| v.starts_with(&format!("{name}=")))
}

fn param(url: &str, key: &str) -> Option<String> {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

fn page_id(html: &str) -> String {
    let marker = "data-page-id=\"login-";
    let i = html.find(marker).map(|i| i + marker.len()).unwrap_or(0);
    html[i..i + html[i..].find('"').unwrap_or(0)].to_owned()
}

/// A login page reached from an authorization request, ready to post.
struct Page {
    url: String,
    csrf: String,
    session: String,
    cookies: String,
    html: String,
}

impl Page {
    async fn post(&self, app: &Router, fields: &[(&str, &str)]) -> (StatusCode, HeaderMap, String) {
        let mut all = vec![
            ("csrf_token", self.csrf.as_str()),
            ("session_code", self.session.as_str()),
        ];
        all.extend_from_slice(fields);
        post_form(app, &self.url, &all, Some(&self.cookies)).await
    }
}

/// Starts the console's authorization request (with `extra` query) in a browser that holds
/// `sso` (an SSO cookie) or none. Returns the login page or, on the SSO path, the redirect.
async fn authorize(
    app: &Router,
    extra: &str,
    sso: Option<&str>,
) -> std::result::Result<Page, Box<(StatusCode, HeaderMap, String)>> {
    let uri = format!(
        "{AUTH}?client_id=account-console&redirect_uri={}&response_type=code&scope=openid&state=st&code_challenge={}&code_challenge_method=S256{extra}",
        url::form_urlencoded::byte_serialize(CONSOLE_REDIRECT.as_bytes()).collect::<String>(),
        sha256_base64url(VERIFIER),
    );
    let hdrs: Vec<(&str, &str)> = sso.map(|c| vec![("cookie", c)]).unwrap_or_default();
    let (st, h, body) = call(app, Method::GET, &uri, &hdrs, None).await;
    if st != StatusCode::SEE_OTHER || !location(&h).contains("login-actions/authenticate") {
        return Err(Box::new((st, h, body)));
    }
    let url = location(&h);
    let auth = cookie_named(&h, "__Host-wk_auth").expect("auth cookie");
    let cookies = match sso {
        Some(s) => format!("{auth}; {s}"),
        None => auth,
    };
    let (st, _, html) = call(app, Method::GET, &url, &[("cookie", &cookies)], None).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    Ok(Page {
        csrf: extract_hidden_input(&html, "csrf_token"),
        session: extract_hidden_input(&html, "session_code"),
        url,
        cookies,
        html,
    })
}

async fn steward_totp(store: &MemoryStore) -> String {
    let s = user(store, STEWARD).await;
    let creds = store.list_credentials("dev", &s.id).await.unwrap();
    let otp = creds.iter().find(|c| c.credential_type == "otp").unwrap();
    generate_totp_code_for_credential(otp, chrono::Utc::now().timestamp() as u64).unwrap()
}

async fn user(store: &MemoryStore, name: &str) -> UserRepresentation {
    store
        .get_user_by_username_or_email("dev", None, name)
        .await
        .unwrap()
        .unwrap()
}

/// A signed-in console: the access token, its SSO cookie and the session id.
struct Signed {
    token: String,
    sso: String,
    sid: String,
}

async fn exchange(app: &Router, code: &str) -> Value {
    let (st, _, body) = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", "account-console"),
            ("code", code),
            ("redirect_uri", CONSOLE_REDIRECT),
            ("code_verifier", VERIFIER),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    serde_json::from_str(&body).unwrap()
}

/// Finishes a login whose last answer is the redirect with a code.
async fn finish(app: &Router, h: &HeaderMap, sso_before: Option<&str>) -> Signed {
    let loc = location(h);
    assert!(loc.starts_with(CONSOLE_REDIRECT), "{loc}");
    let code = param(&loc, "code").expect("code");
    let tok = exchange(app, &code).await;
    let token = tok["access_token"].as_str().unwrap().to_owned();
    let claims = decode_jwt_payload(&token);
    Signed {
        sso: cookie_named(h, "__Host-wk_sso")
            .or_else(|| sso_before.map(str::to_owned))
            .expect("sso cookie"),
        sid: claims["sid"].as_str().unwrap().to_owned(),
        token,
    }
}

async fn sign_in(app: &Router, store: &MemoryStore, who: &str, pw: &str) -> Signed {
    let page = authorize(app, "", None).await.ok().unwrap();
    let (st, h, html) = page.post(app, &[("username", who), ("password", pw)]).await;
    if st == StatusCode::SEE_OTHER {
        return finish(app, &h, None).await;
    }
    assert_eq!(page_id(&html), "login-otp", "{html}");
    let code = steward_totp(store).await;
    let (st, h, html) = page.post(app, &[("otp", &code)]).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{html}");
    finish(app, &h, None).await
}

fn bearer(t: &str) -> String {
    format!("Bearer {t}")
}

async fn rest(
    app: &Router,
    method: Method,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = bearer(token);
    let (st, _, text) = call(
        app,
        method,
        &format!("{ACCOUNT}{path}"),
        &[("authorization", &auth)],
        body.map(|b| b.to_string()),
    )
    .await;
    (st, serde_json::from_str(&text).unwrap_or(Value::Null))
}

#[tokio::test]
async fn the_console_page_is_one_locked_down_document_for_every_console_path() {
    let (app, _) = app().await;
    for path in [
        ACCOUNT.to_owned(),
        format!("{ACCOUNT}/"),
        format!("{ACCOUNT}/personal-info"),
        format!("{ACCOUNT}/account-security/signing-in"),
        format!("{ACCOUNT}/account-security/device-activity"),
    ] {
        let (st, h, html) =
            call(&app, Method::GET, &path, &[("accept-language", "sk")], None).await;
        assert_eq!(st, StatusCode::OK, "{path}");
        let csp = h[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("frame-ancestors 'none'") && csp.contains("script-src 'nonce-"));
        assert_eq!(h[header::REFERRER_POLICY], "no-referrer");
        assert!(html.contains("lang=\"sk\""));
        assert!(html.contains("\"clientId\":\"account-console\""));
        assert!(html.contains("Osobné údaje"), "Slovak strings");
    }
    let (st, _, _) = call(&app, Method::GET, "/realms/nope/account", &[], None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_rest_takes_only_live_audience_account_tokens_of_this_realm() {
    let (app, store) = app().await;
    let me = sign_in(&app, &store, MEMBER, MEMBER_PW).await;
    let claims = decode_jwt_payload(&me.token);
    assert!(
        claims["aud"]
            .as_array()
            .map_or(claims["aud"] == "account", |a| a
                .iter()
                .any(|v| v == "account")),
        "{claims}"
    );
    let (st, info) = rest(&app, Method::GET, "/", &me.token, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(info["username"], MEMBER);
    assert!(info.get("credentials").is_none());

    // No token, a garbage token, a token without audience `account` (an ID token or another
    // client's access token): 401 on every REST path.
    let id_token = {
        let page = authorize(&app, "", Some(&me.sso)).await;
        let (_, h, _) = *page.err().expect("SSO answers with a code");
        exchange(&app, &param(&location(&h), "code").unwrap()).await["id_token"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let (_, _, _, edge_token) = edge_access_token(&app).await;
    for path in ["/", "/credentials", "/sessions"] {
        let (st, h, _) = call(
            &app,
            Method::GET,
            &format!("{ACCOUNT}{path}"),
            &[("authorization", "Bearer x.y.z")],
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{path}");
        assert!(h.contains_key(header::WWW_AUTHENTICATE));
        for wrong in [&id_token, &edge_token] {
            let (st, _) = rest(&app, Method::GET, path, wrong, None).await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "{path}");
        }
        let (st, _, _) = call(&app, Method::DELETE, &format!("{ACCOUNT}{path}"), &[], None).await;
        assert!(
            st == StatusCode::UNAUTHORIZED || st == StatusCode::METHOD_NOT_ALLOWED,
            "{path} {st}"
        );
    }

    // Another realm refuses this realm's token (issuer and keys differ), and never reaches
    // this realm's sessions.
    store
        .upsert_realm(RealmConfig {
            id: "other".into(),
            realm: "other".into(),
            enabled: true,
            ..Default::default()
        })
        .await
        .unwrap();
    for (m, path) in [
        (Method::GET, "/realms/other/account/".to_owned()),
        (Method::GET, "/realms/other/account/sessions".to_owned()),
        (
            Method::DELETE,
            format!("/realms/other/account/sessions/{}", me.sid),
        ),
    ] {
        let auth = bearer(&me.token);
        let (st, _, _) = call(&app, m, &path, &[("authorization", &auth)], None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{path}");
    }
    assert!(
        store
            .get_sso_session("dev", &me.sid)
            .await
            .unwrap()
            .is_some()
    );

    // A token whose session ended is dead at once (logout, sign-out elsewhere).
    store.delete_sso_session("dev", &me.sid).await.unwrap();
    let (st, _) = rest(&app, Method::GET, "/", &me.token, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

/// Each token check stands alone: a token signed with the realm's own key that breaks exactly
/// one property (audience, issuer, typ, session holder, enabled person, no service account)
/// is refused, while the unchanged copy is accepted.
#[tokio::test]
async fn every_token_check_refuses_a_token_that_breaks_only_it() {
    let (app, store, kek) = app_and_kek().await;
    let me = sign_in(&app, &store, MEMBER, MEMBER_PW).await;
    let steward = sign_in(&app, &store, STEWARD, STEWARD_PW).await;
    let kid = jsonwebtoken::decode_header(&me.token).unwrap().kid.unwrap();
    let key = store
        .list_signing_keys("dev")
        .await
        .unwrap()
        .into_iter()
        .find(|k| k.kid == kid)
        .unwrap();
    let base = decode_jwt_payload(&me.token);
    let forge = |edit: &dyn Fn(&mut Value)| {
        let mut claims = base.clone();
        claims["jti"] = json!(uuid::Uuid::new_v4().to_string());
        edit(&mut claims);
        sign_jwt(&key, &kek, Some("JWT"), &claims).unwrap()
    };
    let (st, _) = rest(&app, Method::GET, "/", &forge(&|_| {}), None).await;
    assert_eq!(st, StatusCode::OK, "the unchanged copy is accepted");

    let cases: [(&str, &dyn Fn(&mut Value)); 4] = [
        ("audience", &|c| c["aud"] = json!(["broker"])),
        ("issuer", &|c| {
            c["iss"] = json!(format!("{ISSUER_ORIGIN}/realms/other"));
        }),
        ("typ", &|c| c["typ"] = json!("ID")),
        ("session holder", &|c| c["sid"] = json!(steward.sid)),
    ];
    for (what, edit) in cases {
        let (st, _) = rest(&app, Method::GET, "/", &forge(edit), None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{what}");
    }

    let token = forge(&|_| {});
    let before = user(&store, MEMBER).await;
    let mut disabled = before.clone();
    disabled.enabled = false;
    store.update_user(disabled).await.unwrap();
    let (st, _) = rest(&app, Method::GET, "/", &token, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "disabled person");
    let mut robot = before.clone();
    robot.service_account_client_id = Some("edge".into());
    store.update_user(robot).await.unwrap();
    let (st, _) = rest(&app, Method::GET, "/", &token, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "service account");
    store.update_user(before).await.unwrap();
    let (st, _) = rest(&app, Method::GET, "/", &token, None).await;
    assert_eq!(st, StatusCode::OK);
}

/// An access token of the confidential `edge` client (no `account` audience).
async fn edge_access_token(app: &Router) -> (StatusCode, HeaderMap, String, String) {
    let (st, h, body) = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "client_credentials"),
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
        ],
        None,
    )
    .await;
    let tok: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let at = tok["access_token"]
        .as_str()
        .unwrap_or("none.none.none")
        .to_owned();
    (st, h, body, at)
}

#[tokio::test]
async fn cors_answers_only_the_console_origin() {
    let (app, store) = app().await;
    let me = sign_in(&app, &store, MEMBER, MEMBER_PW).await;
    let preflight = |origin: &'static str| {
        let app = app.clone();
        async move {
            call(
                &app,
                Method::OPTIONS,
                &format!("{ACCOUNT}/sessions"),
                &[
                    ("origin", origin),
                    ("access-control-request-method", "DELETE"),
                    ("access-control-request-headers", "authorization"),
                ],
                None,
            )
            .await
        }
    };
    let (st, h, _) = preflight(ISSUER_ORIGIN).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(h[header::ACCESS_CONTROL_ALLOW_ORIGIN], ISSUER_ORIGIN);
    let (st, h, _) = preflight("https://evil.example").await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(!h.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));

    let auth = bearer(&me.token);
    let (st, h, _) = call(
        &app,
        Method::GET,
        &format!("{ACCOUNT}/"),
        &[("authorization", &auth), ("origin", ISSUER_ORIGIN)],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h[header::ACCESS_CONTROL_ALLOW_ORIGIN], ISSUER_ORIGIN);
    // A page of another origin neither reads nor changes the account, token or not.
    let (st, h, _) = call(
        &app,
        Method::POST,
        &format!("{ACCOUNT}/"),
        &[
            ("authorization", &auth),
            ("origin", "https://portal.dev.example.test"),
        ],
        Some(json!({"firstName": "Mallory"}).to_string()),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(!h.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    assert_ne!(
        user(&store, MEMBER).await.first_name.as_deref(),
        Some("Mallory")
    );
}

#[tokio::test]
async fn personal_info_changes_only_what_a_person_may_change() {
    let (app, store) = app().await;
    let me = sign_in(&app, &store, MEMBER, MEMBER_PW).await;
    let mut before = user(&store, MEMBER).await;
    before.email_verified = true;
    before
        .attributes
        .insert("jc_org".into(), vec!["helsinki".into()]);
    store.update_user(before.clone()).await.unwrap();

    // Names change; the console sends back what it read and that is fine.
    let (st, info) = rest(&app, Method::GET, "/", &me.token, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(info["attributes"].get("jc_org").is_none(), "{info}");
    let mut edit = info.clone();
    edit["firstName"] = json!("  Anna ");
    edit["lastName"] = json!("Nováková");
    let (st, info) = rest(&app, Method::POST, "/", &me.token, Some(edit)).await;
    assert_eq!(st, StatusCode::OK, "{info}");
    assert_eq!(info["firstName"], "Anna");
    assert!(
        user(&store, MEMBER).await.email_verified,
        "unchanged e-mail stays verified"
    );

    // Refused: username, id, emailVerified, admin-only attributes, claim-bearing attributes
    // (jc_org reaches tokens through a mapper), unknown members, bad e-mail, a taken e-mail.
    for (body, field, status) in [
        (
            json!({"username": "root"}),
            "username",
            StatusCode::BAD_REQUEST,
        ),
        (json!({"id": "someone-else"}), "id", StatusCode::BAD_REQUEST),
        (
            json!({"emailVerified": false}),
            "emailVerified",
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"attributes": {"jc_org": ["praha"]}}),
            "jc_org",
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"attributes": {"jc_groups": ["/platform-admins"]}}),
            "jc_groups",
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"attributes": {"department": ["x"]}}),
            "department",
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"attributes": {"locale": ["xx"]}}),
            "locale",
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"realmRoles": ["platform-admin"]}),
            "",
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"email": "not-an-address"}),
            "email",
            StatusCode::BAD_REQUEST,
        ),
        (json!({"email": ""}), "email", StatusCode::BAD_REQUEST),
        (
            json!({"firstName": "x".repeat(256)}),
            "firstName",
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"lastName": "a\u{0007}b"}),
            "lastName",
            StatusCode::BAD_REQUEST,
        ),
        (json!({"email": ANALYST}), "email", StatusCode::CONFLICT),
    ] {
        let (st, err) = rest(&app, Method::POST, "/", &me.token, Some(body.clone())).await;
        assert_eq!(st, status, "{body}");
        assert_eq!(err["field"], field, "{body} {err}");
    }
    let after = user(&store, MEMBER).await;
    assert_eq!(
        after.attributes.get("jc_org"),
        Some(&vec!["helsinki".to_owned()])
    );
    assert!(!after.attributes.contains_key("department"));
    assert_eq!(after.username, MEMBER);
    assert!(after.email_verified);

    // The locale attribute is the person's own.
    let (st, info) = rest(
        &app,
        Method::POST,
        "/",
        &me.token,
        Some(json!({"attributes": {"locale": ["sk"]}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(info["attributes"]["locale"], json!(["sk"]));

    // A new e-mail address is unverified.
    let (st, info) = rest(
        &app,
        Method::POST,
        "/",
        &me.token,
        Some(json!({"email": "anna.new@hel.fi"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(info["emailVerified"], false);
    let after = user(&store, MEMBER).await;
    assert_eq!(after.email.as_deref(), Some("anna.new@hel.fi"));
    assert!(!after.email_verified);
    let events = store.list_events("dev", None).await.unwrap();
    assert!(events.iter().any(|e| e.event_type == "UPDATE_EMAIL"));
}

#[tokio::test]
async fn a_mapper_on_an_editable_attribute_makes_it_read_only() {
    let (app, store) = app().await;
    let me = sign_in(&app, &store, MEMBER, MEMBER_PW).await;
    let mut console = store
        .get_client_by_client_id("dev", "account-console")
        .await
        .unwrap()
        .unwrap();
    console.protocol_mappers.push(ProtocolMapperRepresentation {
        id: "locale-claim".into(),
        name: "locale".into(),
        protocol: "openid-connect".into(),
        protocol_mapper: "oidc-usermodel-attribute-mapper".into(),
        config: BTreeMap::from([
            ("user.attribute".into(), "locale".into()),
            ("claim.name".into(), "locale".into()),
        ]),
        ..Default::default()
    });
    store.upsert_client(console).await.unwrap();
    let (st, err) = rest(
        &app,
        Method::POST,
        "/",
        &me.token,
        Some(json!({"attributes": {"locale": ["de"]}})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(err["errorMessage"], "accountAttributeReadOnly");
}

#[tokio::test]
async fn credentials_list_metadata_and_keep_the_required_authenticator() {
    let (app, store) = app().await;
    let steward = sign_in(&app, &store, STEWARD, STEWARD_PW).await;
    let member = sign_in(&app, &store, MEMBER, MEMBER_PW).await;
    let s = user(&store, STEWARD).await;
    let otp = store
        .list_credentials("dev", &s.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.credential_type == "otp")
        .unwrap();
    let pw = store
        .list_credentials("dev", &s.id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.credential_type == "password")
        .unwrap();

    let (st, groups) = rest(&app, Method::GET, "/credentials", &steward.token, None).await;
    assert_eq!(st, StatusCode::OK);
    let text = groups.to_string();
    assert!(
        !text.contains("secretData")
            && !text.contains(&otp.secret_data)
            && !text.contains("argon2")
    );
    let otp_group = groups
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["type"] == "otp")
        .unwrap();
    assert_eq!(otp_group["required"], true);
    assert_eq!(
        otp_group["userCredentialMetadatas"][0]["credential"]["id"],
        otp.id
    );

    // The member cannot touch the steward's credentials: 404, both stay.
    for id in [&otp.id, &pw.id] {
        let (st, _) = rest(
            &app,
            Method::DELETE,
            &format!("/credentials/{id}"),
            &member.token,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
    // The password cannot be removed; the steward's only authenticator neither (portal-approver
    // holds mfa-required).
    let (st, err) = rest(
        &app,
        Method::DELETE,
        &format!("/credentials/{}", pw.id),
        &steward.token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{err}");
    let (st, err) = rest(
        &app,
        Method::DELETE,
        &format!("/credentials/{}", otp.id),
        &steward.token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(err["errorMessage"], "accountOtpRequired");
    assert_eq!(store.list_credentials("dev", &s.id).await.unwrap().len(), 2);

    // With a second authenticator one of them may go.
    let mut second = otp.clone();
    second.id = "otp-second".into();
    store.upsert_credential(second).await.unwrap();
    let (st, _) = rest(
        &app,
        Method::DELETE,
        &format!("/credentials/{}", otp.id),
        &steward.token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = rest(
        &app,
        Method::DELETE,
        "/credentials/otp-second",
        &steward.token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // A person the realm does not hold to a second factor removes their last authenticator.
    let m = user(&store, MEMBER).await;
    store
        .upsert_credential(CredentialRecord {
            id: "member-otp".into(),
            realm_id: "dev".into(),
            user_id: m.id.clone(),
            credential_type: "otp".into(),
            ..otp.clone()
        })
        .await
        .unwrap();
    let (st, _) = rest(
        &app,
        Method::DELETE,
        "/credentials/member-otp",
        &member.token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = rest(
        &app,
        Method::DELETE,
        "/credentials/member-otp",
        &member.token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn passkeys_are_removed_only_by_their_owner() {
    let (app, store) = app().await;
    let steward = sign_in(&app, &store, STEWARD, STEWARD_PW).await;
    let member = sign_in(&app, &store, MEMBER, MEMBER_PW).await;
    let s = user(&store, STEWARD).await;
    let pk = store
        .insert_webauthn_credential(WebAuthnCredentialRecord {
            id: "pk-steward".into(),
            realm_id: "dev".into(),
            org_id: None,
            user_id: s.id.clone(),
            user_handle: "h".into(),
            credential_id: "cred-1".into(),
            public_key_cose_json: "{}".into(),
            cose_alg: -7,
            sign_count: 0,
            uv_initialized: true,
            backup_eligible: false,
            backup_state: false,
            transports: vec![],
            attestation_type: "none".into(),
            rp_id: "idm.dev.example.test".into(),
            user_label: Some("Laptop".into()),
            created_at: chrono::Utc::now(),
            last_used_at: None,
        })
        .await
        .unwrap();
    let (st, _) = rest(
        &app,
        Method::DELETE,
        &format!("/credentials/{}", pk.id),
        &member.token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(
        store
            .list_webauthn_credentials("dev", &s.id)
            .await
            .unwrap()
            .len(),
        1
    );
    let (_, groups) = rest(&app, Method::GET, "/credentials", &steward.token, None).await;
    assert!(groups.to_string().contains("Laptop"));
    let (st, _) = rest(
        &app,
        Method::DELETE,
        &format!("/credentials/{}", pk.id),
        &steward.token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        store
            .list_webauthn_credentials("dev", &s.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn sessions_sign_out_one_others_or_all_and_never_another_persons() {
    let (app, store) = app().await;
    let a = sign_in(&app, &store, MEMBER, MEMBER_PW).await;
    let b = sign_in(&app, &store, MEMBER, MEMBER_PW).await;
    let c = sign_in(&app, &store, MEMBER, MEMBER_PW).await;
    let steward = sign_in(&app, &store, STEWARD, STEWARD_PW).await;

    let (st, list) = rest(&app, Method::GET, "/sessions", &a.token, None).await;
    assert_eq!(st, StatusCode::OK);
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 3);
    assert!(list.iter().all(|s| s["id"] != steward.sid.as_str()));
    let current: Vec<_> = list.iter().filter(|s| s["current"] == true).collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0]["id"], a.sid.as_str());
    assert_eq!(current[0]["clients"][0]["clientId"], "account-console");
    // The device is named from the User-Agent the browser signed in with.
    assert_eq!(current[0]["browser"], "Firefox");
    assert_eq!(current[0]["os"], "Linux");

    // Another person's session id: 404, it stays alive.
    let (st, _) = rest(
        &app,
        Method::DELETE,
        &format!("/sessions/{}", steward.sid),
        &a.token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(
        store
            .get_sso_session("dev", &steward.sid)
            .await
            .unwrap()
            .is_some()
    );

    // One of my own.
    let (st, _) = rest(
        &app,
        Method::DELETE,
        &format!("/sessions/{}", b.sid),
        &a.token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        store
            .get_sso_session("dev", &b.sid)
            .await
            .unwrap()
            .is_none()
    );
    let (st, _) = rest(&app, Method::GET, "/", &b.token, None).await;
    assert_eq!(
        st,
        StatusCode::UNAUTHORIZED,
        "the signed-out device's token is dead"
    );

    // All others: this one stays.
    let (st, _) = rest(&app, Method::DELETE, "/sessions", &a.token, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        store
            .get_sso_session("dev", &c.sid)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_sso_session("dev", &a.sid)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .get_sso_session("dev", &steward.sid)
            .await
            .unwrap()
            .is_some()
    );

    // All, this one included.
    let (st, _) = rest(
        &app,
        Method::DELETE,
        "/sessions?current=true",
        &a.token,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        store
            .get_sso_session("dev", &a.sid)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_sso_session("dev", &steward.sid)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn update_password_is_an_application_initiated_action_with_reauthentication() {
    let (app, store) = app().await;
    let me = sign_in(&app, &store, MEMBER, MEMBER_PW).await;
    let other = sign_in(&app, &store, MEMBER, MEMBER_PW).await;

    // An unknown action is refused before anything starts.
    let refused = authorize(&app, "&kc_action=delete_account", Some(&me.sso)).await;
    let (st, _, _) = *refused.err().unwrap();
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Within five minutes of signing in the action page comes straight away.
    let page = authorize(&app, "&kc_action=UPDATE_PASSWORD", Some(&me.sso))
        .await
        .ok()
        .unwrap();
    assert_eq!(page_id(&page.html), "update-password", "{}", page.html);
    assert!(page.html.contains("name=\"password_confirm\""));
    assert!(page.html.contains("name=\"logout-sessions\""));
    assert!(
        page.html.contains("kc_action_status=cancelled"),
        "cancel goes back unchanged"
    );

    let (_, _, html) = page
        .post(
            &app,
            &[
                ("new_password", "BrandNewPass123!x"),
                ("password_confirm", "Different123!xx"),
            ],
        )
        .await;
    assert_eq!(page_id(&html), "update-password");
    // The mismatch is shown on the confirmation field, not as a banner.
    assert!(
        html.contains("id=\"input-error-password_confirm\""),
        "{html}"
    );
    assert!(!html.contains("input-error-new_password"));

    let (st, h, html) = page
        .post(
            &app,
            &[
                ("new_password", "BrandNewPass123!x"),
                ("password_confirm", "BrandNewPass123!x"),
                ("logout-sessions", "on"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{html}");
    assert_eq!(
        param(&location(&h), "kc_action_status").as_deref(),
        Some("success")
    );
    // Other devices are signed out; this browser's session stays.
    assert!(
        store
            .get_sso_session("dev", &other.sid)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_sso_session("dev", &me.sid)
            .await
            .unwrap()
            .is_some()
    );
    let page = authorize(&app, "", None).await.ok().unwrap();
    let (_, _, html) = page
        .post(&app, &[("username", MEMBER), ("password", MEMBER_PW)])
        .await;
    assert_eq!(page_id(&html), "login", "the old password no longer works");
    let page = authorize(&app, "", None).await.ok().unwrap();
    let (st, _, _) = page
        .post(
            &app,
            &[("username", MEMBER), ("password", "BrandNewPass123!x")],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);

    // An older session signs in again first, the person's name filled in.
    let mut sso = store
        .get_sso_session("dev", &me.sid)
        .await
        .unwrap()
        .unwrap();
    sso.auth_time -= 600;
    store.upsert_sso_session(sso).await.unwrap();
    let page = authorize(&app, "&kc_action=UPDATE_PASSWORD", Some(&me.sso))
        .await
        .ok()
        .unwrap();
    assert_eq!(page_id(&page.html), "login", "{}", page.html);
    assert!(page.html.contains(&format!("value=\"{MEMBER}\"")));
    let (_, _, html) = page
        .post(
            &app,
            &[("username", MEMBER), ("password", "BrandNewPass123!x")],
        )
        .await;
    assert_eq!(page_id(&html), "update-password", "{html}");
    // max_age=0 forces the sign-in too.
    let mut sso = store
        .get_sso_session("dev", &me.sid)
        .await
        .unwrap()
        .unwrap();
    sso.auth_time = chrono::Utc::now().timestamp();
    store.upsert_sso_session(sso).await.unwrap();
    let page = authorize(&app, "&kc_action=UPDATE_PASSWORD&max_age=0", Some(&me.sso))
        .await
        .ok()
        .unwrap();
    assert_eq!(page_id(&page.html), "login");
}

#[tokio::test]
async fn configure_totp_adds_a_named_authenticator_and_reports_success() {
    let (app, store) = app().await;
    let me = sign_in(&app, &store, MEMBER, MEMBER_PW).await;
    let other = sign_in(&app, &store, MEMBER, MEMBER_PW).await;
    let page = authorize(&app, "&kc_action=CONFIGURE_TOTP", Some(&me.sso))
        .await
        .ok()
        .unwrap();
    assert_eq!(page_id(&page.html), "config-totp", "{}", page.html);
    assert!(page.html.contains("name=\"userLabel\""));
    assert!(page.html.contains("id=\"kc-totp-qr\"") && page.html.contains("<svg"));
    // The key stands in its own block, apart from the step that asks for the code.
    assert!(page.html.contains("id=\"kc-totp-secret-key\""));
    assert!(page.html.contains("id=\"kc-totp-steps\""));
    assert!(!page.html.contains("id=\"instruction1\""));
    let secret = store
        .get_auth_session_by_id("dev", &page.session)
        .await
        .unwrap()
        .unwrap()
        .pending_totp_secret
        .unwrap();
    let cred = CredentialRecord {
        secret_data: json!({ "value": secret }).to_string(),
        credential_data:
            r#"{"digits":6,"period":30,"algorithm":"HmacSHA256","secretEncoding":"BASE32"}"#.into(),
        ..Default::default()
    };
    // A wrong code names the code field, not the password.
    let (_, _, html) = page.post(&app, &[("otp", "000000")]).await;
    assert_eq!(page_id(&html), "config-totp");
    assert!(html.contains("id=\"input-error-otp\""), "{html}");
    assert!(html.contains("Invalid authenticator code"));
    assert!(!html.contains("e-mail or password"));
    let code =
        generate_totp_code_for_credential(&cred, chrono::Utc::now().timestamp() as u64).unwrap();
    let (st, h, html) = page
        .post(
            &app,
            &[("otp", &code), ("userLabel", " Work phone\u{0007} ")],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{html}");
    assert_eq!(
        param(&location(&h), "kc_action_status").as_deref(),
        Some("success")
    );
    let fresh = finish(&app, &h, Some(&me.sso)).await;
    assert_eq!(decode_jwt_payload(&fresh.token)["acr"], "mfa");
    // Without the box, other devices stay signed in.
    assert!(
        store
            .get_sso_session("dev", &other.sid)
            .await
            .unwrap()
            .is_some()
    );
    let (_, groups) = rest(&app, Method::GET, "/credentials", &fresh.token, None).await;
    let otp = groups
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["type"] == "otp")
        .unwrap();
    assert_eq!(
        otp["userCredentialMetadatas"][0]["credential"]["userLabel"],
        "Work phone"
    );

    // A person who already has one adds another (Keycloak allows several).
    let page = authorize(&app, "&kc_action=CONFIGURE_TOTP", Some(&fresh.sso))
        .await
        .ok()
        .unwrap();
    assert_eq!(page_id(&page.html), "config-totp");
}
