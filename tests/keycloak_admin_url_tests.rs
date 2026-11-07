//! The Portal's own calls to Keycloak go to `JC_KEYCLOAK_ADMIN_URL` (T-4432, CC-03, PF-47).
//!
//! On dev every reconciler token and admin call went out through the public edge (idm.dev, the
//! APISIX limit-count shared with visitors). With the variable set, the token and admin calls go
//! to the in-cluster address while discovery, and so every token check, stays on
//! `JC_OIDC_ISSUER`; unset, everything goes to the issuer as before. Two stub realms: one is the
//! public issuer, the other the Service.

use joinedcontext_portal::config::{Config, ConfigError};
use joinedcontext_portal::state::AppState;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REALM: &str = "/realms/hel";

/// A realm that answers discovery, its keys, a token and one user.
async fn realm() -> MockServer {
    let server = MockServer::start().await;
    let issuer = format!("{}{REALM}", server.uri());
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/.well-known/openid-configuration")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/protocol/openid-connect/certs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "keys": [] })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/protocol/openid-connect/token")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "access_token": "t" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/admin/realms/hel/users/ada-id"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "id": "ada-id", "username": "ada" })),
        )
        .mount(&server)
        .await;
    server
}

fn config(issuer: &str, admin_url: Option<&str>) -> Result<Config, ConfigError> {
    Config::from_vars(|k| match k {
        "JC_OIDC_ISSUER" => Some(issuer.to_owned()),
        "JC_OIDC_CLIENT_ID" => Some("joinedcontext-portal".to_owned()),
        "JC_OIDC_CLIENT_SECRET" => Some("test-secret".to_owned()),
        "JC_PORTAL_COOKIE_KEY" => Some("k".repeat(64)),
        "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_ID" => Some("portal-reconciler".to_owned()),
        "JC_PORTAL_KEYCLOAK_ADMIN_CLIENT_SECRET" => Some("admin-secret".to_owned()),
        "JC_KEYCLOAK_ADMIN_URL" => admin_url.map(str::to_owned),
        _ => None,
    })
}

/// The paths each realm was asked, in order.
async fn asked(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect()
}

/// Reads one person through the Portal's admin client, as the People page does.
async fn read_one_person(config: Config) {
    let state = AppState::from_config(config).await.expect("discovery");
    let people = state.people.as_ref().expect("an admin client");
    let user = people
        .admin()
        .await
        .expect("a token")
        .get("ada-id")
        .await
        .expect("the user");
    assert_eq!(user.id, "ada-id");
}

#[tokio::test]
async fn set_the_token_and_admin_calls_go_to_it_and_discovery_stays_on_the_issuer() {
    let public = realm().await;
    let service = realm().await;
    let issuer = format!("{}{REALM}", public.uri());
    let config = config(&issuer, Some(&service.uri())).expect("config");
    assert_eq!(config.oidc.as_ref().expect("oidc").issuer.as_str(), issuer);
    read_one_person(config).await;

    let public = asked(&public).await;
    assert!(
        public
            .iter()
            .all(|r| r.starts_with("GET ") && !r.contains("/admin/")),
        "only discovery at the issuer: {public:?}"
    );
    assert!(public
        .iter()
        .any(|r| r.ends_with("/.well-known/openid-configuration")));
    assert_eq!(
        asked(&service).await,
        [
            "POST /realms/hel/protocol/openid-connect/token",
            "GET /admin/realms/hel/users/ada-id"
        ]
    );
}

#[tokio::test]
async fn unset_every_call_goes_to_the_issuer_as_before() {
    let public = realm().await;
    let issuer = format!("{}{REALM}", public.uri());
    read_one_person(config(&issuer, None).expect("config")).await;
    let public = asked(&public).await;
    assert!(public.contains(&"POST /realms/hel/protocol/openid-connect/token".to_owned()));
    assert!(public.contains(&"GET /admin/realms/hel/users/ada-id".to_owned()));
}

#[test]
fn only_an_http_origin_is_taken_and_the_backchannel_keeps_the_realm_path() {
    let issuer = "https://idm.dev.example/realms/hel/";
    for bad in [
        "keycloak:8080",
        "ftp://keycloak:8080",
        "http://keycloak:8080/auth",
        "http://user:pw@keycloak:8080",
        "http://keycloak:8080/?x=1",
    ] {
        let err = config(issuer, Some(bad)).map(|_| ()).expect_err(bad);
        let ConfigError::Invalid { var, reason } = err;
        assert_eq!(var, "JC_KEYCLOAK_ADMIN_URL", "{bad}");
        assert!(
            !reason.contains("pw"),
            "no credential in the reason: {reason}"
        );
    }
    let oidc = |admin| config(issuer, admin).expect("config").oidc.expect("oidc");
    assert_eq!(
        oidc(Some("http://keycloak.keycloak.svc:8080")).backchannel(),
        "http://keycloak.keycloak.svc:8080/realms/hel"
    );
    assert_eq!(
        oidc(Some("http://keycloak.keycloak.svc:8080/")).backchannel(),
        "http://keycloak.keycloak.svc:8080/realms/hel"
    );
    assert_eq!(
        oidc(Some("  ")).backchannel(),
        "https://idm.dev.example/realms/hel"
    );
    assert_eq!(
        oidc(None).backchannel(),
        "https://idm.dev.example/realms/hel"
    );
}
