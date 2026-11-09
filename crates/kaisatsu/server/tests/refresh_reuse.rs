#![forbid(unsafe_code)]
//! Refresh token reuse, as Keycloak 26.8 with `refreshTokenMaxReuse` 0: a handle presented after
//! it was rotated away (a later request, or a parallel one that lost the rotation) revokes its
//! token family, refresh and access tokens alike, and keeps the SSO session, so the client signs
//! the person in again without a password (APISIX renews in parallel, DESIGN S3b). Every case
//! runs on the memory store and on PostgreSQL (`KAISATSU_TEST_DATABASE_URL`, else `SKIP`).

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
use http_body_util::BodyExt;
use jc_router::Mount;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_server::router;
use kaisatsu_store_memory::MemoryStore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use tower::ServiceExt;

mod common;
use common::*;
#[path = "../../store-pg/tests/common/mod.rs"]
mod pgdb;

const AUTH: &str = "/realms/dev/protocol/openid-connect/auth";
const TOKEN: &str = "/realms/dev/protocol/openid-connect/token";
const TOKEN_URL: &str = "https://idm.dev.example.test/realms/dev/protocol/openid-connect/token";
const INTROSPECT: &str = "/realms/dev/protocol/openid-connect/token/introspect";
const USERINFO: &str = "/realms/dev/protocol/openid-connect/userinfo";
const CALLBACK: &str = "https://portal.dev.example.test/callback";
const VERIFIER: &str = "pkce-verifier-string-with-at-least-43-characters-long-012345";
/// The test client that exchanges the person's token (its id is in the token's `aud`).
const EXCHANGER: &str = "tx";

struct World {
    app: Router,
    store: Arc<dyn Store>,
}

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", "idm.dev.example.test", "").unwrap()
}

/// The `ri_dev` realm plus a token-exchange client `tx` named in `edge`'s and `portal-ui`'s
/// access tokens.
async fn world_on(store: Arc<dyn Store>, kek: Kek) -> World {
    let state = setup_dev_realm_on(store.clone(), kek).await;
    store
        .upsert_client(ClientRepresentation {
            id: "uuid-tx".into(),
            realm_id: "dev".into(),
            client_id: EXCHANGER.into(),
            enabled: true,
            client_authenticator_type: "client-secret".into(),
            secret: Some("tx-secret".into()),
            attributes: BTreeMap::from([(
                "standard.token.exchange.enabled".into(),
                "true".into(),
            )]),
            ..Default::default()
        })
        .await
        .unwrap();
    for id in ["edge", "portal-ui"] {
        let mut c = store
            .get_client_by_client_id("dev", id)
            .await
            .unwrap()
            .unwrap();
        c.protocol_mappers.push(ProtocolMapperRepresentation {
            id: format!("aud-tx-{id}"),
            name: "aud-tx".into(),
            protocol: "openid-connect".into(),
            protocol_mapper: "oidc-audience-mapper".into(),
            config: BTreeMap::from([
                ("included.client.audience".into(), EXCHANGER.into()),
                ("access.token.claim".into(), "true".into()),
            ]),
            ..Default::default()
        });
        store.upsert_client(c).await.unwrap();
    }
    World {
        app: router(state, mount()),
        store,
    }
}

async fn memory_world() -> World {
    let kek = Kek::generate_for_tests();
    world_on(Arc::new(MemoryStore::new(kek.clone())), kek).await
}

/// Runs `$case` on the memory store and on a throwaway PostgreSQL database.
macro_rules! on_both_stores {
    ($name:ident, $case:ident) => {
        mod $name {
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn memory() {
                super::$case(super::memory_world().await).await;
            }

            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn postgres() {
                let Some(db) = super::pgdb::test_db(stringify!($name)).await else {
                    return;
                };
                let kek = super::Kek::generate_for_tests();
                let store = std::sync::Arc::new(db.store(kek.clone()).await);
                super::$case(super::world_on(store, kek).await).await;
                db.drop_db().await;
            }
        }
    };
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let st = resp.status();
    let h = resp.headers().clone();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (st, h, String::from_utf8_lossy(&body).into_owned())
}

async fn get(app: &Router, uri: &str, cookie: Option<&str>) -> (StatusCode, HeaderMap, String) {
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    send(app, b.body(Body::empty()).unwrap()).await
}

/// A form POST to the token endpoint, with a DPoP proof when given.
async fn token(app: &Router, form: &[(&str, &str)], dpop: Option<&str>) -> (StatusCode, Value) {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();
    let mut b = Request::builder()
        .method(Method::POST)
        .uri(TOKEN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(p) = dpop {
        b = b.header("dpop", p);
    }
    let (st, _, body) = send(app, b.body(Body::from(body)).unwrap()).await;
    (st, serde_json::from_str(&body).unwrap_or(Value::Null))
}

fn location(h: &HeaderMap) -> String {
    h.get(header::LOCATION)
        .expect("location header")
        .to_str()
        .unwrap()
        .to_owned()
}

fn query_param(url: &str, name: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

fn cookie(h: &HeaderMap, name: &str) -> String {
    h.get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with(&format!("{name}=")))
        .map(|v| v.split(';').next().unwrap().to_owned())
        .unwrap_or_else(|| panic!("{name} cookie"))
}

fn auth_query(client_id: &str) -> String {
    format!(
        "{AUTH}?client_id={client_id}&redirect_uri={}&response_type=code&scope=openid&state=st-1&nonce=n-1&code_challenge={}&code_challenge_method=S256",
        url::form_urlencoded::byte_serialize(CALLBACK.as_bytes()).collect::<String>(),
        sha256_base64url(VERIFIER),
    )
}

/// A DPoP key holder (RFC 9449).
struct Holder {
    jwk: Value,
    key: EncodingKey,
}

fn holder() -> Holder {
    let kek = Kek::generate_for_tests();
    let rec = KeyManager::generate_es256("holder", &kek, 100, "ACTIVE").unwrap();
    let der = kek
        .decrypt(
            format!("holder:{}", rec.kid).as_bytes(),
            &rec.encrypted_private_key,
        )
        .unwrap();
    Holder {
        jwk: serde_json::from_str(&rec.public_jwk_json).unwrap(),
        key: EncodingKey::from_ec_der(&der),
    }
}

fn proof(h: &Holder) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.typ = Some("dpop+jwt".into());
    header.jwk = Some(serde_json::from_value(h.jwk.clone()).unwrap());
    let claims = json!({ "jti": uuid::Uuid::new_v4().to_string(), "htm": "POST",
                         "htu": TOKEN_URL, "iat": chrono::Utc::now().timestamp() });
    encode(&header, &claims, &h.key).unwrap()
}

fn thumbprint(jwk: &Value) -> String {
    let canonical = format!(
        r#"{{"crv":"{}","kty":"EC","x":"{}","y":"{}"}}"#,
        jwk["crv"].as_str().unwrap(),
        jwk["x"].as_str().unwrap(),
        jwk["y"].as_str().unwrap()
    );
    URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes()))
}

/// How a client authenticates at the token endpoint: `edge` with its secret, `portal-ui` as a
/// public client (optionally binding its tokens with DPoP).
#[derive(Clone, Copy)]
enum Who<'a> {
    Edge,
    Public(Option<&'a Holder>),
}

impl Who<'_> {
    fn client_id(self) -> &'static str {
        match self {
            Who::Edge => "edge",
            Who::Public(_) => "portal-ui",
        }
    }

    async fn post(self, app: &Router, form: &[(&str, &str)]) -> (StatusCode, Value) {
        let mut f = form.to_vec();
        f.push(("client_id", self.client_id()));
        let p = match self {
            Who::Edge => {
                f.push(("client_secret", "edge-secret-123"));
                None
            }
            Who::Public(holder) => holder.map(proof),
        };
        token(app, &f, p.as_deref()).await
    }

    async fn refresh(self, app: &Router, rt: &str) -> (StatusCode, Value) {
        self.post(
            app,
            &[("grant_type", "refresh_token"), ("refresh_token", rt)],
        )
        .await
    }
}

/// A person's browser session: the SSO cookie and the first tokens of `who`.
struct Session {
    sso: String,
    tokens: Value,
}

/// Logs `demo.orgmember@hel.fi` in through the login form and exchanges the code.
async fn sign_in(app: &Router, who: Who<'_>) -> Session {
    let (st, h, _) = get(app, &auth_query(who.client_id()), None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let login = location(&h);
    let auth = cookie(&h, "__Host-wk_auth");
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
    let sso = cookie(&h, "__Host-wk_sso");
    let code = query_param(&location(&h), "code").expect("code");
    let (st, tokens) = who
        .post(
            app,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", CALLBACK),
                ("code_verifier", VERIFIER),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{tokens}");
    Session { sso, tokens }
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_else(|| panic!("{key} in {v}"))
}

fn family_id(rt: &str) -> &str {
    rt.split_once('.').expect("handle has a family id").0
}

async fn family(w: &World, rt: &str) -> RefreshTokenFamily {
    w.store
        .get_refresh_family("dev", family_id(rt))
        .await
        .unwrap()
        .expect("family stored")
}

/// The SSO session lives: a new authorization request with its cookie returns a code with no
/// login form.
async fn assert_silent_sign_in(w: &World, s: &Session, client_id: &str) {
    let (st, h, body) = get(&w.app, &auth_query(client_id), Some(&s.sso)).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    let loc = location(&h);
    assert!(loc.starts_with(CALLBACK), "login form instead of a code: {loc}");
    assert!(query_param(&loc, "code").is_some(), "{loc}");
}

async fn introspect(w: &World, at: &str) -> Value {
    let (st, _, body) = post_form(
        &w.app,
        INTROSPECT,
        &[
            ("client_id", "edge"),
            ("client_secret", "edge-secret-123"),
            ("token", at),
        ],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    serde_json::from_str(&body).unwrap()
}

async fn userinfo(w: &World, at: &str) -> StatusCode {
    let req = Request::builder()
        .method(Method::GET)
        .uri(USERINFO)
        .header(header::AUTHORIZATION, format!("Bearer {at}"))
        .body(Body::empty())
        .unwrap();
    send(&w.app, req).await.0
}

async fn exchange(w: &World, at: &str) -> (StatusCode, Value) {
    token(
        &w.app,
        &[
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            ("client_id", EXCHANGER),
            ("client_secret", "tx-secret"),
            ("subject_token", at),
        ],
        None,
    )
    .await
}

/// Signs in with `who`, rotates once (the first handle is now rotated away) and presents the
/// first handle again. Returns the session, the access tokens of both grants and the handles.
async fn reuse_once(w: &World, who: Who<'_>) -> (Session, [String; 2], [String; 2]) {
    let s = sign_in(&w.app, who).await;
    let rt1 = str_of(&s.tokens, "refresh_token").to_owned();
    let at1 = str_of(&s.tokens, "access_token").to_owned();
    let (st, rotated) = who.refresh(&w.app, &rt1).await;
    assert_eq!(st, StatusCode::OK, "{rotated}");
    let rt2 = str_of(&rotated, "refresh_token").to_owned();
    let at2 = str_of(&rotated, "access_token").to_owned();
    // Live before the reuse.
    assert_eq!(introspect(w, &at2).await["active"], true);
    let (st, body) = who.refresh(&w.app, &rt1).await;
    assert_eq!((st, str_of(&body, "error")), (StatusCode::BAD_REQUEST, "invalid_grant"));
    (s, [at1, at2], [rt1, rt2])
}

/// The family is revoked (its newest handle refused too) and the SSO session stays.
async fn assert_family_dead_session_alive(w: &World, who: Who<'_>, s: &Session, newest: &str) {
    let fam = family(w, newest).await;
    assert!(fam.revoked);
    assert!(
        w.store
            .get_sso_session("dev", &fam.sso_session_id)
            .await
            .unwrap()
            .is_some(),
        "the SSO session stays"
    );
    let (st, body) = who.refresh(&w.app, newest).await;
    assert_eq!((st, str_of(&body, "error")), (StatusCode::BAD_REQUEST, "invalid_grant"));
    assert_silent_sign_in(w, s, who.client_id()).await;
}

async fn parallel_renewals(w: World) {
    let s = sign_in(&w.app, Who::Edge).await;
    let rt = str_of(&s.tokens, "refresh_token").to_owned();
    let racers = 10;
    let start = Arc::new(tokio::sync::Barrier::new(racers));
    let mut handles = Vec::new();
    for _ in 0..racers {
        let (app, rt, start) = (w.app.clone(), rt.clone(), start.clone());
        handles.push(tokio::spawn(async move {
            start.wait().await;
            Who::Edge.refresh(&app, &rt).await
        }));
    }
    let mut winners = Vec::new();
    for h in handles {
        let (st, body) = h.await.unwrap();
        if st == StatusCode::OK {
            winners.push(body);
        } else {
            assert_eq!((st, str_of(&body, "error")), (StatusCode::BAD_REQUEST, "invalid_grant"));
        }
    }
    assert_eq!(winners.len(), 1, "exactly one request rotates: {winners:?}");
    let won = &winners[0];
    let newest = str_of(won, "refresh_token");
    assert_ne!(newest, rt);
    // The losers reused the handle: the family is dead, its access tokens with it, the
    // person's SSO session lives on.
    assert_family_dead_session_alive(&w, Who::Edge, &s, newest).await;
    for at in [str_of(&s.tokens, "access_token"), str_of(won, "access_token")] {
        assert_eq!(introspect(&w, at).await["active"], false);
    }
}
on_both_stores!(ten_parallel_renewals_leave_one_winner_and_the_sso_session, parallel_renewals);

async fn reuse_inactive_at_introspection(w: World) {
    let (s, ats, rts) = reuse_once(&w, Who::Edge).await;
    assert_family_dead_session_alive(&w, Who::Edge, &s, &rts[1]).await;
    for at in &ats {
        assert_eq!(introspect(&w, at).await["active"], false, "{at}");
    }
    // The person signs in again silently and the new family's token is live.
    let again = sign_in(&w.app, Who::Edge).await;
    assert_eq!(
        introspect(&w, str_of(&again.tokens, "access_token")).await["active"],
        true
    );
}
on_both_stores!(an_access_token_of_a_reused_family_is_inactive_at_introspection, reuse_inactive_at_introspection);

async fn reuse_refused_at_userinfo(w: World) {
    let s = sign_in(&w.app, Who::Edge).await;
    let control = str_of(&s.tokens, "access_token").to_owned();
    assert_eq!(userinfo(&w, &control).await, StatusCode::OK);
    let (_, ats, _) = reuse_once(&w, Who::Edge).await;
    for at in &ats {
        assert_eq!(userinfo(&w, at).await, StatusCode::UNAUTHORIZED, "{at}");
    }
    // Another family of the same person and client is untouched.
    assert_eq!(userinfo(&w, &control).await, StatusCode::OK);
}
on_both_stores!(an_access_token_of_a_reused_family_is_rejected_at_userinfo, reuse_refused_at_userinfo);

async fn reuse_refused_at_exchange(w: World) {
    let s = sign_in(&w.app, Who::Edge).await;
    let (st, body) = exchange(&w, str_of(&s.tokens, "access_token")).await;
    assert_eq!(st, StatusCode::OK, "the control exchange: {body}");
    let (_, ats, _) = reuse_once(&w, Who::Edge).await;
    for at in &ats {
        let (st, body) = exchange(&w, at).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    }
}
on_both_stores!(an_access_token_of_a_reused_family_is_refused_at_token_exchange, reuse_refused_at_exchange);

async fn two_rotations_old(w: World) {
    let s = sign_in(&w.app, Who::Edge).await;
    let rt1 = str_of(&s.tokens, "refresh_token").to_owned();
    let (_, second) = Who::Edge.refresh(&w.app, &rt1).await;
    let rt2 = str_of(&second, "refresh_token").to_owned();
    let (st, third) = Who::Edge.refresh(&w.app, &rt2).await;
    assert_eq!(st, StatusCode::OK, "{third}");
    let rt3 = str_of(&third, "refresh_token");
    let (st, body) = Who::Edge.refresh(&w.app, &rt1).await;
    assert_eq!((st, str_of(&body, "error")), (StatusCode::BAD_REQUEST, "invalid_grant"));
    assert_family_dead_session_alive(&w, Who::Edge, &s, rt3).await;
    assert_eq!(introspect(&w, str_of(&third, "access_token")).await["active"], false);
}
on_both_stores!(a_handle_two_rotations_old_revokes_the_family_too, two_rotations_old);

async fn public_client(w: World) {
    let (s, ats, rts) = reuse_once(&w, Who::Public(None)).await;
    assert_family_dead_session_alive(&w, Who::Public(None), &s, &rts[1]).await;
    for at in &ats {
        assert_eq!(introspect(&w, at).await["active"], false);
    }
}
on_both_stores!(a_public_client_family_is_revoked_the_same_way, public_client);

async fn dpop_bound(w: World) {
    let h = holder();
    let who = Who::Public(Some(&h));
    let (s, ats, rts) = reuse_once(&w, who).await;
    assert_eq!(
        family(&w, &rts[1]).await.dpop_jkt.as_deref(),
        Some(thumbprint(&h.jwk).as_str()),
        "a DPoP-bound family"
    );
    assert_family_dead_session_alive(&w, who, &s, &rts[1]).await;
    for at in &ats {
        assert_eq!(introspect(&w, at).await["active"], false);
    }
}
on_both_stores!(a_dpop_bound_family_is_revoked_the_same_way, dpop_bound);
