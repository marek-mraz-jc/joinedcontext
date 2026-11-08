#![forbid(unsafe_code)]
//! The Portal's calls to its realm, replayed against kaisatsu (crates/kaisatsu/PORTAL-CALLS.md).
//!
//! The realm is the shipped `import/realms/jc-portal.json` plus a test person. Each call is sent
//! the way the Portal sends it (`crates/portal/src`, file:line in the comments), and its answer
//! is read the way the Portal reads it: discovery, the ID token and the logout token through the
//! Portal's own `openidconnect` 4, bearer tokens through `jsonwebtoken` 9 with the checks of
//! `auth/bearer.rs`, admin answers through the fields the Portal deserializes.

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::{Extension, Router};
use http_body_util::BodyExt;
use jc_router::Mount;
use kaisatsu_core::*;
use kaisatsu_crypto::*;
use kaisatsu_import::{ApplyMode, ApplyOptions, apply_realm_files};
use kaisatsu_oidc::OutboundGuard;
use kaisatsu_server::{KaisatsuState, router};
use kaisatsu_store_memory::MemoryStore;
use openidconnect::core::{
    CoreAuthDisplay, CoreAuthenticationFlow, CoreClaimName, CoreClaimType, CoreClient,
    CoreClientAuthMethod, CoreGrantType, CoreIdToken, CoreJsonWebKey,
    CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm, CoreResponseMode,
    CoreResponseType, CoreSubjectIdentifierType, CoreTokenResponse,
};
use openidconnect::{
    AdditionalProviderMetadata, ClientId, CsrfToken, IssuerUrl, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge,
    ProviderMetadata, RedirectUrl, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use tower::ServiceExt;

const HOST: &str = "idm.jc.test";
const ISSUER: &str = "https://idm.jc.test/realms/jc";
const ADMIN: &str = "/admin/realms/jc";
const TOKEN: &str = "/realms/jc/protocol/openid-connect/token";
/// The Portal's public address in the shipped realm (`JC_PORTAL_PUBLIC_URL`).
const PORTAL: &str = "https://portal.jc.localhost";
const PORTAL_API_SECRET: &str = "portal-api-secret-for-tests-0123456789";
const RECONCILER_SECRET: &str = "portal-reconciler-secret-for-tests-0123";
const ANA: &str = "ana@hel.example";
const ANA_PASSWORD: &str = "AnaPass123!Valid";
/// An Endpoint slug of the opaque alphabet the hub scopes take (reconciler/hub_scopes.rs:96).
const SLUG: &str = "counterscounterscounters12";
/// A connector id: 26 letters of `[a-z2-7]`, the realm's `connector` scope regexp.
const CONNECTOR: &str = "abcdefghijklmnopqrstuvwxyz";
/// `reconciler/groups.rs` `MANAGED_BY` / `MANAGED_VALUE`, which every wave writes.
const MANAGED: (&str, &str) = ("managed-by", "joinedcontext");

// ------------------------------------------------------------------------------------------
// The realm
// ------------------------------------------------------------------------------------------

fn mount() -> Mount {
    Mount::new("kaisatsu", "https", HOST, "").unwrap()
}

/// The shipped realm file, read from where the first-start setup imports it.
fn shipped_realm() -> Value {
    serde_json::from_str(include_str!("../../import/realms/jc-portal.json"))
        .expect("jc-portal.json parses")
}

/// What an installation adds after the import: one Organization's groups and one person in
/// `/helsinki/members`, whose group attributes give the `jc_org` and `jc_groups` claims
/// (ADR-N-067 section 2.2).
fn test_people() -> Value {
    json!({
        "realm": "jc",
        "groups": [{
            "name": "helsinki", "path": "/helsinki",
            "subGroups": [{
                "name": "members", "path": "/helsinki/members",
                "attributes": { "jc_org": ["helsinki"], "jc_groups": ["/helsinki/members"] }
            }]
        }],
        "users": [{
            "username": ANA, "email": ANA, "firstName": "Ana", "lastName": "Virtanen",
            "enabled": true, "emailVerified": true,
            "credentials": [{ "type": "password", "value": "$(USER_PASSWORD_ANA)", "temporary": false }],
            "groups": ["/helsinki/members"]
        }]
    })
}

struct Realm {
    app: Router,
    store: Arc<MemoryStore>,
    state: KaisatsuState,
    kek: Kek,
}

async fn realm() -> Realm {
    let kek = Kek::generate_for_tests();
    let store = Arc::new(MemoryStore::new(kek.clone()));
    let opts = ApplyOptions {
        mode: ApplyMode::NoDelete,
        dry_run: false,
        allow_literal_secrets: false,
        secrets_dir: None,
        env_vars: BTreeMap::from([
            ("CLIENT_SECRET_PORTAL_API".into(), PORTAL_API_SECRET.into()),
            (
                "CLIENT_SECRET_PORTAL_RECONCILER".into(),
                RECONCILER_SECRET.into(),
            ),
            ("USER_PASSWORD_ANA".into(), ANA_PASSWORD.into()),
        ]),
    };
    let store_dyn: Arc<dyn Store> = store.clone();
    apply_realm_files(
        &store_dyn,
        &[
            ("10-jc-portal.json".into(), shipped_realm()),
            ("20-test-people.json".into(), test_people()),
        ],
        &opts,
    )
    .await
    .expect("the shipped realm and the test person apply");
    let state = KaisatsuState::new(
        store_dyn,
        kek.clone(),
        OutboundGuard::default(),
        Arc::new(LocalAuthorizer),
    );
    let app = router(state.clone(), mount()).layer(Extension(mount()));
    Realm {
        app,
        store,
        state,
        kek,
    }
}

// ------------------------------------------------------------------------------------------
// HTTP helpers
// ------------------------------------------------------------------------------------------

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    text: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.text))
    }

    fn location(&self) -> String {
        self.headers
            .get(header::LOCATION)
            .unwrap_or_else(|| panic!("no Location ({}): {}", self.status, self.text))
            .to_str()
            .unwrap()
            .to_owned()
    }

    fn cookie(&self, prefix: &str) -> Option<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.starts_with(prefix))
            .map(|v| v.split(';').next().unwrap().to_owned())
    }
}

/// A path relative to the realm host; an absolute `https://idm.jc.test/...` is cut to its path.
fn local(uri: &str) -> String {
    uri.strip_prefix(&format!("https://{HOST}"))
        .unwrap_or(uri)
        .to_owned()
}

async fn send(app: &Router, request: Request<Body>) -> Answer {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Answer {
        status,
        headers,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// An admin API call as `Admin::send` (reconciler/app_clients.rs:517) and `people.rs`
/// `Admin::send` (people.rs:262) make it: bearer token, JSON body.
async fn admin(app: &Router, token: &str, method: Method, path: &str, body: Option<Value>) -> Answer {
    let mut b = Request::builder()
        .method(method)
        .uri(format!("{ADMIN}{path}"))
        .header(header::HOST, HOST)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    send(app, b.body(body).unwrap()).await
}

async fn get_page(app: &Router, uri: &str, cookie: Option<&str>) -> Answer {
    let mut b = Request::builder()
        .method(Method::GET)
        .uri(local(uri))
        .header(header::HOST, HOST);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    send(app, b.body(Body::empty()).unwrap()).await
}

async fn post_form(app: &Router, uri: &str, pairs: &[(&str, &str)], cookie: Option<&str>) -> Answer {
    let encoded = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    let mut b = Request::builder()
        .method(Method::POST)
        .uri(local(uri))
        .header(header::HOST, HOST)
        .header(header::ACCEPT, "application/json")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    send(app, b.body(Body::from(encoded)).unwrap()).await
}

fn hidden(html: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    let start = html
        .find(&marker)
        .unwrap_or_else(|| panic!("no hidden {name}: {html}"))
        + marker.len();
    let end = html[start..].find('"').unwrap() + start;
    html[start..end].to_owned()
}

fn param(url: &str, name: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let from_fragment: Vec<(String, String)> = u
        .fragment()
        .map(|f| url::form_urlencoded::parse(f.as_bytes()).into_owned().collect())
        .unwrap_or_default();
    u.query_pairs()
        .into_owned()
        .chain(from_fragment)
        .find(|(k, _)| k == name)
        .map(|(_, v)| v)
}

fn payload(jwt: &str) -> Value {
    use base64::engine::{Engine, general_purpose::URL_SAFE_NO_PAD};
    let part = jwt.split('.').nth(1).expect("a JWT");
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
}

/// The browser's part of an authorization request: the realm's login form for Ana, submitted.
/// Answers the redirect back to the client and the SSO cookie.
async fn sign_in(app: &Router, authorize: &str) -> (String, String) {
    let first = get_page(app, authorize, None).await;
    assert_eq!(first.status, StatusCode::SEE_OTHER, "{}", first.text);
    let auth_cookie = first.cookie("__Host-wk_auth=").expect("auth session cookie");
    let login = first.location();
    let page = get_page(app, &login, Some(&auth_cookie)).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text);
    let done = post_form(
        app,
        &login,
        &[
            ("csrf_token", &hidden(&page.text, "csrf_token")),
            ("session_code", &hidden(&page.text, "session_code")),
            ("username", ANA),
            ("password", ANA_PASSWORD),
        ],
        Some(&auth_cookie),
    )
    .await;
    assert_eq!(done.status, StatusCode::SEE_OTHER, "{}", done.text);
    (
        done.location(),
        done.cookie("__Host-wk_sso=").expect("sso cookie"),
    )
}

/// A client-credentials token as `Admin::token` (reconciler/app_clients.rs:493) and
/// `People::admin` (people.rs:196) ask it: `ClientAuth::Secret` puts id and secret in the body
/// (auth/client_auth.rs:115).
async fn service_token(app: &Router, client_id: &str, secret: &str) -> String {
    let answer = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            ("client_secret", secret),
        ],
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    answer.json()["access_token"]
        .as_str()
        .expect("access_token")
        .to_owned()
}

async fn reconciler(app: &Router) -> String {
    service_token(app, "portal-reconciler", RECONCILER_SECRET).await
}

async fn portal_api(app: &Router) -> String {
    service_token(app, "portal-api", PORTAL_API_SECRET).await
}

// ------------------------------------------------------------------------------------------
// The Portal's own readers
// ------------------------------------------------------------------------------------------

/// auth/oidc.rs:34 `LogoutMetadata`.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct LogoutMetadata {
    #[serde(default)]
    end_session_endpoint: Option<String>,
}

impl AdditionalProviderMetadata for LogoutMetadata {}

/// auth/oidc.rs:42 `PortalProviderMetadata`.
type PortalProviderMetadata = ProviderMetadata<
    LogoutMetadata,
    CoreAuthDisplay,
    CoreClientAuthMethod,
    CoreClaimName,
    CoreClaimType,
    CoreGrantType,
    CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm,
    CoreJsonWebKey,
    CoreResponseMode,
    CoreResponseType,
    CoreSubjectIdentifierType,
>;

/// The Portal's discovery (auth/oidc.rs:220) over the router instead of the network.
async fn discover(app: &Router) -> PortalProviderMetadata {
    let app = app.clone();
    let http = move |request: openidconnect::HttpRequest| {
        let app = app.clone();
        async move {
            let (parts, body) = request.into_parts();
            let mut b = Request::builder()
                .method(parts.method)
                .uri(local(&parts.uri.to_string()))
                .header(header::HOST, HOST);
            for (name, value) in &parts.headers {
                b = b.header(name, value);
            }
            let response = app
                .oneshot(b.body(Body::from(body)).unwrap())
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let (parts, body) = response.into_parts();
            let bytes = body
                .collect()
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?
                .to_bytes();
            Ok::<_, std::io::Error>(openidconnect::HttpResponse::from_parts(
                parts,
                bytes.to_vec(),
            ))
        }
    };
    PortalProviderMetadata::discover_async(IssuerUrl::new(ISSUER.into()).unwrap(), &http)
        .await
        .expect("the Portal's discovery accepts the realm")
}

/// `auth/bearer.rs` `BearerVerifier`: the realm's JWKS as jsonwebtoken 9 reads it, ES256 and
/// RS256 keys alone (bearer.rs:37), then `claims_of_type` (bearer.rs:266).
struct Bearer {
    keys: HashMap<String, jsonwebtoken9::DecodingKey>,
}

impl Bearer {
    async fn of(app: &Router) -> Self {
        let jwks = get_page(app, "/realms/jc/protocol/openid-connect/certs", None).await;
        assert_eq!(jwks.status, StatusCode::OK);
        let set: jsonwebtoken9::jwk::JwkSet = serde_json::from_str(&jwks.text)
            .unwrap_or_else(|e| panic!("bearer.rs:163 refuses the JWKS ({e}): {}", jwks.text));
        let keys: HashMap<_, _> = set
            .keys
            .iter()
            .filter(|jwk| match jwk.common.key_algorithm {
                Some(alg) => matches!(alg.to_string().as_str(), "ES256" | "RS256"),
                None => matches!(
                    jwk.algorithm,
                    jsonwebtoken9::jwk::AlgorithmParameters::EllipticCurve(_)
                        | jsonwebtoken9::jwk::AlgorithmParameters::RSA(_)
                ),
            })
            .filter_map(|jwk| {
                Some((
                    jwk.common.key_id.clone()?,
                    jsonwebtoken9::DecodingKey::from_jwk(jwk).ok()?,
                ))
            })
            .collect();
        assert!(!keys.is_empty(), "no ES256 or RS256 key the Portal can use");
        Self { keys }
    }

    /// `Ok(claims)` when the Portal takes `token` as a `typ` token for `audience`.
    fn verify(&self, token: &str, audience: &str, typ: &str) -> std::result::Result<Value, String> {
        use jsonwebtoken9::{Algorithm, Validation, decode, decode_header};
        let header = decode_header(token).map_err(|e| e.to_string())?;
        if ![Algorithm::ES256, Algorithm::RS256].contains(&header.alg) {
            return Err(format!("algorithm {:?} is refused", header.alg));
        }
        let kid = header.kid.ok_or("no kid")?;
        let key = self.keys.get(&kid).ok_or("unknown kid")?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[ISSUER]);
        validation.set_audience(&[audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.validate_nbf = true;
        validation.leeway = 30;
        let claims = decode::<Value>(token, key, &validation)
            .map_err(|e| e.to_string())?
            .claims;
        match claims["typ"].as_str() {
            Some(t) if t == typ => Ok(claims),
            other => Err(format!("typ {other:?}, the door takes {typ}")),
        }
    }
}

/// `reconciler/app_clients.rs:377` `drift` and `:405` `same`: what a run reports as changed in
/// the realm and writes back. Empty when the realm keeps what the Portal wrote.
fn drift(want: &Value, held: &Value) -> Vec<String> {
    fn strings(value: &Value) -> Option<BTreeSet<&str>> {
        value.as_array()?.iter().map(Value::as_str).collect()
    }
    let same = |held: Option<&Value>, want: &Value| match (held.and_then(strings), strings(want)) {
        (Some(h), Some(w)) => h == w,
        _ => held == Some(want),
    };
    let mut found = Vec::new();
    for (field, value) in want.as_object().unwrap() {
        if field == "attributes" {
            for (key, wanted) in value.as_object().unwrap() {
                let current = held.get("attributes").and_then(|a| a.get(key));
                if current != Some(wanted) {
                    found.push(format!("attribute {key}: {current:?} != {wanted}"));
                }
            }
            continue;
        }
        if !same(held.get(field), value) {
            found.push(format!("{field}: {:?} != {value}", held.get(field)));
        }
    }
    found
}

/// `reconciler/app_clients.rs:601` `Admin::find`: the first client of `GET /clients?clientId=`.
async fn find_client(app: &Router, token: &str, client_id: &str) -> Option<Value> {
    let listed = admin(
        app,
        token,
        Method::GET,
        &format!("/clients?clientId={client_id}"),
        None,
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text);
    listed.json().as_array().unwrap().first().cloned()
}

fn id_of(value: &Value) -> String {
    value["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no id: {value}"))
        .to_owned()
}

// ------------------------------------------------------------------------------------------
// Sign-in: discovery, code flow, refresh, silent check, logout (auth/oidc.rs)
// ------------------------------------------------------------------------------------------

/// The Portal's code flow end to end, each answer read by the Portal's own parser and verifier.
#[tokio::test]
async fn the_portal_signs_a_person_in_refreshes_and_signs_them_out() {
    let realm = realm().await;
    let app = &realm.app;

    // auth/oidc.rs:220 discovery, :223 end_session_endpoint, :229 token endpoint.
    let metadata = discover(app).await;
    let end_session = metadata
        .additional_metadata()
        .end_session_endpoint
        .clone()
        .expect("end_session_endpoint (auth/oidc.rs:223)");
    assert!(metadata.token_endpoint().is_some(), "token endpoint (auth/oidc.rs:229)");
    let redirect = format!("{PORTAL}/api/v1/auth/callback");
    let client = CoreClient::from_provider_metadata(
        metadata,
        ClientId::new("portal-api".into()),
        None,
    )
    .set_redirect_uri(RedirectUrl::new(redirect.clone()).unwrap());

    // auth/oidc.rs:596 login: profile and email, PKCE, nonce, state, ui_locales.
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (authorize, state, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("profile".into()))
        .add_scope(Scope::new("email".into()))
        .set_pkce_challenge(challenge)
        .add_extra_param("ui_locales", "sk")
        .url();
    let (back, sso) = sign_in(app, authorize.as_str()).await;
    assert!(back.starts_with(&redirect), "{back}");
    assert_eq!(param(&back, "state").as_deref(), Some(state.secret().as_str()));
    let code = param(&back, "code").expect("code");

    // auth/oidc.rs:760 the code exchange through `token_request` (auth/oidc.rs:289).
    let answer = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect),
            ("code_verifier", verifier.secret()),
            ("client_id", "portal-api"),
            ("client_secret", PORTAL_API_SECRET),
        ],
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let tokens: CoreTokenResponse = serde_json::from_str(&answer.text)
        .unwrap_or_else(|e| panic!("auth/oidc.rs:327 refuses the token response ({e}): {}", answer.text));
    let id_token = tokens.id_token().expect("an ID token").clone();
    // auth/oidc.rs:768: signature (RS256 alone, openidconnect's default), issuer, audience, nonce.
    let claims = id_token
        .claims(&client.id_token_verifier(), &nonce)
        .unwrap_or_else(|e| panic!("auth/oidc.rs:769 refuses the ID token: {e}"));
    let ana = realm
        .store
        .get_user_by_username_or_email("jc", None, ANA)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claims.subject().as_str(), ana.id);
    assert_eq!(
        claims.preferred_username().map(|u| u.as_str()),
        Some(ANA),
        "auth/oidc.rs:779 username"
    );
    assert_eq!(claims.email().map(|e| e.as_str()), Some(ANA));
    assert_eq!(claims.email_verified(), Some(true), "auth/session.rs verified_email");
    assert_eq!(
        claims.name().and_then(|n| n.get(None)).map(|n| n.as_str()),
        Some("Ana Virtanen")
    );
    // auth/oidc.rs:831 groups (full path, one leading `/` cut by the Portal), :859 realm roles
    // from the ID token (`realm-roles-in-id-token`), :848 acr and amr.
    let id_claims = payload(&id_token.to_string());
    assert_eq!(id_claims["groups"], json!(["/helsinki/members"]), "{id_claims}");
    assert!(
        id_claims["realm_access"]["roles"].is_array(),
        "realm_access.roles in the ID token: {id_claims}"
    );
    assert!(id_claims["amr"].is_array(), "{id_claims}");
    assert_eq!(id_claims["jc_org"], json!(["helsinki"]), "{id_claims}");
    assert_eq!(id_claims["jc_groups"], json!(["/helsinki/members"]), "{id_claims}");
    let refresh = tokens
        .refresh_token()
        .expect("a refresh token (auth/oidc.rs:449)")
        .secret()
        .clone();

    // auth/oidc.rs:390 refresh: a new ID token verified without a nonce (:410).
    let answer = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", "portal-api"),
            ("client_secret", PORTAL_API_SECRET),
        ],
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let fresh: CoreTokenResponse = serde_json::from_str(&answer.text).expect("token response");
    let fresh_id = fresh.id_token().expect("a refreshed ID token");
    fresh_id
        .claims(&client.id_token_verifier(), |_: Option<&Nonce>| Ok(()))
        .unwrap_or_else(|e| panic!("auth/oidc.rs:410 refuses the refreshed ID token: {e}"));

    // auth/oidc.rs:265 the silent check: `portal-ui`, prompt=none, response_mode=fragment, a
    // redirect to /api/v1/auth/sso-check/done. With the SSO cookie the realm hands a code back.
    let (sso_challenge, _) = PkceCodeChallenge::new_random_sha256();
    let check = |cookie: Option<String>| {
        let uri = format!(
            "/realms/jc/protocol/openid-connect/auth?client_id=portal-ui&redirect_uri={}\
             &response_type=code&response_mode=fragment&scope=openid&prompt=none\
             &code_challenge={}&code_challenge_method=S256",
            url::form_urlencoded::byte_serialize(
                format!("{PORTAL}/api/v1/auth/sso-check/done").as_bytes()
            )
            .collect::<String>(),
            sso_challenge.as_str(),
        );
        async move { get_page(app, &uri, cookie.as_deref()).await }
    };
    let live = check(Some(sso.clone())).await;
    assert_eq!(live.status, StatusCode::SEE_OTHER, "{}", live.text);
    let landed = live.location();
    assert!(landed.starts_with(&format!("{PORTAL}/api/v1/auth/sso-check/done#")), "{landed}");
    assert!(param(&landed, "code").is_some(), "{landed}");
    let none = check(None).await;
    assert_eq!(
        param(&none.location(), "error").as_deref(),
        Some("login_required"),
        "auth/oidc.rs:259 no realm session"
    );

    // auth/oidc.rs:964 RP-initiated logout: post_logout_redirect_uri is the public base
    // (`Url::as_str`, trailing slash), id_token_hint, client_id. The realm then posts the
    // back-channel logout to the Portal (auth/oidc.rs:1055).
    let backchannel = format!("{PORTAL}/api/v1/auth/backchannel-logout");
    realm.state.guard().insert_mock(&backchannel, 204, Vec::new());
    let mut logout = url::Url::parse(&end_session).unwrap();
    logout
        .query_pairs_mut()
        .append_pair("post_logout_redirect_uri", &format!("{PORTAL}/"))
        .append_pair("id_token_hint", &fresh_id.to_string())
        .append_pair("client_id", "portal-api");
    let out = get_page(app, logout.as_str(), Some(&sso)).await;
    assert_eq!(out.status, StatusCode::SEE_OTHER, "{}", out.text);
    assert_eq!(out.location(), format!("{PORTAL}/"));
    let posted = realm.state.guard().posted_forms(&backchannel);
    assert_eq!(posted.len(), 1, "one back-channel logout to the Portal");
    let logout_token = posted[0]
        .iter()
        .find(|(k, _)| k == "logout_token")
        .map(|(_, v)| v.clone())
        .expect("logout_token form field (auth/oidc.rs:517)");
    // auth/oidc.rs:1011 `ends_a_session`: the back-channel event, no nonce.
    let logout_claims = payload(&logout_token);
    assert!(
        logout_claims["events"]
            .get("http://schemas.openid.net/event/backchannel-logout")
            .is_some()
            && logout_claims.get("nonce").is_none(),
        "{logout_claims}"
    );
    // auth/oidc.rs:1069: parsed as an ID token and verified with the ID token verifier.
    let parsed: CoreIdToken = logout_token.parse().expect("auth/oidc.rs:1068 a JWT");
    let verified = parsed
        .claims(&client.id_token_verifier(), |_: Option<&Nonce>| Ok(()))
        .unwrap_or_else(|e| panic!("auth/oidc.rs:1069 refuses the logout token: {e}"));
    assert_eq!(verified.subject().as_str(), ana.id);
}
