//! Who may do what on one Endpoint (API/01 §57, EP-103, T-5531): the Portal asks the gateway's
//! `access/simulate` with no action once per principal the Endpoint's space names, with its own
//! token, and lays the grant documents out as a matrix and a graph.

mod common;

use std::sync::Arc;

use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::activity::ActivityFilter;
use joinedcontext_portal::auth::oidc::OidcClient;
use joinedcontext_portal::auth::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const ROUTE: &str = "/api/v1/projects/helsinki/endpoints/bikes-ops/access/matrix";

/// The realm: discovery and the Portal's own token.
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

fn policy(mirror: &Mirror, name: &str, space: &str, spec: Value) {
    let mut spec = spec;
    spec["contextSpaceRef"] = json!(space);
    spec["assigner"] = json!("did:web:hel.fi");
    mirror.upsert(common::envelope("Policy", name, "helsinki", spec));
}

/// helsinki-bikes-ops in space `bikes`: stewards write status, the public reads, a prohibition
/// keeps `owner` from the public; a Policy of space `air` names somebody else.
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
    for space in ["bikes", "air"] {
        mirror.upsert(common::envelope(
            "ContextSpace",
            space,
            "helsinki",
            json!({}),
        ));
    }
    mirror.upsert(common::envelope(
        "Endpoint",
        "bikes-ops",
        "helsinki",
        json!({ "slug": "helsinki-bikes-ops", "contextSpaceRef": "bikes" }),
    ));
    policy(
        &mirror,
        "stewards-write",
        "bikes",
        json!({ "effect": "permission", "assignee": { "kind": "group", "id": "bike-stewards" },
                "operations": ["updateOps"],
                "information": [{ "entities": [{ "type": "BikeHireDockingStation" }] }] }),
    );
    policy(
        &mirror,
        "public-read",
        "bikes",
        json!({ "effect": "permission", "assignee": { "kind": "role", "id": "public" },
                "operations": ["retrieveOps"] }),
    );
    policy(
        &mirror,
        "no-owner",
        "bikes",
        json!({ "effect": "prohibition", "assignee": { "kind": "role", "id": "public" },
                "operations": ["retrieveOps"],
                "information": [{ "entities": [{ "type": "BikeHireDockingStation" }] }] }),
    );
    policy(
        &mirror,
        "air-only",
        "air",
        json!({ "effect": "permission", "assignee": { "kind": "user", "id": "elsewhere@hel.fi" },
                "operations": ["retrieveOps"] }),
    );
    let state = AppState::new(config, Some(oidc)).with_mirror(Arc::new(mirror));
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

/// `who@hel.fi` reads `kinds` in `scope`.
fn reader(state: &AppState, who: &str, kinds: &[&str], scope: Value) -> Identity {
    state.mirror.upsert(common::envelope(
        "Role",
        &format!("{who}-role"),
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": kinds, "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        &format!("{who}-binding"),
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": format!("{who}-role"),
                "scope": scope }),
    ));
    let mut person = common::person(who);
    person.username = format!("{who}@hel.fi");
    person.email = Some(format!("{who}@hel.fi"));
    person
}

/// The gateway's grant documents: the stewards write status on the stations, the public reads
/// everything but `owner` of the stations, everyone else is admitted to nothing.
async fn gateway_answers(gateway: &MockServer) {
    let stewards = json!({
        "admitted": true, "policies": ["stewards-write"],
        "permissions": [{ "resource": { "type": "BikeHireDockingStation" },
                          "actions": ["updateEntity", "appendAttrs"], "attributes": ["status"],
                          "constraints": { "q": "operator==\"HSL\"" } }],
        "prohibitions": []
    });
    let public = json!({
        "admitted": true, "policies": ["public-read", "no-owner"],
        "permissions": [{ "resource": { "type": "BikeHireDockingStation" },
                          "actions": ["queryEntity", "retrieveEntity", "queryTemporalEntities"],
                          "attributes": "*", "constraints": {} }],
        "prohibitions": [{ "resource": { "type": "BikeHireDockingStation" },
                           "actions": ["queryEntity"], "attributes": ["owner"] }]
    });
    for (subject, answer) in [
        (json!({ "groups": ["bike-stewards"] }), stewards),
        (json!({ "groups": [], "roles": [] }), public),
    ] {
        Mock::given(method("POST"))
            .and(path("/api/endpoint/helsinki-bikes-ops/access/simulate"))
            .and(header("authorization", "Bearer the-portals-own-token"))
            .and(body_partial_json(json!({ "subject": subject })))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .mount(gateway)
            .await;
    }
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

async fn matrix(state: &AppState, who: Identity, route: &str) -> common::Answer {
    common::send(state, who, "GET", route, None).await
}

fn cell<'a>(matrix: &'a Value, principal: &str, action: &str, effect: &str) -> &'a Value {
    matrix["cells"]
        .as_array()
        .expect("cells")
        .iter()
        .find(|c| c["principal"] == principal && c["action"] == action && c["effect"] == effect)
        .unwrap_or_else(|| panic!("no {principal} {action} {effect} cell in {matrix}"))
}

/// An administrator reads every principal the space names with what the gateway grants them;
/// the gateway is asked with the Portal's own token, once per principal, never with an action.
#[tokio::test]
async fn an_administrator_reads_the_gateways_grants_per_principal() {
    let r = rig().await;
    gateway_answers(&r.gateway).await;
    let answer = matrix(&r.state, admin(), ROUTE).await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    let m = body(&answer);
    let principals: Vec<String> = m["principals"]
        .as_array()
        .expect("principals")
        .iter()
        .map(|p| {
            format!(
                "{}:{}",
                p["kind"].as_str().unwrap_or_default(),
                p["name"].as_str().unwrap_or_default()
            )
        })
        .collect();
    assert_eq!(principals, ["group:bike-stewards", "public:public"]);
    assert_eq!(m["types"], json!(["BikeHireDockingStation"]));
    assert_eq!(m["space"], "bikes");

    let write = cell(&m, "group:bike-stewards", "write", "permit");
    assert_eq!(write["attributes"], json!(["status"]));
    assert_eq!(write["constraint"], "q: operator==\"HSL\"");
    assert_eq!(write["decidedBy"], "stewards-write");
    let read = cell(&m, "public:public", "read", "permit");
    assert_eq!(read["attributes"], "*");
    assert_eq!(read["constraint"], Value::Null);
    assert_eq!(read["decidedBy"], "public-read");
    assert_eq!(
        cell(&m, "public:public", "history", "permit")["decidedBy"],
        "public-read"
    );
    let prohibited = cell(&m, "public:public", "read", "prohibit");
    assert_eq!(prohibited["attributes"], json!(["owner"]));
    assert_eq!(prohibited["decidedBy"], "no-owner");
    assert_eq!(m["cells"].as_array().map(Vec::len), Some(4), "{m}");

    let edges: Vec<(String, String)> = m["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|e| {
            (
                e["from"].as_str().unwrap_or_default().to_owned(),
                e["to"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    for wanted in [
        ("principal:group:bike-stewards", "policy:stewards-write"),
        ("policy:stewards-write", "endpoint:bikes-ops"),
        ("principal:public:public", "policy:no-owner"),
        ("endpoint:bikes-ops", "type:BikeHireDockingStation"),
    ] {
        assert!(
            edges.contains(&(wanted.0.to_owned(), wanted.1.to_owned())),
            "{wanted:?} in {edges:?}"
        );
    }

    // Another space's assignee is never asked about and never named.
    assert!(!answer.text.contains("elsewhere"), "{}", answer.text);
    assert!(!answer.text.contains("air-only"), "{}", answer.text);
    let questions = asked(&r.gateway).await;
    assert_eq!(questions.len(), 2, "{questions:?}");
    for question in questions {
        assert!(question.get("action").is_none(), "{question}");
        assert!(question.get("resource").is_none(), "{question}");
    }
}

/// The agreement: every cell the matrix shows is a column of the grant document the gateway
/// answered for that principal, and every operation the document grants has its cell.
#[tokio::test]
async fn every_cell_agrees_with_the_gateways_document() {
    let r = rig().await;
    gateway_answers(&r.gateway).await;
    let m = body(&matrix(&r.state, admin(), ROUTE).await);
    let expected = [
        ("group:bike-stewards", "write", "permit"),
        ("public:public", "read", "permit"),
        ("public:public", "history", "permit"),
        ("public:public", "read", "prohibit"),
    ];
    let got: Vec<(String, String, String)> = m["cells"]
        .as_array()
        .expect("cells")
        .iter()
        .map(|c| {
            let text = |member: &str| c[member].as_str().unwrap_or_default().to_owned();
            (text("principal"), text("action"), text("effect"))
        })
        .collect();
    let expected: Vec<(String, String, String)> = expected
        .iter()
        .map(|(p, a, e)| ((*p).to_owned(), (*a).to_owned(), (*e).to_owned()))
        .collect();
    assert_eq!(got, expected);
}

/// A reader of Policies in the Endpoint's space reads the matrix as an administrator does.
#[tokio::test]
async fn a_policy_reader_of_the_space_reads_it() {
    let r = rig().await;
    gateway_answers(&r.gateway).await;
    let who = reader(
        &r.state,
        "pia",
        &["Policy", "Endpoint"],
        json!({ "contextSpace": "bikes" }),
    );
    let answer = matrix(&r.state, who, ROUTE).await;
    assert_eq!(answer.status, 200, "{}", answer.text);
}

/// PF-59: a ServiceAccount no Policy of the space names (granted through a role or a group) is
/// a row for an administrator, and invisible to a Policy reader of the space who may not read
/// ServiceAccounts: never asked about, never named.
#[tokio::test]
async fn a_service_account_shows_only_to_who_may_read_it() {
    let r = rig().await;
    r.state.mirror.upsert(common::envelope(
        "ServiceAccount",
        "sensors",
        "helsinki",
        json!({}),
    ));
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({ "subject": { "serviceAccount": "helsinki-sensors" } }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "admitted": true, "policies": [],
            "permissions": [{ "resource": { "type": "BikeHireDockingStation" },
                              "actions": ["upsertEntity"], "attributes": "*" }],
            "prohibitions": []
        })))
        .mount(&r.gateway)
        .await;
    gateway_answers(&r.gateway).await;

    let answer = matrix(&r.state, admin(), ROUTE).await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    let m = body(&answer);
    assert_eq!(
        cell(&m, "serviceAccount:helsinki-sensors", "write", "permit")["attributes"],
        "*"
    );

    let who = reader(
        &r.state,
        "pia",
        &["Policy", "Endpoint"],
        json!({ "contextSpace": "bikes" }),
    );
    r.gateway.reset().await;
    gateway_answers(&r.gateway).await;
    let answer = matrix(&r.state, who, ROUTE).await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    assert!(!answer.text.contains("sensors"), "{}", answer.text);
    for question in asked(&r.gateway).await {
        assert!(
            question["subject"].get("serviceAccount").is_none(),
            "{question}"
        );
    }
}

/// A member who reads no Policy, or reads them only in another space, is refused before the
/// gateway is asked.
#[tokio::test]
async fn a_member_without_policy_reads_is_refused() {
    let r = rig().await;
    gateway_answers(&r.gateway).await;
    let endpoints_only = reader(
        &r.state,
        "eve",
        &["Endpoint"],
        json!({ "project": "helsinki" }),
    );
    let answer = matrix(&r.state, endpoints_only, ROUTE).await;
    assert_eq!(answer.status, 403, "{}", answer.text);
    let other_space = reader(
        &r.state,
        "olli",
        &["Policy"],
        json!({ "contextSpace": "air" }),
    );
    let answer = matrix(&r.state, other_space, ROUTE).await;
    assert_eq!(answer.status, 403, "{}", answer.text);
    assert!(asked(&r.gateway).await.is_empty());
}

#[tokio::test]
async fn an_anonymous_caller_is_401() {
    let r = rig().await;
    let response = joinedcontext_portal::server::app(r.state.clone())
        .oneshot(
            axum::http::Request::builder()
                .uri(ROUTE)
                .body(axum::body::Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn an_unknown_endpoint_is_404_and_a_bad_kind_400() {
    let r = rig().await;
    let answer = matrix(
        &r.state,
        admin(),
        "/api/v1/projects/helsinki/endpoints/nope/access/matrix",
    )
    .await;
    assert_eq!(answer.status, 404, "{}", answer.text);
    let answer = matrix(&r.state, admin(), &format!("{ROUTE}?kind=did")).await;
    assert_eq!(answer.status, 400, "{}", answer.text);
}

/// Over 50 principals is a 422 that asks for a kind; `?kind=` narrows below the cap.
#[tokio::test]
async fn over_fifty_principals_asks_for_a_kind() {
    let r = rig().await;
    gateway_answers(&r.gateway).await;
    for n in 0..60 {
        policy(
            &r.state.mirror,
            &format!("person-{n}"),
            "bikes",
            json!({ "effect": "permission", "assignee": { "kind": "user", "id": format!("p{n}@hel.fi") },
                    "operations": ["retrieveOps"] }),
        );
    }
    let answer = matrix(&r.state, admin(), ROUTE).await;
    assert_eq!(answer.status, 422, "{}", answer.text);
    assert!(answer.text.contains("kind"), "{}", answer.text);
    assert!(asked(&r.gateway).await.is_empty());
    let answer = matrix(&r.state, admin(), &format!("{ROUTE}?kind=group")).await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    assert_eq!(body(&answer)["principals"][0]["name"], "bike-stewards");
}

/// One `access.simulated` event per matrix read, naming the reader and the Endpoint.
#[tokio::test]
async fn every_read_is_recorded_once() {
    let r = rig().await;
    gateway_answers(&r.gateway).await;
    let answer = matrix(&r.state, admin(), ROUTE).await;
    assert_eq!(answer.status, 200, "{}", answer.text);
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
        "anna read the access matrix of bikes-ops: 2 principals"
    );
    assert_eq!(event.details["matrix"], true);
}

/// A gateway that does not answer is the Portal's 503, never an empty matrix.
#[tokio::test]
async fn a_gateway_that_fails_is_503() {
    let r = rig().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&r.gateway)
        .await;
    let answer = matrix(&r.state, admin(), ROUTE).await;
    assert_eq!(answer.status, 503, "{}", answer.text);
}
