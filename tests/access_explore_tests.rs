//! What a principal reaches (API/01 §41, EP-103, EP-59, T-3792): an organization administrator
//! asks, the Portal asks the gateway's `access/simulate` with no action for every Endpoint of the
//! project with its own token, and lists only the Endpoints the subject reaches.

mod common;

use joinedcontext_portal::config::ClientAuth;
use std::sync::Arc;

use serde_json::{json, Value};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::activity::ActivityFilter;
use joinedcontext_portal::auth::oidc::OidcClient;
use joinedcontext_portal::auth::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::people::People;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const ADMIN: &str = "/admin/realms/helsinki";
const ROUTE: &str = "/api/v1/projects/helsinki/access/explore";

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
        .and(path(format!("{ADMIN}/users/jana-id/consents")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "clientId": "mcp-hub", "grantedClientScopes": ["endpoint:air-quality"],
            "createdDate": 1_758_700_000_000_i64, "lastUpdatedDate": 1_758_800_000_000_i64,
        }])))
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
    for (name, slug) in [
        ("air", "air-quality"),
        ("bikes", "bikes"),
        ("secret", "secret-ops"),
        ("unserved", "unserved"),
    ] {
        // `bikes` lists nothing: it serves the default set (T-5699).
        let spec = if name == "bikes" {
            json!({ "slug": slug })
        } else {
            json!({ "slug": slug, "enabledRepresentations": ["ogc-features"] })
        };
        mirror.upsert(common::envelope("Endpoint", name, "helsinki", spec));
    }
    mirror.upsert(common::envelope(
        "McpServer",
        "city-air",
        "helsinki",
        json!({ "members": [{ "kind": "Endpoint", "name": "air" }] }),
    ));
    mirror.upsert(common::envelope(
        "McpServer",
        "ops",
        "helsinki",
        json!({ "members": [{ "kind": "Endpoint", "name": "secret" }] }),
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
    // Revoke is a decision door: it needs a second factor proven in the session (T-5567).
    person.second_factor = true;
    person
}

/// The gateway: `air-quality` and `bikes` grant, `secret-ops` refuses at the door, `unserved` is
/// not served. Every question must carry the Portal's own token and no action.
async fn gateway_answers(gateway: &MockServer) {
    let grant = |slug: &str, policy: &str| {
        json!({
            "admitted": true, "policies": [policy],
            "subject": { "type": "serviceAccount", "id": "helsinki-sensors" },
            "resource": { "type": "endpoint", "id": slug, "space": "air" },
            "permissions": [{ "resource": { "type": "AirQualityObserved" },
                              "actions": ["queryEntity"], "attributes": ["pm10"],
                              "constraints": { "geoQ": { "geometry": "Point", "coordinates": [24.9, 60.1], "georel": "near;maxDistance==500" } } }],
            "prohibitions": []
        })
    };
    for (slug, answer) in [
        ("air-quality", grant("air-quality", "air-read")),
        ("bikes", grant("bikes", "bikes-read")),
        ("secret-ops", json!({ "admitted": false })),
    ] {
        Mock::given(method("POST"))
            .and(path(format!("/api/endpoint/{slug}/access/simulate")))
            .and(header("authorization", "Bearer the-portals-own-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .mount(gateway)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/api/endpoint/unserved/access/simulate"))
        .respond_with(ResponseTemplate::new(404))
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

async fn explore(state: &AppState, who: Identity, subject: Value) -> common::Answer {
    common::send(
        state,
        who,
        "POST",
        ROUTE,
        Some(json!({ "subject": subject })),
    )
    .await
}

/// EP-59: the reached Endpoints are listed with their grant; one the audience refuses and one
/// the gateway does not serve are absent, their names nowhere in the answer.
#[tokio::test]
async fn only_the_endpoints_the_subject_reaches_are_listed() {
    let r = rig().await;
    gateway_answers(&r.gateway).await;
    let answer = explore(
        &r.state,
        admin(),
        json!({ "kind": "serviceAccount", "name": "sensors" }),
    )
    .await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    let explored = body(&answer);
    let names: Vec<&str> = explored["endpoints"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, ["air", "bikes"]);
    assert!(!answer.text.contains("secret"), "{}", answer.text);
    assert!(!answer.text.contains("unserved"), "{}", answer.text);
    assert!(!answer.text.contains("\"ops\""), "{}", answer.text);
    let air = &explored["endpoints"][0];
    assert_eq!(air["slug"], "air-quality");
    assert_eq!(air["space"], "air");
    assert_eq!(air["policies"], json!(["air-read"]));
    assert_eq!(
        air["representations"],
        json!(["ogc-features", "ngsi-ld", "mcp"])
    );
    assert_eq!(
        air["permissions"][0]["constraints"]["geoQ"]["geometry"],
        "Point"
    );
    // T-5699, F-POR1-020: an Endpoint listing nothing serves the catalogue's default set, not
    // `ngsi-ld` and `mcp` alone.
    let bikes = explored["endpoints"][1]["representations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for served in ["ngsi-ld", "mcp", "ogc-features"] {
        assert!(bikes.contains(&json!(served)), "{bikes:?}");
    }
    assert!(!bikes.contains(&json!("sta")), "{bikes:?}");
    assert_eq!(explored["mcpServers"], json!(["city-air"]));
    assert_eq!(explored["consents"], json!([]));
    for question in asked(&r.gateway).await {
        assert_eq!(
            question,
            json!({ "subject": { "groups": [], "roles": [], "serviceAccount": "helsinki-sensors" } })
        );
    }
}

/// A person is asked about with their groups and roles, and their consents come along.
#[tokio::test]
async fn a_persons_consents_come_with_their_rows() {
    let r = rig().await;
    gateway_answers(&r.gateway).await;
    let answer = explore(
        &r.state,
        admin(),
        json!({ "kind": "person", "id": "jana-id" }),
    )
    .await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    let explored = body(&answer);
    // The gateway takes `user` as the token `sub`: a person is simulated by their Keycloak id.
    assert_eq!(explored["subject"]["user"], "jana-id");
    assert_eq!(explored["consents"][0]["clientId"], "mcp-hub");
    assert_eq!(
        explored["consents"][0]["lastUpdatedDate"],
        1_758_800_000_000_i64
    );
}

/// A subject the gateway admits nowhere reaches nothing: an empty list, not an error.
#[tokio::test]
async fn a_subject_who_reaches_nothing_gets_an_empty_list() {
    let r = rig().await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({ "subject": { "groups": [], "roles": [] } }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "admitted": false })))
        .mount(&r.gateway)
        .await;
    let answer = explore(&r.state, admin(), json!({ "kind": "public" })).await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    assert_eq!(body(&answer)["endpoints"], json!([]));
}

/// Viewing another principal's access is an administrator's: anybody else is 403 before the
/// realm or the gateway is asked.
#[tokio::test]
async fn a_caller_who_is_not_an_administrator_is_refused() {
    let r = rig().await;
    gateway_answers(&r.gateway).await;
    let answer = explore(
        &r.state,
        common::person("eve"),
        json!({ "kind": "person", "id": "jana-id" }),
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

/// A person the realm does not hold, or a ServiceAccount of another project, is 404, and the
/// gateway is not asked; a body the route cannot read is 400.
#[tokio::test]
async fn a_subject_from_elsewhere_is_not_found() {
    let r = rig().await;
    gateway_answers(&r.gateway).await;
    for subject in [
        json!({ "kind": "person", "id": "gone-id" }),
        json!({ "kind": "serviceAccount", "name": "ghost" }),
    ] {
        let answer = explore(&r.state, admin(), subject.clone()).await;
        assert_eq!(answer.status, 404, "{subject}: {}", answer.text);
    }
    let answer = common::send(
        &r.state,
        admin(),
        "POST",
        "/api/v1/projects/elsewhere/access/explore",
        Some(json!({ "subject": { "kind": "serviceAccount", "name": "sensors" } })),
    )
    .await;
    assert_eq!(answer.status, 404, "{}", answer.text);
    for subject in [
        json!({ "kind": "robot" }),
        json!({ "kind": "group", "name": " " }),
    ] {
        let answer = explore(&r.state, admin(), subject.clone()).await;
        assert_eq!(answer.status, 400, "{subject}: {}", answer.text);
    }
    assert!(asked(&r.gateway).await.is_empty());
}

/// Who looked at whose access is recorded.
#[tokio::test]
async fn every_look_is_recorded() {
    let r = rig().await;
    gateway_answers(&r.gateway).await;
    let answer = explore(
        &r.state,
        admin(),
        json!({ "kind": "serviceAccount", "name": "sensors" }),
    )
    .await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    let page = r
        .state
        .activity
        .list(
            "helsinki",
            &ActivityFilter {
                kinds: vec!["access.explored".into()],
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
        "anna looked at what helsinki-sensors can access: 2 Endpoints"
    );
    assert_eq!(event.details["actor"], "anna");
}

/// Revoke removes one consent through the realm's admin API and is recorded; it needs the
/// rights sign-out needs.
#[tokio::test]
async fn revoking_a_consent_deletes_it_in_the_realm() {
    let r = rig().await;
    Mock::given(method("DELETE"))
        .and(path(format!("{ADMIN}/users/jana-id/consents/mcp-hub")))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&r._realm)
        .await;
    let uri = "/api/v1/organization/people/jana-id/consents/mcp-hub";
    // eve proves a second factor too, so her 403 is the missing right alone.
    let mut eve = common::person("eve");
    eve.second_factor = true;
    let refused = common::send(&r.state, eve, "DELETE", uri, None).await;
    assert_eq!(refused.status, 403, "{}", refused.text);
    assert!(
        !refused.text.contains("second-factor-required"),
        "{}",
        refused.text
    );
    let answer = common::send(&r.state, admin(), "DELETE", uri, None).await;
    assert_eq!(answer.status, 204, "{}", answer.text);
    let odd = common::send(
        &r.state,
        admin(),
        "DELETE",
        "/api/v1/organization/people/jana-id/consents/a.b",
        None,
    )
    .await;
    assert_eq!(odd.status, 404, "{}", odd.text);
}

/// T-4446: a project of 500 Endpoints is explored with at most EXPLORE_AT_ONCE questions in
/// flight, and every one that grants is still listed, in the Endpoints' order.
#[tokio::test]
async fn a_large_project_is_explored_a_bounded_number_at_a_time() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let r = rig().await;
    let (now, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (in_flight, peak) = (Arc::clone(&now), Arc::clone(&most));
    let gateway = axum::Router::new().route(
        "/api/endpoint/{slug}/access/simulate",
        axum::routing::post(
            move |axum::extract::Path(slug): axum::extract::Path<String>| {
                let (in_flight, peak) = (Arc::clone(&in_flight), Arc::clone(&peak));
                async move {
                    let at = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(at, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    axum::Json(json!({
                        "admitted": true, "policies": ["read"],
                        "resource": { "type": "endpoint", "id": slug, "space": "air" },
                        "permissions": [{ "resource": { "type": "AirQualityObserved" }, "actions": ["queryEntity"] }],
                        "prohibitions": []
                    }))
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move { axum::serve(listener, gateway).await });

    let mirror = Mirror::new();
    for i in 0..500 {
        mirror.upsert(common::envelope(
            "Endpoint",
            &format!("e{i:03}"),
            "helsinki",
            json!({ "slug": format!("s{i:03}"), "enabledRepresentations": ["ogc-features"] }),
        ));
    }
    mirror.upsert(common::envelope(
        "ServiceAccount",
        "sensors",
        "helsinki",
        json!({ "description": "the sensor feed" }),
    ));
    let mut state = r.state.clone().with_mirror(Arc::new(mirror));
    let mut config = (*state.config).clone();
    config.gateway_url = Some(format!("http://{address}"));
    state.config = Arc::new(config);

    let answer = explore(
        &state,
        admin(),
        json!({ "kind": "serviceAccount", "name": "sensors" }),
    )
    .await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    let names: Vec<String> = body(&answer)["endpoints"]
        .as_array()
        .expect("endpoints")
        .iter()
        .map(|row| row["name"].as_str().expect("a name").to_owned())
        .collect();
    let all: Vec<String> = (0..500).map(|i| format!("e{i:03}")).collect();
    assert_eq!(names, all);
    let peak = most.load(Ordering::SeqCst);
    assert!(
        (2..=joinedcontext_portal::api::access_explore::EXPLORE_AT_ONCE).contains(&peak),
        "{peak} questions in flight at once"
    );
}
