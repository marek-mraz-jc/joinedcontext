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

// ------------------------------------------------------------------------------------------
// Bearer callers, connectors and the Endpoint token exchange (auth/bearer.rs, mcp/connector.rs,
// auth/oidc.rs:364)
// ------------------------------------------------------------------------------------------

/// A token of the public client `mcp-local` for `scope`, through the browser and PKCE, as a
/// connector or `jcctl` gets one.
async fn public_client_token(app: &Router, scope: &str) -> Value {
    let (pkce, verifier) = PkceCodeChallenge::new_random_sha256();
    let challenge = pkce.as_str();
    let verifier = verifier.secret();
    let redirect = "http://localhost/callback";
    let authorize = format!(
        "/realms/jc/protocol/openid-connect/auth?client_id=mcp-local&redirect_uri={}\
         &response_type=code&scope={}&state=s1&code_challenge={challenge}\
         &code_challenge_method=S256",
        url::form_urlencoded::byte_serialize(redirect.as_bytes()).collect::<String>(),
        url::form_urlencoded::byte_serialize(scope.as_bytes()).collect::<String>(),
    );
    let (back, _) = sign_in(app, &authorize).await;
    let code = param(&back, "code").unwrap_or_else(|| panic!("no code: {back}"));
    let answer = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", redirect),
            ("code_verifier", verifier),
            ("client_id", "mcp-local"),
        ],
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    answer.json()
}

/// A realm administrator's token, standing in for the deployment Job that links an Endpoint's
/// scope to `portal-api` (deployment components/admin-permissions; hub_scopes.rs:269 reads the
/// links and never writes them).
async fn realm_admin(realm: &Realm) -> String {
    let key = realm
        .store
        .list_signing_keys("jc")
        .await
        .unwrap()
        .into_iter()
        .find(|k| k.status == "ACTIVE")
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    sign_jwt(
        &key,
        &realm.kek,
        Some("JWT"),
        &json!({ "iss": ISSUER, "sub": "operator", "azp": "admin-cli", "typ": "Bearer",
                 "iat": now, "exp": now + 600, "realm_access": { "roles": ["realm-admin"] } }),
    )
    .unwrap()
}

/// The exchange of auth/oidc.rs:364 as `portal-api` asks it.
async fn exchange(app: &Router, subject: &str, slug: &str) -> Answer {
    let scope = format!("gateway-hub endpoint:{slug}");
    post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:token-exchange"),
            ("subject_token", subject),
            ("subject_token_type", "urn:ietf:params:oauth:token-type:access_token"),
            ("requested_token_type", "urn:ietf:params:oauth:token-type:access_token"),
            ("audience", "mcp-hub"),
            ("scope", &scope),
            ("client_id", "portal-api"),
            ("client_secret", PORTAL_API_SECRET),
        ],
        None,
    )
    .await
}

#[tokio::test]
async fn bearer_and_connector_tokens_verify_and_exchange_for_one_endpoint() {
    let realm = realm().await;
    let app = &realm.app;
    let bearer = Bearer::of(app).await;
    let ana = realm
        .store
        .get_user_by_username_or_email("jc", None, ANA)
        .await
        .unwrap()
        .unwrap();

    // A bearer caller of the Portal API (`mcp:portal`): auth/session.rs:385 `verify_with_client`
    // with the Portal's client id as audience (state.rs:207), typ Bearer, azp the caller.
    let mcp = public_client_token(app, "openid mcp:portal").await;
    let access = mcp["access_token"].as_str().unwrap();
    let claims = bearer
        .verify(access, "portal-api", "Bearer")
        .unwrap_or_else(|e| panic!("auth/bearer.rs:266 refuses an mcp:portal token: {e}"));
    assert_eq!(claims["sub"], json!(ana.id));
    assert_eq!(claims["azp"], json!("mcp-local"));
    assert_eq!(claims["groups"], json!(["/helsinki/members"]), "{claims}");
    // bearer.rs:203: a Portal API token holds no `connector:` scope.
    assert!(
        !claims["scope"]
            .as_str()
            .unwrap_or_default()
            .split_whitespace()
            .any(|s| s.starts_with("connector:")),
        "{claims}"
    );

    // A connector token (mcp/connector.rs:121): audience `portal-connectors`, exactly one
    // `connector:{id}` scope (bearer.rs:212 `verify_for_scope`), and `portal-api`, which
    // exchanges it for Endpoint reads.
    let connector = public_client_token(app, &format!("openid connector:{CONNECTOR}")).await;
    let subject = connector["access_token"].as_str().unwrap();
    let claims = bearer
        .verify(subject, "portal-connectors", "Bearer")
        .unwrap_or_else(|e| panic!("mcp/connector.rs:121 refuses the connector token: {e}"));
    let scoped: Vec<&str> = claims["scope"]
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .filter(|s| s.starts_with("connector:"))
        .collect();
    assert_eq!(scoped, [format!("connector:{CONNECTOR}")], "{claims}");
    bearer
        .verify(subject, "portal-api", "Bearer")
        .expect("the connector token also names portal-api");
    assert_eq!(claims["jc_org"], json!(["helsinki"]), "{claims}");

    // The Endpoint's scope as the hub wave makes it (hub_scopes.rs:172), linked to portal-api as
    // the deployment's Job does.
    let api = portal_api(app).await;
    let made = admin(
        app,
        &api,
        Method::POST,
        "/client-scopes",
        Some(hub_scope(SLUG)),
    )
    .await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.text);
    let scopes = admin(app, &api, Method::GET, "/client-scopes", None).await.json();
    let scope_id = scopes
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == format!("endpoint:{SLUG}"))
        .map(id_of)
        .expect("the new scope is listed");
    let operator = realm_admin(&realm).await;
    let portal = find_client(app, &operator, "portal-api").await.unwrap();
    let linked = admin(
        app,
        &operator,
        Method::PUT,
        &format!("/clients/{}/optional-client-scopes/{scope_id}", id_of(&portal)),
        None,
    )
    .await;
    assert_eq!(linked.status, StatusCode::NO_CONTENT, "{}", linked.text);

    // auth/oidc.rs:364: audience mcp-hub, scope `gateway-hub endpoint:{slug}`.
    let answer = exchange(app, subject, SLUG).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let exchanged: CoreTokenResponse = serde_json::from_str(&answer.text).expect("token response");
    let token = exchanged.access_token().secret();
    let claims = bearer
        .verify(token, "mcp-hub", "Bearer")
        .unwrap_or_else(|e| panic!("the gateway's hub audience is missing: {e}"));
    assert_eq!(claims["sub"], json!(ana.id));
    assert!(
        claims["scope"]
            .as_str()
            .unwrap_or_default()
            .split_whitespace()
            .any(|s| s == format!("endpoint:{SLUG}")),
        "{claims}"
    );

    // auth/oidc.rs:125: an Endpoint whose scope is not linked yet answers `invalid_scope`.
    let refused = exchange(app, subject, "otherslugotherslugother234").await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
    assert_eq!(refused.json()["error"], json!("invalid_scope"), "{}", refused.text);

    // auth/oidc.rs:114: once the person's sessions end, the subject token is refused as
    // `invalid_token` (or Keycloak's `invalid_request` "Invalid token").
    let reconciler = reconciler(app).await;
    let out = admin(
        app,
        &reconciler,
        Method::POST,
        &format!("/users/{}/logout", ana.id),
        None,
    )
    .await;
    assert_eq!(out.status, StatusCode::NO_CONTENT, "{}", out.text);
    let ended = exchange(app, subject, SLUG).await;
    assert_eq!(ended.status, StatusCode::BAD_REQUEST, "{}", ended.text);
    let body = ended.json();
    assert!(
        body["error"] == "invalid_token"
            || (body["error"] == "invalid_request" && body["error_description"] == "Invalid token"),
        "auth/oidc.rs:114 reads this as an ended session: {body}"
    );

    // auth/oidc.rs:336 the Portal's own client-credentials token, read as a token response.
    let answer = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "client_credentials"),
            ("client_id", "portal-api"),
            ("client_secret", PORTAL_API_SECRET),
        ],
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let own: CoreTokenResponse = serde_json::from_str(&answer.text).expect("token response");
    assert!(own.expires_in().is_some(), "auth/oidc.rs:350 holds it until expiry");
}

/// reconciler/hub_scopes.rs:61 `desired`.
fn hub_scope(slug: &str) -> Value {
    json!({
        "name": format!("endpoint:{slug}"),
        "description": "Reach the Endpoint helsinki/counters through the MCP hub",
        "protocol": "openid-connect",
        "attributes": {
            "include.in.token.scope": "true",
            "display.on.consent.screen": "true",
            "consent.screen.text": "Endpoint helsinki/counters",
            MANAGED.0: MANAGED.1,
            "joinedcontext.endpoint": "helsinki/counters",
        },
    })
}

// ------------------------------------------------------------------------------------------
// People (people.rs) and the group wave (reconciler/groups.rs), as `portal-reconciler`
// ------------------------------------------------------------------------------------------

fn strings_of(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {value}"))
        .iter()
        .map(|v| v.as_str().unwrap_or_else(|| panic!("not a string: {v}")))
        .collect()
}

#[tokio::test]
async fn the_portal_manages_people_as_the_reconciler() {
    let realm = realm().await;
    let app = &realm.app;
    let token = reconciler(app).await;
    let newcomer = "newcomer@hel.example";

    // people.rs:327 create: 201 with the id as Location's last segment.
    let body = json!({
        "username": newcomer, "email": newcomer, "firstName": "Nora", "lastName": "Newcomer",
        "enabled": true, "emailVerified": false,
        "requiredActions": ["VERIFY_EMAIL", "UPDATE_PASSWORD"],
        "attributes": { "locale": ["sk"] },
    });
    let made = admin(app, &token, Method::POST, "/users", Some(body.clone())).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.text);
    let id = made.location().rsplit('/').next().unwrap().to_owned();
    assert!(!id.is_empty());
    // people.rs:341 the fallback when no Location came back.
    let found = admin(app, &token, Method::GET, &format!("/users?email={newcomer}&exact=true"), None).await;
    assert_eq!(found.status, StatusCode::OK, "{}", found.text);
    assert_eq!(found.json()[0]["id"], json!(id), "{}", found.text);
    // The same e-mail again is 409, which people.rs maps to Conflict (PF-04 "exists already").
    let again = admin(app, &token, Method::POST, "/users", Some(body)).await;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.text);

    // people.rs:304 get, read as KcUser.
    let user = admin(app, &token, Method::GET, &format!("/users/{id}"), None).await.json();
    assert_eq!(user["username"], json!(newcomer));
    assert_eq!(user["email"], json!(newcomer));
    assert_eq!(user["firstName"], json!("Nora"));
    assert_eq!(user["lastName"], json!("Newcomer"));
    assert_eq!(user["enabled"], json!(true));
    assert_eq!(user["emailVerified"], json!(false));
    assert!(user["createdTimestamp"].as_i64().is_some_and(|t| t > 0), "{user}");
    let mut required = strings_of(&user["requiredActions"]);
    required.sort_unstable();
    assert_eq!(required, ["UPDATE_PASSWORD", "VERIFY_EMAIL"]);
    assert_eq!(user["attributes"]["locale"], json!(["sk"]), "{user}");

    // people.rs:290 list: the search finds the person and never a service account (Keycloak
    // leaves them out; the People page would list `service-account-portal-api` otherwise).
    let page = admin(
        app,
        &token,
        Method::GET,
        "/users?search=&first=0&max=100&briefRepresentation=false",
        None,
    )
    .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text);
    let names: Vec<String> = page
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["username"].as_str().unwrap().to_owned())
        .collect();
    assert!(names.contains(&newcomer.to_owned()), "{names:?}");
    assert!(names.iter().all(|n| !n.starts_with("service-account-")), "{names:?}");
    // `first` and `max` count people: two pages of one hold Ana and the newcomer.
    let mut paged = Vec::new();
    for first in 0..3 {
        let one = admin(app, &token, Method::GET, &format!("/users?search=&first={first}&max=1&briefRepresentation=false"), None).await.json();
        paged.extend(one.as_array().unwrap().iter().map(|u| u["username"].as_str().unwrap().to_owned()));
    }
    paged.sort();
    assert_eq!(paged, [ANA, newcomer], "people.rs:290 pages");
    let searched = admin(app, &token, Method::GET, "/users?search=nora&first=0&max=20&briefRepresentation=false", None).await;
    assert_eq!(strings_of(&json!(searched.json().as_array().unwrap().iter().map(|u| u["id"].clone()).collect::<Vec<_>>())), [id.as_str()]);

    // people.rs:361 update: read, edit, write the whole representation back; what the Portal
    // does not know survives.
    let mut raw = user.clone();
    raw["firstName"] = json!("Norah");
    raw["attributes"]["department"] = json!(["parks"]);
    let put = admin(app, &token, Method::PUT, &format!("/users/{id}"), Some(raw)).await;
    assert_eq!(put.status, StatusCode::NO_CONTENT, "{}", put.text);
    let user = admin(app, &token, Method::GET, &format!("/users/{id}"), None).await.json();
    assert_eq!(user["firstName"], json!("Norah"));
    assert_eq!(user["attributes"]["locale"], json!(["sk"]), "{user}");
    assert_eq!(user["attributes"]["department"], json!(["parks"]), "{user}");

    // people.rs:383 execute-actions e-mail: sent (2xx) or the realm cannot mail (5xx, the
    // temporary password fallback). A 4xx is an error the Portal shows.
    let mail = admin(
        app,
        &token,
        Method::PUT,
        &format!(
            "/users/{id}/execute-actions-email?lifespan=259200&client_id=portal-api&redirect_uri={}",
            url::form_urlencoded::byte_serialize(format!("{PORTAL}/").as_bytes()).collect::<String>()
        ),
        Some(json!(["VERIFY_EMAIL", "UPDATE_PASSWORD"])),
    )
    .await;
    assert!(
        mail.status.is_success() || mail.status.is_server_error(),
        "people.rs:398 reads {} as an error: {}",
        mail.status,
        mail.text
    );

    // people.rs:408 temporary password.
    let reset = admin(
        app,
        &token,
        Method::PUT,
        &format!("/users/{id}/reset-password"),
        Some(json!({ "type": "password", "value": "Temporary-Pass-123!", "temporary": true })),
    )
    .await;
    assert_eq!(reset.status, StatusCode::NO_CONTENT, "{}", reset.text);

    // people.rs:426 credentials, read as {id, type}.
    let credentials = admin(app, &token, Method::GET, &format!("/users/{id}/credentials"), None).await;
    assert_eq!(credentials.status, StatusCode::OK, "{}", credentials.text);
    let credentials = credentials.json();
    assert!(
        credentials.as_array().unwrap().iter().any(|c| c["type"] == "password" && c["id"].is_string()),
        "{credentials}"
    );

    // people.rs:445 sessions and :452 consents: arrays.
    for path in ["sessions", "consents"] {
        let listed = admin(app, &token, Method::GET, &format!("/users/{id}/{path}"), None).await;
        assert_eq!(listed.status, StatusCode::OK, "{path}: {}", listed.text);
        assert!(listed.json().is_array(), "{path}: {}", listed.text);
    }
    let ana = realm.store.get_user_by_username_or_email("jc", None, ANA).await.unwrap().unwrap();
    let sessions = admin(app, &token, Method::GET, &format!("/users/{}/sessions", ana.id), None).await.json();
    assert!(sessions.as_array().unwrap().is_empty(), "nobody signed in yet: {sessions}");

    // people.rs:507 join_members: an exact search for `members`, the tree under each root, the
    // group at `/{organization}/members`, then the membership.
    let tree = admin(
        app,
        &token,
        Method::GET,
        "/groups?search=members&exact=true&briefRepresentation=true&max=1000",
        None,
    )
    .await;
    assert_eq!(tree.status, StatusCode::OK, "{}", tree.text);
    let tree = tree.json();
    fn find(nodes: &Value, path: &str) -> Option<String> {
        nodes.as_array()?.iter().find_map(|node| {
            if node["path"] == path {
                node["id"].as_str().map(str::to_owned)
            } else {
                find(&node["subGroups"], path)
            }
        })
    }
    let members = find(&tree, "/helsinki/members")
        .unwrap_or_else(|| panic!("people.rs:147 finds no /helsinki/members in {tree}"));
    let joined = admin(app, &token, Method::PUT, &format!("/users/{id}/groups/{members}"), None).await;
    assert_eq!(joined.status, StatusCode::NO_CONTENT, "{}", joined.text);

    // people.rs:483 the groups by path and the realm roles, composites expanded.
    let groups = admin(app, &token, Method::GET, &format!("/users/{id}/groups"), None).await.json();
    let paths: Vec<&str> = groups.as_array().unwrap().iter().map(|g| g["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["/helsinki/members"], "{groups}");
    let roles = admin(app, &token, Method::GET, &format!("/users/{id}/role-mappings/realm/composite"), None).await;
    assert_eq!(roles.status, StatusCode::OK, "{}", roles.text);
    let roles = roles.json();
    let names: Vec<&str> = roles.as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    // The realm's default role: claude.ai's connector asks offline_access (T-6301).
    assert!(names.contains(&"offline_access"), "{roles}");

    // people.rs:418 sign out, :434 a second factor's removal (none here), :519 delete.
    let out = admin(app, &token, Method::POST, &format!("/users/{id}/logout"), None).await;
    assert_eq!(out.status, StatusCode::NO_CONTENT, "{}", out.text);
    let gone = admin(app, &token, Method::DELETE, &format!("/users/{id}"), None).await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT, "{}", gone.text);
    // people.rs:270 a 404 is NotFound.
    let missing = admin(app, &token, Method::GET, &format!("/users/{id}"), None).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "{}", missing.text);
    assert!(
        missing.json()["error"].is_string() || missing.json()["errorMessage"].is_string(),
        "people.rs:273 reads errorMessage or error: {}",
        missing.text
    );
}

#[tokio::test]
async fn the_group_wave_converges_as_the_reconciler() {
    let realm = realm().await;
    let app = &realm.app;
    let token = reconciler(app).await;
    let ana = realm.store.get_user_by_username_or_email("jc", None, ANA).await.unwrap().unwrap();

    // groups.rs:331 a managed root group with its Organization claims.
    let attributes = json!({ MANAGED.0: [MANAGED.1], "jc_org": ["helsinki"], "jc_groups": ["/helsinki/stewards"] });
    let made = admin(app, &token, Method::POST, "/groups", Some(json!({ "name": "helsinki-stewards", "attributes": attributes }))).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.text);
    // groups.rs:344 found again by an exact name search, read as KcGroup.
    let found = admin(app, &token, Method::GET, "/groups?search=helsinki-stewards&exact=true", None).await.json();
    let group = found
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["name"] == "helsinki-stewards")
        .unwrap_or_else(|| panic!("groups.rs:350: {found}"))
        .clone();
    let id = id_of(&group);

    // groups.rs:207 the whole list with attributes, the managed mark as groups.rs:140 reads it.
    let all = admin(app, &token, Method::GET, "/groups?briefRepresentation=false&max=1000", None).await.json();
    let listed = all.as_array().unwrap().iter().find(|g| g["id"] == json!(id)).unwrap_or_else(|| panic!("{all}"));
    assert_eq!(listed["attributes"], attributes, "{listed}");

    // groups.rs:385 members, :403 join, :486 exists.
    let path = format!("/groups/{id}/members?max=1000");
    assert_eq!(admin(app, &token, Method::GET, &path, None).await.json(), json!([]));
    let exists = admin(app, &token, Method::GET, &format!("/users/{}", ana.id), None).await;
    assert_eq!(exists.status, StatusCode::OK, "{}", exists.text);
    let nobody = admin(app, &token, Method::GET, "/users/00000000-0000-0000-0000-000000000000", None).await;
    assert_eq!(nobody.status, StatusCode::NOT_FOUND, "app_clients.rs:583 reads 404 as absent: {}", nobody.text);
    let joined = admin(app, &token, Method::PUT, &format!("/users/{}/groups/{id}", ana.id), None).await;
    assert_eq!(joined.status, StatusCode::NO_CONTENT, "{}", joined.text);
    let held = admin(app, &token, Method::GET, &path, None).await.json();
    let ids: Vec<&str> = held.as_array().unwrap().iter().map(|u| u["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [ana.id.as_str()], "{held}");

    // groups.rs:471 the claims rewritten.
    let changed = json!({ MANAGED.0: [MANAGED.1], "jc_org": ["helsinki"], "jc_groups": ["/helsinki/editors"] });
    let put = admin(app, &token, Method::PUT, &format!("/groups/{id}"), Some(json!({ "name": "helsinki-stewards", "attributes": changed }))).await;
    assert_eq!(put.status, StatusCode::NO_CONTENT, "{}", put.text);
    let all = admin(app, &token, Method::GET, "/groups?briefRepresentation=false&max=1000", None).await.json();
    let listed = all.as_array().unwrap().iter().find(|g| g["id"] == json!(id)).unwrap();
    assert_eq!(listed["attributes"], changed, "{listed}");

    // groups.rs:428 leave, :288 delete.
    let left = admin(app, &token, Method::DELETE, &format!("/users/{}/groups/{id}", ana.id), None).await;
    assert_eq!(left.status, StatusCode::NO_CONTENT, "{}", left.text);
    let gone = admin(app, &token, Method::DELETE, &format!("/groups/{id}"), None).await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT, "{}", gone.text);

    // Least privilege: the reconciler writes no client and reads no client secret.
    let clients = admin(app, &token, Method::GET, "/clients?clientId=portal-api", None).await;
    let listed = if clients.status == StatusCode::OK { clients.json() } else { json!([]) };
    assert!(listed.as_array().unwrap().is_empty(), "portal-reconciler reads clients: {listed}");
    let made = admin(app, &token, Method::POST, "/clients", Some(json!({ "clientId": "app-sneaky", "attributes": { MANAGED.0: MANAGED.1 } }))).await;
    assert!(made.status.is_client_error(), "portal-reconciler creates a client: {}", made.status);
}

// ------------------------------------------------------------------------------------------
// App clients (reconciler/app_clients.rs): `portal-api` writes the client, its mappers, roles and
// secret; `portal-reconciler` maps the roles to groups and users.
// ------------------------------------------------------------------------------------------

/// app_clients.rs:163 `desired` for App `bikes` of project helsinki under `apps.jc.localhost`.
fn app_client() -> Value {
    let origin = "https://bikes.apps.jc.localhost";
    let redirect = format!("{origin}/*");
    json!({
        "clientId": "app-bikes",
        "name": "Bikes",
        "protocol": "openid-connect",
        "enabled": true,
        "publicClient": false,
        "bearerOnly": false,
        "standardFlowEnabled": true,
        "implicitFlowEnabled": false,
        "directAccessGrantsEnabled": false,
        "serviceAccountsEnabled": false,
        "frontchannelLogout": true,
        "redirectUris": [redirect],
        "webOrigins": [origin],
        "attributes": {
            "pkce.code.challenge.method": "S256",
            "post.logout.redirect.uris": redirect,
            "access.token.signed.response.alg": "RS256",
            "id.token.signed.response.alg": "RS256",
            "access.token.lifespan": "60",
            MANAGED.0: MANAGED.1,
            "joinedcontext.app": "helsinki/bikes",
        },
    })
}

/// app_clients.rs:251 `audience_mapper`, :270 `groups_mapper`, :291 `organization_mappers`.
fn app_mappers(audiences: &[&str]) -> Vec<Value> {
    let mut mappers: Vec<Value> = audiences
        .iter()
        .map(|audience| {
            json!({
                "name": format!("audience-{audience}"),
                "protocol": "openid-connect",
                "protocolMapper": "oidc-audience-mapper",
                "config": {
                    "included.custom.audience": audience,
                    "access.token.claim": "true",
                    "id.token.claim": "false",
                    "introspection.token.claim": "true",
                },
            })
        })
        .collect();
    mappers.push(json!({
        "name": "groups",
        "protocol": "openid-connect",
        "protocolMapper": "oidc-group-membership-mapper",
        "consentRequired": false,
        "config": {
            "claim.name": "groups", "full.path": "true", "access.token.claim": "true",
            "id.token.claim": "true", "userinfo.token.claim": "true",
        },
    }));
    for claim in ["jc_org", "jc_groups"] {
        mappers.push(json!({
            "name": claim,
            "protocol": "openid-connect",
            "protocolMapper": "oidc-usermodel-attribute-mapper",
            "consentRequired": false,
            "config": {
                "user.attribute": claim, "claim.name": claim, "jsonType.label": "String",
                "multivalued": "true", "aggregate.attrs": "true", "access.token.claim": "true",
                "id.token.claim": "true", "userinfo.token.claim": "true",
            },
        }));
    }
    mappers
}

/// app_clients.rs:655 `converge_mappers`: every held mapper the wave does not want goes, a
/// missing one is posted, a changed one put back. Answers what the run changed.
async fn converge_mappers(app: &Router, token: &str, uuid: &str, wanted: &[Value]) -> Vec<String> {
    let path = format!("/clients/{uuid}/protocol-mappers/models");
    let held = admin(app, token, Method::GET, &path, None).await;
    assert_eq!(held.status, StatusCode::OK, "{}", held.text);
    let held = held.json();
    let mut changed = Vec::new();
    for mapper in held.as_array().unwrap() {
        let ours = wanted
            .iter()
            .any(|w| w["protocolMapper"] == mapper["protocolMapper"] && w["name"] == mapper["name"]);
        if !ours {
            let gone = admin(app, token, Method::DELETE, &format!("{path}/{}", id_of(mapper)), None).await;
            assert_eq!(gone.status, StatusCode::NO_CONTENT, "{}", gone.text);
            changed.push(format!("removed {}", mapper["name"]));
        }
    }
    for want in wanted {
        match held.as_array().unwrap().iter().find(|m| m["name"] == want["name"]) {
            None => {
                let made = admin(app, token, Method::POST, &path, Some(want.clone())).await;
                assert_eq!(made.status, StatusCode::CREATED, "{}", made.text);
                changed.push(format!("added {}", want["name"]));
            }
            Some(mapper) => {
                let differs = want["config"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .any(|(k, v)| mapper["config"].get(k) != Some(v));
                if differs {
                    let mut body = want.clone();
                    body["id"] = mapper["id"].clone();
                    let put = admin(app, token, Method::PUT, &format!("{path}/{}", id_of(mapper)), Some(body)).await;
                    assert_eq!(put.status, StatusCode::NO_CONTENT, "{}", put.text);
                    changed.push(format!("rewrote {}", want["name"]));
                }
            }
        }
    }
    changed
}

/// app_clients.rs:620 `drop_optional_scopes`: answers the names it unlinked.
async fn drop_optional_scopes(app: &Router, token: &str, uuid: &str) -> Vec<String> {
    let path = format!("/clients/{uuid}/optional-client-scopes");
    let linked = admin(app, token, Method::GET, &path, None).await;
    assert_eq!(linked.status, StatusCode::OK, "{}", linked.text);
    let linked = linked.json();
    let mut names = Vec::new();
    for scope in linked.as_array().unwrap() {
        let gone = admin(app, token, Method::DELETE, &format!("{path}/{}", id_of(scope)), None).await;
        assert_eq!(gone.status, StatusCode::NO_CONTENT, "{}", gone.text);
        names.push(scope["name"].as_str().unwrap().to_owned());
    }
    names
}

/// Ana's sign-in to App `bikes` through its confidential client, as the App's edge does it;
/// answers the access token's claims as the edge verifies them (RS256, aud `app-bikes`).
async fn app_sign_in(app: &Router, api: &str, uuid: &str) -> Value {
    let secret = admin(app, api, Method::GET, &format!("/clients/{uuid}/client-secret"), None).await.json();
    let secret = secret["value"].as_str().unwrap().to_owned();
    let (pkce, verifier) = PkceCodeChallenge::new_random_sha256();
    let back = "https://bikes.apps.jc.localhost/oauth2/callback";
    let authorize = format!(
        "/realms/jc/protocol/openid-connect/auth?client_id=app-bikes&redirect_uri={}\
         &response_type=code&scope=openid&state=s2&code_challenge={}&code_challenge_method=S256",
        url::form_urlencoded::byte_serialize(back.as_bytes()).collect::<String>(),
        pkce.as_str(),
    );
    let (landed, _) = sign_in(app, &authorize).await;
    let code = param(&landed, "code").unwrap_or_else(|| panic!("no code: {landed}"));
    let answer = post_form(
        app,
        TOKEN,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", back),
            ("code_verifier", verifier.secret()),
            ("client_id", "app-bikes"),
            ("client_secret", &secret),
        ],
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let access = answer.json()["access_token"].as_str().unwrap().to_owned();
    assert_eq!(jsonwebtoken9::decode_header(&access).unwrap().alg, jsonwebtoken9::Algorithm::RS256);
    Bearer::of(app)
        .await
        .verify(&access, "app-bikes", "Bearer")
        .unwrap_or_else(|e| panic!("the App's edge refuses the token: {e}"))
}

#[tokio::test]
async fn the_app_wave_converges_an_app_client() {
    let realm = realm().await;
    let app = &realm.app;
    let api = portal_api(app).await;
    let members = reconciler(app).await;
    let want = app_client();

    // app_clients.rs:1196 create, :601 find it back.
    assert!(find_client(app, &api, "app-bikes").await.is_none());
    let made = admin(app, &api, Method::POST, "/clients", Some(want.clone())).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.text);
    let held = find_client(app, &api, "app-bikes").await.expect("app-bikes is found");
    let uuid = id_of(&held);
    // app_clients.rs:1177 the next run finds nothing to write back.
    assert_eq!(drift(&want, &held), Vec::<String>::new(), "{held}");

    // T-6301: Keycloak links the realm's default optional scopes, which the wave unlinks;
    // kaisatsu links none to an admin-made client, which the wave reads as nothing to drop.
    let dropped = drop_optional_scopes(app, &api, &uuid).await;
    assert!(dropped.iter().all(|n| n == "connector" || n == "offline_access"), "{dropped:?}");
    assert_eq!(drop_optional_scopes(app, &api, &uuid).await, Vec::<String>::new());

    // :655 the mappers: added once, then nothing to change.
    let wanted = app_mappers(&["app-bikes", SLUG]);
    let first = converge_mappers(app, &api, &uuid, &wanted).await;
    assert!(first.iter().filter(|c| c.starts_with("added")).count() == wanted.len(), "{first:?}");
    assert_eq!(converge_mappers(app, &api, &uuid, &wanted).await, Vec::<String>::new());
    // One Endpoint fewer: its audience mapper goes.
    let fewer = app_mappers(&["app-bikes"]);
    assert_eq!(converge_mappers(app, &api, &uuid, &fewer).await, [format!("removed \"audience-{SLUG}\"")]);

    // :870 roles: listed, created by name, read back with an id.
    let roles = format!("/clients/{uuid}/roles");
    assert_eq!(admin(app, &api, Method::GET, &roles, None).await.json(), json!([]));
    for name in ["rider", "dispatcher"] {
        let made = admin(app, &api, Method::POST, &roles, Some(json!({ "name": name }))).await;
        assert_eq!(made.status, StatusCode::CREATED, "{}", made.text);
    }
    let held_roles = admin(app, &api, Method::GET, &roles, None).await.json();
    let role = |name: &str| {
        held_roles
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{held_roles}"))
            .clone()
    };
    let rider = role("rider");
    let mapping = json!([{ "id": rider["id"], "name": "rider" }]);

    // :799 the group by name and :841 the user by e-mail, as the member credential.
    let groups = admin(app, &members, Method::GET, "/groups?briefRepresentation=true&max=1000", None).await;
    assert_eq!(groups.status, StatusCode::OK, "{}", groups.text);
    let helsinki = groups
        .json()
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["name"] == "helsinki")
        .map(id_of)
        .expect("the root group helsinki");
    let users = admin(app, &members, Method::GET, &format!("/users?exact=true&email={}", url::form_urlencoded::byte_serialize(ANA.as_bytes()).collect::<String>()), None).await;
    assert_eq!(users.status, StatusCode::OK, "{}", users.text);
    let ana = users.json()[0].clone();
    assert_eq!(ana["email"], json!(ANA), "{ana}");
    let ana_id = id_of(&ana);

    // :940 the mappings, written by the member credential.
    for path in [format!("/groups/{helsinki}"), format!("/users/{ana_id}")] {
        let mapped = admin(app, &members, Method::POST, &format!("{path}/role-mappings/clients/{uuid}"), Some(mapping.clone())).await;
        assert_eq!(mapped.status, StatusCode::NO_CONTENT, "{path}: {}", mapped.text);
    }
    // :908 who holds the role, read by the App-client manager (view-clients + query-*).
    let by_groups = admin(app, &api, Method::GET, &format!("{roles}/rider/groups?briefRepresentation=true&max=1000"), None).await;
    assert_eq!(by_groups.status, StatusCode::OK, "{}", by_groups.text);
    let by_groups = by_groups.json();
    let names: Vec<&str> = by_groups.as_array().unwrap().iter().map(|g| g["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["helsinki"]);
    let by_users = admin(app, &api, Method::GET, &format!("{roles}/rider/users?max=1000"), None).await;
    assert_eq!(by_users.status, StatusCode::OK, "{}", by_users.text);
    let held_by = by_users.json();
    assert_eq!(held_by[0]["id"], json!(ana_id), "{held_by}");
    assert_eq!(held_by[0]["username"], json!(ANA), "{held_by}");

    // A sign-in to the App: the edge verifies RS256 and reads aud, resource_access and groups.
    let claims = app_sign_in(app, &api, &uuid).await;
    assert_eq!(claims["resource_access"]["app-bikes"]["roles"], json!(["rider"]), "{claims}");
    assert_eq!(claims["groups"], json!(["/helsinki/members"]), "{claims}");
    assert_eq!(claims["jc_org"], json!(["helsinki"]), "{claims}");
    assert!(claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap() <= 60, "access.token.lifespan 60: {claims}");
    // The role held through the root group `helsinki` alone (a group subject of `spec.access`).
    let gone = admin(app, &members, Method::DELETE, &format!("/users/{ana_id}/role-mappings/clients/{uuid}"), Some(mapping.clone())).await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT, "{}", gone.text);
    let claims = app_sign_in(app, &api, &uuid).await;
    assert_eq!(claims["resource_access"]["app-bikes"]["roles"], json!(["rider"]), "a group's App role: {claims}");
    let mapped = admin(app, &members, Method::POST, &format!("/users/{ana_id}/role-mappings/clients/{uuid}"), Some(mapping.clone())).await;
    assert_eq!(mapped.status, StatusCode::NO_CONTENT, "{}", mapped.text);

    // :929 a mapping the App no longer gives is removed by the member credential.
    for path in [format!("/groups/{helsinki}"), format!("/users/{ana_id}")] {
        let gone = admin(app, &members, Method::DELETE, &format!("{path}/role-mappings/clients/{uuid}"), Some(mapping.clone())).await;
        assert_eq!(gone.status, StatusCode::NO_CONTENT, "{path}: {}", gone.text);
    }
    assert_eq!(admin(app, &api, Method::GET, &format!("{roles}/rider/users?max=1000"), None).await.json(), json!([]));
    // A role taken away is gone from the next token (app_clients.rs:181, T-4914).
    let claims = app_sign_in(app, &api, &uuid).await;
    assert!(claims["resource_access"]["app-bikes"].is_null(), "{claims}");
    // :878 a role the App drops.
    let gone = admin(app, &api, Method::DELETE, &format!("{roles}/dispatcher"), None).await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT, "{}", gone.text);

    // :1003 the secret the App's edge reads.
    let secret = admin(app, &api, Method::GET, &format!("/clients/{uuid}/client-secret"), None).await;
    assert_eq!(secret.status, StatusCode::OK, "{}", secret.text);
    assert!(secret.json()["value"].as_str().is_some_and(|v| v.len() >= 32), "a secret value");

    // :1181 a field changed by hand is put back, then nothing drifts.
    let mut changed = held.clone();
    changed["redirectUris"] = json!(["https://evil.example/*"]);
    let put = admin(app, &api, Method::PUT, &format!("/clients/{uuid}"), Some(changed)).await;
    assert_eq!(put.status, StatusCode::NO_CONTENT, "{}", put.text);
    let held = find_client(app, &api, "app-bikes").await.unwrap();
    assert_eq!(drift(&want, &held), ["redirectUris: Some(Array [String(\"https://evil.example/*\")]) != [\"https://bikes.apps.jc.localhost/*\"]"]);
    let put = admin(app, &api, Method::PUT, &format!("/clients/{uuid}"), Some(want.clone())).await;
    assert_eq!(put.status, StatusCode::NO_CONTENT, "{}", put.text);
    assert_eq!(drift(&want, &find_client(app, &api, "app-bikes").await.unwrap()), Vec::<String>::new());

    // :1067 the managed app clients, by a clientId search; :1107 one no App names is removed.
    let listed = admin(app, &api, Method::GET, "/clients?search=true&clientId=app-&max=1000", None).await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text);
    let listed = listed.json();
    let ids: Vec<&str> = listed.as_array().unwrap().iter().map(|c| c["clientId"].as_str().unwrap()).collect();
    assert_eq!(ids, ["app-bikes"]);
    let gone = admin(app, &api, Method::DELETE, &format!("/clients/{uuid}"), None).await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT, "{}", gone.text);
    assert!(find_client(app, &api, "app-bikes").await.is_none());
}
