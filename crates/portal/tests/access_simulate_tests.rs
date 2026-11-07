//! Trying a Policy on a person (API/01 §39, EP-103, T-3311): an organization administrator asks,
//! the Portal resolves the subject through the realm's admin API and asks the gateway with its
//! own token, never the administrator's; every question is recorded.

mod common;

use joinedcontext_portal::config::ClientAuth;
use std::sync::Arc;

use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::activity::ActivityFilter;
use joinedcontext_portal::auth::oidc::OidcClient;
use joinedcontext_portal::auth::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::people::People;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const ADMIN: &str = "/admin/realms/helsinki";
const GATEWAY: &str = "/api/endpoint/air-quality/access/simulate";
const ROUTE: &str = "/api/v1/projects/helsinki/endpoints/air/access/simulate";

/// The realm: discovery, the Portal's own token, and one person, jana, a member of `stewards`
/// holding the realm role `viewer`.
async fn realm() -> MockServer {
    let realm = MockServer::start().await;
    let issuer = format!("{}/realms/helsinki", realm.uri());
    Mock::given(method("GET"))
        .and(path("/realms/helsinki/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["ES256"]
        })))
        .mount(&realm)
        .await;
    Mock::given(method("GET"))
        .and(path("/realms/helsinki/protocol/openid-connect/certs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "keys": [] })))
        .mount(&realm)
        .await;
    Mock::given(method("POST"))
        .and(path("/realms/helsinki/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "the-portals-own-token", "token_type": "Bearer", "expires_in": 300
        })))
        .mount(&realm)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{ADMIN}/users/jana-id")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "jana-id", "username": "jana@hel.fi", "email": "jana@hel.fi", "enabled": true,
            "createdTimestamp": 1_758_700_000_000_i64,
        })))
        .mount(&realm)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{ADMIN}/users/gone-id")))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({ "error": "User not found" })),
        )
        .mount(&realm)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{ADMIN}/users/jana-id/groups")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{ "id": "g1", "name": "stewards", "path": "/stewards" }])),
        )
        .mount(&realm)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{ADMIN}/users/jana-id/role-mappings/realm/composite"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{ "id": "r1", "name": "viewer" }])),
        )
        .mount(&realm)
        .await;
    realm
}

struct Rig {
    state: AppState,
    gateway: MockServer,
    _realm: MockServer,
}

async fn rig() -> Rig {
    let realm = realm().await;
    let gateway = MockServer::start().await;
    let issuer = format!("{}/realms/helsinki", realm.uri());
    let mut config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("test-secret"),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    config.gateway_url = Some(gateway.uri());
    let oidc = OidcClient::discover(
        config.oidc.as_ref().expect("a realm"),
        "https://portal.test/api/v1/auth/callback",
    )
    .await
    .expect("discovery");
    let mirror = Mirror::new();
    mirror.upsert(common::envelope(
        "Endpoint",
        "air",
        "helsinki",
        json!({ "slug": "air-quality", "contextSpaces": ["air"] }),
    ));
    mirror.upsert(common::envelope(
        "ServiceAccount",
        "sensors",
        "helsinki",
        json!({ "description": "the sensor feed" }),
    ));
    let mut state = AppState::new(config, Some(oidc)).with_mirror(Arc::new(mirror));
    state.people = People::new(
        &issuer,
        "portal-reconciler".into(),
        ClientAuth::Secret("secret".into()),
    )
    .map(Arc::new);
    Rig {
        state,
        gateway,
        _realm: realm,
    }
}

fn admin() -> Identity {
    let mut person = common::person("anna");
    person.groups = vec!["portal-approver".into()];
    person
}

/// The gateway grants, through the Policy `air-read`, whatever it is asked.
async fn granting(gateway: &MockServer) {
    Mock::given(method("POST"))
        .and(path(GATEWAY))
        .and(header("authorization", "Bearer the-portals-own-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "decision": true,
            "context": { "reason": "policy_grant_matched", "policy": "air-read", "assigner": "helsinki" }
        })))
        .mount(gateway)
        .await;
}

async fn asked(gateway: &MockServer) -> Vec<Value> {
    gateway
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|request| serde_json::from_slice(&request.body).expect("a JSON body"))
        .collect()
}

fn body(answer: &common::Answer) -> Value {
    serde_json::from_str(&answer.text).expect("a JSON answer")
}

/// EP-103: a person is asked about as their token would carry them, with the Portal's own
/// token, and the answer names the Policy that decided. The user is the Keycloak id, the `sub`
/// a Policy or binding names them by since ADR-N-061, never the username.
#[tokio::test]
async fn a_person_is_asked_about_with_their_groups_and_roles() {
    let r = rig().await;
    granting(&r.gateway).await;
    let answer = common::send(
        &r.state,
        admin(),
        "POST",
        ROUTE,
        Some(json!({
            "subject": { "kind": "person", "id": "jana-id" },
            "action": "retrieveEntity",
            "type": "AirQualityObserved"
        })),
    )
    .await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    let answer = body(&answer);
    assert_eq!(answer["decision"], true);
    assert_eq!(answer["reason"], "policy_grant_matched");
    assert_eq!(answer["policy"], "air-read");
    assert_eq!(
        answer["subject"],
        json!({ "user": "jana-id", "groups": ["stewards"], "roles": ["viewer"] })
    );
    assert_eq!(
        asked(&r.gateway).await,
        vec![json!({
            "subject": { "user": "jana-id", "groups": ["stewards"], "roles": ["viewer"] },
            "action": { "name": "retrieveEntity" },
            "resource": { "type": "AirQualityObserved" }
        })]
    );
}

/// A ServiceAccount is asked about by its derived client id; a group, a role and the public
/// carry only what they name.
#[tokio::test]
async fn every_other_kind_reaches_the_gateway_as_named() {
    let r = rig().await;
    granting(&r.gateway).await;
    for subject in [
        json!({ "kind": "serviceAccount", "name": "sensors" }),
        json!({ "kind": "group", "name": "stewards" }),
        json!({ "kind": "role", "name": "viewer" }),
        json!({ "kind": "public" }),
    ] {
        let answer = common::send(
            &r.state,
            admin(),
            "POST",
            ROUTE,
            Some(json!({ "subject": subject, "action": "queryEntity" })),
        )
        .await;
        assert_eq!(answer.status, 200, "{subject}: {}", answer.text);
    }
    let subjects: Vec<Value> = asked(&r.gateway)
        .await
        .into_iter()
        .map(|b| b["subject"].clone())
        .collect();
    assert_eq!(
        subjects,
        vec![
            json!({ "groups": [], "roles": [], "serviceAccount": "helsinki-sensors" }),
            json!({ "groups": ["stewards"], "roles": [] }),
            json!({ "groups": [], "roles": ["viewer"] }),
            json!({ "groups": [], "roles": [] }),
        ]
    );
}

/// Only an organization administrator may ask, and a refused caller never reaches the realm or
/// the gateway.
#[tokio::test]
async fn a_person_who_is_not_an_administrator_is_refused_before_anything_is_asked() {
    let r = rig().await;
    granting(&r.gateway).await;
    let answer = common::send(
        &r.state,
        common::person("eve"),
        "POST",
        ROUTE,
        Some(
            json!({ "subject": { "kind": "person", "id": "jana-id" }, "action": "retrieveEntity" }),
        ),
    )
    .await;
    assert_eq!(answer.status, 403, "{}", answer.text);
    assert!(asked(&r.gateway).await.is_empty());
    assert!(r
        ._realm
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .all(|request| !request.url.path().starts_with(ADMIN)));
}

/// A body the route cannot read is the caller's mistake, said before the gateway is asked.
#[tokio::test]
async fn a_question_the_route_cannot_read_is_a_bad_request() {
    let r = rig().await;
    granting(&r.gateway).await;
    for request in [
        json!({ "subject": { "kind": "robot" }, "action": "retrieveEntity" }),
        json!({ "subject": { "kind": "public", "id": "x" }, "action": "retrieveEntity" }),
        json!({ "subject": { "kind": "public" }, "action": "retrieveEntity", "space": "air" }),
        json!({ "subject": { "kind": "public" }, "action": "readEverything" }),
        json!({ "subject": { "kind": "group", "name": "  " }, "action": "retrieveEntity" }),
        json!({ "subject": { "kind": "role", "name": "a\nb" }, "action": "retrieveEntity" }),
        json!({ "subject": { "kind": "group", "name": "x".repeat(256) }, "action": "retrieveEntity" }),
        json!({ "subject": { "kind": "public" }, "action": "retrieveEntity", "type": "" }),
    ] {
        let answer = common::send(&r.state, admin(), "POST", ROUTE, Some(request.clone())).await;
        assert_eq!(answer.status, 400, "{request}: {}", answer.text);
    }
    assert!(asked(&r.gateway).await.is_empty());
}

/// No such Endpoint, person or ServiceAccount: 404, and the gateway is not asked.
#[tokio::test]
async fn an_unknown_endpoint_person_or_service_account_is_not_found() {
    let r = rig().await;
    granting(&r.gateway).await;
    for (uri, subject) in [
        (
            "/api/v1/projects/helsinki/endpoints/nowhere/access/simulate",
            json!({ "kind": "public" }),
        ),
        (ROUTE, json!({ "kind": "person", "id": "gone-id" })),
        (ROUTE, json!({ "kind": "serviceAccount", "name": "ghost" })),
    ] {
        let answer = common::send(
            &r.state,
            admin(),
            "POST",
            uri,
            Some(json!({ "subject": subject, "action": "retrieveEntity" })),
        )
        .await;
        assert_eq!(answer.status, 404, "{subject}: {}", answer.text);
    }
    assert!(asked(&r.gateway).await.is_empty());
}

/// A gateway that refuses the Portal's own token, or no gateway at all, is the operator's to
/// fix: 503, never the administrator's 403.
#[tokio::test]
async fn a_gateway_that_refuses_the_portal_or_is_absent_is_unavailable() {
    let r = rig().await;
    Mock::given(method("POST"))
        .and(path(GATEWAY))
        .respond_with(
            ResponseTemplate::new(403).set_body_json(json!({ "detail": "wrong audience" })),
        )
        .mount(&r.gateway)
        .await;
    let question = json!({ "subject": { "kind": "public" }, "action": "retrieveEntity" });
    let answer = common::send(&r.state, admin(), "POST", ROUTE, Some(question.clone())).await;
    assert_eq!(answer.status, 503, "{}", answer.text);

    let mut state = r.state.clone();
    let mut config = (*state.config).clone();
    config.gateway_url = None;
    state.config = Arc::new(config);
    let answer = common::send(&state, admin(), "POST", ROUTE, Some(question)).await;
    assert_eq!(answer.status, 503, "{}", answer.text);
}

/// EP-103: who asked about whom, on which Endpoint, and what the gateway said, is recorded.
#[tokio::test]
async fn every_question_is_recorded_with_both_subjects() {
    let r = rig().await;
    Mock::given(method("POST"))
        .and(path(GATEWAY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "decision": false, "context": { "reason": "no_grant" }
        })))
        .mount(&r.gateway)
        .await;
    let answer = common::send(
        &r.state,
        admin(),
        "POST",
        ROUTE,
        Some(json!({ "subject": { "kind": "person", "id": "jana-id" }, "action": "deleteEntity" })),
    )
    .await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    assert_eq!(body(&answer)["reason"], "no_grant");
    let page = r
        .state
        .activity
        .list(
            "helsinki",
            &ActivityFilter {
                kinds: vec!["access.simulated".into()],
                limit: 10,
                ..ActivityFilter::default()
            },
        )
        .await
        .expect("a page");
    let [event] = page.items.as_slice() else {
        panic!("one event, got {:?}", page.items);
    };
    assert_eq!(
        event.summary,
        "anna tried jana-id on air: deleteEntity refused"
    );
    assert_eq!(event.details["actor"], "anna");
    assert_eq!(event.details["subject"]["user"], "jana-id");
    assert_eq!(event.details["endpoint"], "air");
    assert_eq!(event.details["decision"], false);
}
