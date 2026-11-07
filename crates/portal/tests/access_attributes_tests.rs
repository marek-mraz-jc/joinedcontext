//! The Policy behind a refused attribute (API/01 §58, T-5528): the Portal asks the gateway's
//! `access/simulate` about the caller themself, with its own token, and names the Policy only to
//! a caller who may read it.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::auth::oidc::OidcClient;
use joinedcontext_portal::auth::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const GATEWAY: &str = "/api/endpoint/air-quality/access/simulate";
const ROUTE: &str =
    "/api/v1/projects/helsinki/endpoints/air/access/attributes?type=AirQualityObserved&attribute=no2&action=write";

/// The realm: discovery, its keys and the Portal's own token.
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
    realm
}

struct Rig {
    state: AppState,
    gateway: MockServer,
    _realm: MockServer,
}

/// One Endpoint `air` over the space `air`, whose model has `AirQualityObserved`, and the Policy
/// `no2-frozen` the gateway names. `viewer@hel.fi` reads the Endpoint; `steward@hel.fi` reads it
/// and the space's Policies.
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
    let mut policy = common::envelope(
        "Policy",
        "no2-frozen",
        "helsinki",
        json!({ "contextSpaceRef": { "name": "air" }, "effect": "prohibition", "assigner": "helsinki" }),
    );
    policy.metadata.title = serde_json::from_value(json!("NO2 is corrected by the lab only")).ok();
    for envelope in [
        common::envelope(
            "Endpoint",
            "air",
            "helsinki",
            json!({ "slug": "air-quality", "contextSpaceRef": { "name": "air" } }),
        ),
        common::envelope(
            "ContextSpace",
            "air",
            "helsinki",
            json!({ "dataModelRef": { "name": "air-model" } }),
        ),
        // Another space of the project, where `outsider@hel.fi` reads Endpoints.
        common::envelope("ContextSpace", "water", "helsinki", json!({})),
        common::envelope(
            "DataModel",
            "air-model",
            "helsinki",
            json!({ "classes": ["AirQualityObserved"] }),
        ),
        policy,
        common::envelope(
            "Role",
            "endpoint-reader",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Project", "ContextSpace", "Endpoint"], "verbs": ["read"] }] }),
        ),
        common::envelope(
            "Role",
            "policy-reader",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["Project", "ContextSpace", "Endpoint", "Policy"], "verbs": ["read"] }] }),
        ),
        common::envelope(
            "RoleBinding",
            "viewer-reads",
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": "viewer@hel.fi" }], "role": "endpoint-reader",
                    "scope": { "contextSpace": "air" } }),
        ),
        common::envelope(
            "RoleBinding",
            "steward-reads",
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": "steward@hel.fi" }], "role": "policy-reader",
                    "scope": { "contextSpace": "air" } }),
        ),
        common::envelope(
            "RoleBinding",
            "outsider-reads",
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": "outsider@hel.fi" }], "role": "endpoint-reader",
                    "scope": { "contextSpace": "water" } }),
        ),
    ] {
        mirror.upsert(envelope);
    }
    let state = AppState::new(config, Some(oidc)).with_mirror(Arc::new(mirror));
    Rig {
        state,
        gateway,
        _realm: realm,
    }
}

fn who(name: &str) -> Identity {
    let mut person = common::person(name);
    person.username = format!("{name}@hel.fi");
    person.groups = vec!["/stewards".into()];
    person.roles = vec!["viewer".into()];
    person
}

/// The gateway prohibits the write through `no2-frozen`, answering only the Portal's own token.
async fn prohibiting(gateway: &MockServer) {
    Mock::given(method("POST"))
        .and(path(GATEWAY))
        .and(header("authorization", "Bearer the-portals-own-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "decision": false,
            "context": { "reason": "prohibited", "policy": "no2-frozen", "assigner": "helsinki" }
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

/// A caller who may read the Policy reads its name and title; the gateway was asked about the
/// caller as their session names them, with the Portal's token.
#[tokio::test]
async fn a_policy_reader_reads_the_policy_that_refused_them() {
    let r = rig().await;
    prohibiting(&r.gateway).await;
    let answer = common::send(&r.state, who("steward"), "GET", ROUTE, None).await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    assert_eq!(
        body(&answer),
        json!({
            "type": "AirQualityObserved", "attribute": "no2", "action": "write",
            "decision": false, "reason": "prohibited",
            "policy": "no2-frozen", "policyTitle": "NO2 is corrected by the lab only",
            "policyHidden": false
        })
    );
    assert_eq!(
        asked(&r.gateway).await,
        vec![json!({
            "subject": { "user": "steward@hel.fi", "groups": ["stewards"], "roles": ["viewer"] },
            "action": { "name": "updateAttrs" },
            "resource": { "type": "AirQualityObserved", "properties": { "attribute": "no2" } }
        })]
    );
}

/// A caller who may read no Policy learns only that one decided; the gateway is not asked.
#[tokio::test]
async fn a_caller_without_policy_read_gets_no_name_and_no_question() {
    let r = rig().await;
    prohibiting(&r.gateway).await;
    let answer = common::send(&r.state, who("viewer"), "GET", ROUTE, None).await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    let answer = body(&answer);
    assert_eq!(answer["policy"], Value::Null);
    assert_eq!(answer["policyTitle"], Value::Null);
    assert_eq!(answer["policyHidden"], true);
    assert!(!answer.to_string().contains("no2-frozen"), "{answer}");
    assert!(asked(&r.gateway).await.is_empty());
}

/// Reading Policies of another space is not reading this one: the name stays hidden.
#[tokio::test]
async fn a_policy_the_caller_may_not_read_stays_hidden() {
    let r = rig().await;
    prohibiting(&r.gateway).await;
    r.state.mirror.upsert(common::envelope(
        "RoleBinding",
        "outsider-policies",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "viewer@hel.fi" }], "role": "policy-reader",
                "scope": { "contextSpace": "water" } }),
    ));
    let answer = common::send(&r.state, who("viewer"), "GET", ROUTE, None).await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    let answer = body(&answer);
    assert_eq!(answer["decision"], false);
    assert_eq!(answer["policy"], Value::Null);
    assert_eq!(answer["policyHidden"], true);
}

#[tokio::test]
async fn nobody_signed_in_is_refused() {
    let r = rig().await;
    let response = joinedcontext_portal::server::app(r.state.clone())
        .oneshot(Request::get(ROUTE).body(Body::empty()).expect("request"))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(asked(&r.gateway).await.is_empty());
}

/// Reading another space's Endpoints does not reach this one.
#[tokio::test]
async fn a_caller_who_may_not_read_the_endpoint_is_refused() {
    let r = rig().await;
    prohibiting(&r.gateway).await;
    let answer = common::send(&r.state, who("outsider"), "GET", ROUTE, None).await;
    assert_eq!(answer.status, 403, "{}", answer.text);
    assert!(asked(&r.gateway).await.is_empty());
}

/// An unknown Endpoint and an unknown type answer the same body.
#[tokio::test]
async fn an_unknown_endpoint_and_an_unknown_type_read_alike() {
    let r = rig().await;
    prohibiting(&r.gateway).await;
    let endpoint = common::send(
        &r.state,
        who("steward"),
        "GET",
        "/api/v1/projects/helsinki/endpoints/nope/access/attributes?type=AirQualityObserved&attribute=no2",
        None,
    )
    .await;
    let entity_type = common::send(
        &r.state,
        who("steward"),
        "GET",
        "/api/v1/projects/helsinki/endpoints/air/access/attributes?type=Parking&attribute=no2",
        None,
    )
    .await;
    assert_eq!(endpoint.status, 404, "{}", endpoint.text);
    assert_eq!(entity_type.status, 404, "{}", entity_type.text);
    let detail = |answer: &common::Answer| body(answer)["detail"].clone();
    assert_eq!(
        detail(&endpoint).as_str().map(|d| d.replace("nope", "air")),
        detail(&entity_type).as_str().map(str::to_owned)
    );
    assert!(asked(&r.gateway).await.is_empty());
}

#[tokio::test]
async fn a_malformed_question_names_its_parameter() {
    let r = rig().await;
    for (query, word) in [
        (
            "type=AirQualityObserved&attribute=no2&action=purge",
            "action",
        ),
        ("type=AirQualityObserved&attribute=%20", "attribute"),
        ("attribute=no2", "type"),
    ] {
        let answer = common::send(
            &r.state,
            who("steward"),
            "GET",
            &format!("/api/v1/projects/helsinki/endpoints/air/access/attributes?{query}"),
            None,
        )
        .await;
        assert_eq!(answer.status, 400, "{query}: {}", answer.text);
        assert!(answer.text.contains(word), "{query}: {}", answer.text);
    }
}
