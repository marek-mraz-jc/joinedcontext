//! T-7133: an Organization with a sign-in host of its own (`JC_PORTAL_ORGANIZATION_REALMS`) signs
//! in through its own realm on its own Portal host, and nowhere else. Through the whole router,
//! with the `Host` a browser sends:
//!
//! - a bearer of ba's realm reaches ba's projects on ba's host, and is a foreign issuer's token on
//!   hel's host and on the deployment's own address; a key of the installation under ba's issuer,
//!   and a realm the Portal does not know, are refused there too;
//! - a token of ba's realm never carries the operators' bootstrap group;
//! - a session ba's realm issued opens on ba's host alone.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use p256::pkcs8::EncodePrivateKey;
use serde_json::json;
use tower::ServiceExt;

use common::envelope;
use joinedcontext_portal::auth::client_auth::ClientAuth;
use joinedcontext_portal::auth::realms::RealmSetting;
use joinedcontext_portal::auth::session::{self, Session};
use joinedcontext_portal::config::{Config, OidcConfig};
use joinedcontext_portal::organization_host::{self, OrganizationHost};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const HEL: &str = "portal.hel.example";
const BA: &str = "portal.ba.example";
const ROOTS: &str = "platform-admins";
const INSTALLATION: &str = "https://idm.example.test/realms/jc";
const BA_REALM: &str = "https://idm.ba.example/realms/ba";

fn host(name: &str) -> OrganizationHost {
    let at = if name == "hel" { HEL } else { BA };
    OrganizationHost {
        organization: name.to_owned(),
        base: format!("https://{at}").parse().expect("url"),
    }
}

/// A fresh P-256 key pair: the signing key and its public half as a JWK set.
fn keypair(kid: &str) -> (EncodingKey, JwkSet) {
    let secret = p256::SecretKey::random(&mut rand_core::OsRng);
    let der = secret.to_pkcs8_der().expect("der");
    let signer = EncodingKey::from_ec_der(der.as_bytes());
    let mut jwk: serde_json::Value =
        serde_json::from_str(&secret.public_key().to_jwk_string()).expect("jwk");
    jwk["kid"] = json!(kid);
    jwk["alg"] = json!("ES256");
    jwk["use"] = json!("sig");
    let set: JwkSet = serde_json::from_value(json!({ "keys": [jwk] })).expect("set");
    (signer, set)
}

struct World {
    state: AppState,
    installation: EncodingKey,
    ba: EncodingKey,
}

/// hel (the installation's Organization, no realm of its own) and ba (its own realm), a project
/// each; `ba-only` is bound in ba, `hel-only` in hel. Port 9 (discard) refuses a key fetch at
/// once, so only the keys installed here verify.
fn world() -> World {
    world_at("http://127.0.0.1:9")
}

/// [`world`] with ba's realm reached at `ba_back` (its keys, tokens and admin API).
fn world_at(ba_back: &str) -> World {
    let oidc = OidcConfig {
        issuer: INSTALLATION.parse().expect("url"),
        client_id: "portal-api".into(),
        auth: ClientAuth::Secret("s".into()),
        extra_ca_pem: None,
        admin_url: Some("http://127.0.0.1:9".parse().expect("url")),
    };
    let config = Config {
        organization_hosts: vec![host("hel"), host("ba")],
        organization_realms: vec![RealmSetting {
            organization: "ba".into(),
            issuer: BA_REALM.parse().expect("url"),
            jwks_url: format!("{ba_back}/realms/ba/protocol/openid-connect/certs")
                .parse()
                .expect("url"),
        }],
        oidc: Some(oidc),
        keycloak_admin: Some(("portal-reconciler".into(), ClientAuth::Secret("r".into()))),
        bootstrap_admins: ROOTS.to_owned(),
        ..Config::for_tests()
    };
    let state = AppState::new(config, None);
    let (installation, set) = keypair("installation");
    state
        .bearer
        .as_ref()
        .expect("installation verifier")
        .install(&set);
    let (ba, set) = keypair("ba");
    state.organization_realms["ba"].bearer.install(&set);
    let m = &state.mirror;
    m.upsert(envelope(
        "Role",
        "reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Project", "ContextSpace", "Person"], "verbs": ["read"] }] }),
    ));
    m.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        json!({ "installation": true }),
    ));
    m.upsert(envelope("Organization", "ba", ORG_NAMESPACE, json!({})));
    for (project, owner) in [("helsinki", "hel"), ("bratislava", "ba")] {
        m.upsert(envelope(
            "Project",
            project,
            ORG_NAMESPACE,
            json!({ "organizationRef": owner }),
        ));
    }
    for (who, org) in [("hel-only", "hel"), ("ba-only", "ba")] {
        m.upsert(envelope(
            "RoleBinding",
            &format!("{who}-{org}"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{who}@example.test") }], "role": "reader",
                    "scope": { "organization": org } }),
        ));
    }
    World {
        state,
        installation,
        ba,
    }
}

fn token(key: &EncodingKey, kid: &str, issuer: &str, who: &str, groups: &[&str]) -> String {
    let now = session::now_unix();
    let claims = json!({
        "iss": issuer, "typ": "Bearer", "aud": "portal-api", "sub": format!("sub-{who}"),
        "preferred_username": who, "email": format!("{who}@example.test"), "email_verified": true,
        "exp": now + 300, "iat": now, "groups": groups,
    });
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.into());
    encode(&header, &claims, key).expect("sign")
}

async fn get(state: &AppState, at: &str, auth: (&str, &str)) -> StatusCode {
    server::app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects")
                .header(header::HOST, at)
                .header(auth.0, auth.1)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
        .status()
}

async fn bearer(state: &AppState, at: &str, token: &str) -> StatusCode {
    get(state, at, ("authorization", &format!("Bearer {token}"))).await
}

#[tokio::test]
async fn a_token_of_an_organizations_realm_reaches_its_own_host_alone() {
    let w = world();
    let own = token(&w.ba, "ba", BA_REALM, "ba-only", &[]);
    assert_eq!(
        bearer(&w.state, BA, &own).await,
        StatusCode::OK,
        "ba's realm on ba's host"
    );
    assert_eq!(
        bearer(&w.state, HEL, &own).await,
        StatusCode::UNAUTHORIZED,
        "ba's realm on hel's host"
    );
    assert_eq!(
        bearer(&w.state, "portal.example", &own).await,
        StatusCode::UNAUTHORIZED,
        "ba's realm on the deployment's own address"
    );
    // The installation's key under ba's issuer, and ba's key under the installation's issuer.
    let forged = token(&w.installation, "installation", BA_REALM, "ba-only", &[]);
    assert_eq!(
        bearer(&w.state, BA, &forged).await,
        StatusCode::UNAUTHORIZED
    );
    let swapped = token(&w.ba, "ba", INSTALLATION, "ba-only", &[]);
    assert_eq!(
        bearer(&w.state, BA, &swapped).await,
        StatusCode::UNAUTHORIZED
    );
    // A realm the Portal does not sign in through is a foreign issuer's everywhere.
    let unknown = token(
        &w.ba,
        "ba",
        "https://idm.za.example/realms/za",
        "ba-only",
        &[],
    );
    assert_eq!(
        bearer(&w.state, BA, &unknown).await,
        StatusCode::UNAUTHORIZED
    );
    // The installation realm still signs in on ba's host (operators, people not moved yet).
    let operator = token(
        &w.installation,
        "installation",
        INSTALLATION,
        "ba-only",
        &[],
    );
    assert_eq!(bearer(&w.state, BA, &operator).await, StatusCode::OK);
    let hel = token(
        &w.installation,
        "installation",
        INSTALLATION,
        "hel-only",
        &[],
    );
    assert_eq!(bearer(&w.state, HEL, &hel).await, StatusCode::OK);
}

/// The bootstrap group is the operators', in the installation realm: a token of ba's realm that
/// names it reaches nothing a stranger would not.
#[tokio::test]
async fn an_organizations_realm_never_grants_the_operators_group() {
    let w = world();
    let root = token(&w.ba, "ba", BA_REALM, "root", &[ROOTS]);
    assert_eq!(bearer(&w.state, BA, &root).await, StatusCode::FORBIDDEN);
    let operator = token(
        &w.installation,
        "installation",
        INSTALLATION,
        "root",
        &[ROOTS],
    );
    assert_eq!(
        bearer(&w.state, BA, &operator).await,
        StatusCode::OK,
        "the control"
    );
}

/// The cookie a browser holds after signing in on `on` through `realm`.
async fn cookie(config: &Config, on: &str, realm: Option<&str>) -> String {
    let mint = async {
        let now = session::now_unix();
        let session = Session {
            identity: common::person("ba-only"),
            expires_at: now + 3600,
            issued_at: now,
            signed_in_at: None,
            id_token: "id".into(),
            access_expires_at: now + 3600,
            refresh_token: None,
        };
        let mut identity = session.identity.clone();
        identity.email = Some("ba-only@example.test".into());
        let session = Session {
            identity,
            ..session
        };
        let jar = session::store_in(
            PrivateCookieJar::new(config.cookie_key.clone()),
            &session,
            realm,
        )
        .expect("store");
        (jar, StatusCode::OK)
            .into_response()
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
            .collect::<Vec<_>>()
            .join("; ")
    };
    organization_host::scope(host(on), mint).await
}

#[tokio::test]
async fn a_session_of_an_organizations_realm_opens_on_its_own_host_alone() {
    let w = world();
    let config = &w.state.config;
    let own = cookie(config, "ba", Some("ba")).await;
    assert_eq!(get(&w.state, BA, ("cookie", &own)).await, StatusCode::OK);
    assert_eq!(
        get(&w.state, HEL, ("cookie", &own)).await,
        StatusCode::UNAUTHORIZED,
        "ba's realm session on hel's host"
    );
    // A session sealed on hel's host that names ba's realm is no session anywhere.
    let crossed = cookie(config, "hel", Some("ba")).await;
    assert_eq!(
        get(&w.state, HEL, ("cookie", &crossed)).await,
        StatusCode::UNAUTHORIZED
    );
    // The installation realm's session on ba's host still opens (operators, people not moved).
    let installation = cookie(config, "ba", None).await;
    assert_eq!(
        get(&w.state, BA, ("cookie", &installation)).await,
        StatusCode::OK
    );
}

async fn mcp(state: &AppState, at: &str, token: &str) -> StatusCode {
    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/mcp")
                .header(header::HOST, at)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(ping.to_string()))
                .expect("request"),
        )
        .await
        .expect("response")
        .status()
}

/// T-7142: the Portal's MCP door takes a token of ba's realm on ba's host, as `/api/v1` does,
/// and nowhere else.
#[tokio::test]
async fn the_mcp_door_takes_an_organizations_realm_on_its_own_host_alone() {
    let w = world();
    let own = token(&w.ba, "ba", BA_REALM, "ba-only", &[]);
    assert_eq!(mcp(&w.state, BA, &own).await, StatusCode::OK, "ba's host");
    assert_eq!(
        mcp(&w.state, HEL, &own).await,
        StatusCode::UNAUTHORIZED,
        "ba's realm on hel's host"
    );
    assert_eq!(
        mcp(&w.state, "portal.example", &own).await,
        StatusCode::UNAUTHORIZED
    );
    let operator = token(
        &w.installation,
        "installation",
        INSTALLATION,
        "hel-only",
        &[],
    );
    assert_eq!(
        mcp(&w.state, HEL, &operator).await,
        StatusCode::OK,
        "the control"
    );
}

async fn people(state: &AppState, at: &str, token: &str) -> StatusCode {
    server::app(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/v1/organization/people")
                .header(header::HOST, at)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
        .status()
}

/// T-7142: on ba's host the people doors manage ba's realm, as ba's own `portal-reconciler`, and
/// only there; a reader of hel is refused ba's people, and ba's realm is never asked from
/// another host.
#[tokio::test]
async fn the_people_of_an_organizations_realm_are_managed_in_that_realm_alone() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let ba = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/ba/protocol/openid-connect/token"))
        .and(body_string_contains("client_credentials"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access_token": "ba-admin", "token_type": "Bearer" })),
        )
        .expect(1)
        .mount(&ba)
        .await;
    Mock::given(method("GET"))
        .and(path("/admin/realms/ba/users"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&ba)
        .await;
    let w = world_at(&ba.uri());
    let own = token(&w.ba, "ba", BA_REALM, "ba-only", &[]);
    assert_eq!(
        people(&w.state, BA, &own).await,
        StatusCode::OK,
        "ba's people on ba's host"
    );
    assert_eq!(people(&w.state, HEL, &own).await, StatusCode::UNAUTHORIZED);
    let hel = token(
        &w.installation,
        "installation",
        INSTALLATION,
        "hel-only",
        &[],
    );
    assert_eq!(
        people(&w.state, BA, &hel).await,
        StatusCode::FORBIDDEN,
        "a reader of hel on ba's people"
    );
    // On hel's host the installation realm's people, which this Portal has no admin client of:
    // never ba's realm.
    assert_eq!(
        people(&w.state, HEL, &hel).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
}
