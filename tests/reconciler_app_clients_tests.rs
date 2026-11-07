//! Every published App gets a Keycloak client of its own (ADR-N-030, AP-111, T-2677).
//!
//! Against a mocked Keycloak admin API: a published App with no client gets one created with its
//! redirects under `/apps/{name}/` and its secret read back; a client edited in the console is
//! written back and the drift reported; a client of that id that this platform did not create is
//! never touched and the App is reported; a managed client whose App is gone is deleted; a draft
//! App gets nothing; and no secret ever reaches an outcome.
//!
//! The App's roles (AP-113, T-2678): each `spec.roles[]` entry becomes a client role, each
//! `spec.access[]` subject a group or user role mapping, a role or mapping made in the console
//! is removed and reported, a subject the realm does not know yet is a warning, and the audience
//! mappers name `app-{name}` and the slug of every Endpoint the App reads, nothing else.
//!
//! Two credentials (T-3022): the login client `portal-api` writes the App's client and roles
//! and reads who holds each role; `portal-reconciler` looks up every group and user and writes
//! or removes every mapping. A refused member credential fails the roles, never the login.

use joinedcontext_portal::config::ClientAuth;
use std::sync::Arc;

use joinedcontext_portal::reconciler::app_clients::{
    audience_mapper, desired, groups_mapper, organization_mappers, record, AppClientSync,
};
use joinedcontext_portal::reconciler::groups::{MANAGED_BY, MANAGED_VALUE};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{body_string_contains, method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// T-6301: the client holds no optional scope, so the pass unlinks nothing.
async fn no_optional_scopes(keycloak: &MockServer) {
    Mock::given(method("GET"))
        .and(path_regex(
            "^/admin/realms/bb/clients/[^/]+/optional-client-scopes$",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .with_priority(5)
        .mount(keycloak)
        .await;
}

const REALM: &str = "/admin/realms/bb";
const HOST: &str = "city.example";

fn app(project: &str, name: &str, lifecycle: &str) -> ResourceEnvelope {
    app_with(project, name, lifecycle, json!({}))
}

/// A static App whose spec is `extra` over the smallest valid one.
fn app_with(project: &str, name: &str, lifecycle: &str, extra: Value) -> ResourceEnvelope {
    let mut spec = json!({
        "kind": "static",
        "source": { "path": "." },
        "build": { "node": "22" },
        "visibility": "project",
        "lifecycle": lifecycle,
        "dataNeeds": [],
    });
    for (key, value) in extra.as_object().into_iter().flatten() {
        spec[key] = value.clone();
    }
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "App".to_owned(),
        metadata: ObjectMeta::new(name, project),
        spec,
        status: None,
    }
}

/// The client `uuid` holds no role and the audience mapper of `app` already: a run writes
/// nothing about roles.
async fn roles_in_place(keycloak: &MockServer, uuid: &str, app: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients/{uuid}/roles")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .with_priority(10)
        .mount(keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{REALM}/clients/{uuid}/protocol-mappers/models"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            mapper("m-1", &format!("app-{app}")),
            groups_held(),
            org_held(0),
            org_held(1)
        ])))
        .with_priority(10)
        .mount(keycloak)
        .await;
}

fn mapper(id: &str, audience: &str) -> Value {
    let mut held = audience_mapper(audience);
    held["id"] = json!(id);
    held
}

/// The platform's `groups` mapper as the realm holds it on an App's client (AP-113, T-3033).
fn groups_held() -> Value {
    let mut held = groups_mapper();
    held["id"] = json!("m-groups");
    held
}

/// The Keycloak copy of the Organization mapper `index` (ADR-N-067 2.2), with its id.
fn org_held(index: usize) -> Value {
    let mut held = organization_mappers()[index].clone();
    held["id"] = json!(format!("m-org-{index}"));
    held
}

/// Every write the run may make about roles and mappers succeeds.
async fn writes_succeed(keycloak: &MockServer) {
    for verb in ["POST", "PUT", "DELETE"] {
        Mock::given(method(verb))
            .and(path_regex(format!(
                "^{REALM}/(clients/[^/]+/(roles|protocol-mappers)|groups|users)"
            )))
            .respond_with(ResponseTemplate::new(204))
            .with_priority(10)
            .mount(keycloak)
            .await;
    }
}

fn mirror_with(apps: Vec<ResourceEnvelope>) -> Mirror {
    let mirror = Mirror::new();
    for envelope in apps {
        mirror.upsert(envelope);
    }
    mirror
}

async fn realm() -> MockServer {
    let keycloak = MockServer::start().await;
    no_optional_scopes(&keycloak).await;
    Mock::given(method("POST"))
        .and(path("/realms/bb/protocol/openid-connect/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access_token": "admin-token", "expires_in": 60 })),
        )
        .mount(&keycloak)
        .await;
    keycloak
}

fn sync(keycloak: &MockServer) -> AppClientSync {
    AppClientSync::new(
        &format!("{}/realms/bb", keycloak.uri()),
        "portal-api".to_owned(),
        ClientAuth::Secret("portal-api-credential".into()),
        HOST.to_owned(),
    )
    .expect("a realm issuer")
}

/// The realm answers `GET /clients?clientId={id}` with `found`.
async fn client_lookup(keycloak: &MockServer, id: &str, found: Value) {
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", id))
        .respond_with(ResponseTemplate::new(200).set_body_json(found))
        .mount(keycloak)
        .await;
}

/// The list the wave reads to find managed clients no App names any more.
async fn app_client_list(keycloak: &MockServer, listed: Value) {
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("search", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(listed))
        .mount(keycloak)
        .await;
}

async fn secret_of(keycloak: &MockServer, uuid: &str, value: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients/{uuid}/client-secret")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "type": "secret", "value": value })),
        )
        .mount(keycloak)
        .await;
}

/// What the run wrote, as `METHOD path`.
async fn wrote(keycloak: &MockServer) -> Vec<String> {
    keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() != "GET" && !r.url.path().ends_with("/token"))
        .map(|r| format!("{} {}", r.method.as_str(), r.url.path()))
        .collect()
}

fn managed_client(uuid: &str, project: &str, name: &str) -> Value {
    // The fixtures' Apps carry no title, so the client is named after the App.
    let mut client = desired(project, name, name, HOST, &[]);
    client["id"] = json!(uuid);
    client
}

#[tokio::test]
async fn a_published_app_with_no_client_gets_one_and_its_secret_is_read_back() {
    let keycloak = realm().await;
    // Missing on the first lookup, present once created.
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", "app-bikes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .up_to_n_times(1)
        .mount(&keycloak)
        .await;
    client_lookup(
        &keycloak,
        "app-bikes",
        json!([managed_client("uuid-bikes", "helsinki", "bikes")]),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(format!("{REALM}/clients")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&keycloak)
        .await;
    secret_of(&keycloak, "uuid-bikes", "generated-by-keycloak").await;
    app_client_list(&keycloak, json!([])).await;
    Mock::given(method("GET"))
        .and(path_regex(format!(
            "^{REALM}/clients/uuid-bikes/(roles|protocol-mappers/models)$"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&keycloak)
        .await;
    writes_succeed(&keycloak).await;

    let run = sync(&keycloak)
        .converge(&mirror_with(vec![app("helsinki", "bikes", "published")]))
        .await;

    assert_eq!(run.outcomes.len(), 1);
    assert_eq!(run.outcomes[0].error, None, "{:?}", run.outcomes);
    assert_eq!(
        wrote(&keycloak).await,
        vec![
            format!("POST {REALM}/clients"),
            format!("POST {REALM}/clients/uuid-bikes/protocol-mappers/models"),
            format!("POST {REALM}/clients/uuid-bikes/protocol-mappers/models"),
            format!("POST {REALM}/clients/uuid-bikes/protocol-mappers/models"),
            format!("POST {REALM}/clients/uuid-bikes/protocol-mappers/models"),
        ]
    );
    // Its audience and the platform's groups mapper, so the person's token names their projects.
    let mappers = body_of(
        &keycloak,
        "POST",
        "/clients/uuid-bikes/protocol-mappers/models",
    )
    .await;
    assert_eq!(
        mappers[0]["config"]["included.custom.audience"],
        "app-bikes"
    );
    assert_eq!(mappers[1], groups_mapper());
    let created = keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|r| r.method.as_str() == "POST" && r.url.path() == format!("{REALM}/clients"))
        .expect("the client was created");
    let body: Value = serde_json::from_slice(&created.body).expect("json");
    assert_eq!(
        body["redirectUris"],
        json!(["https://bikes.apps.city.example/*"])
    );
    assert_eq!(body["directAccessGrantsEnabled"], false);
    assert_eq!(body["attributes"][MANAGED_BY], MANAGED_VALUE);
    assert_eq!(
        run.secrets.get("bikes").map(|s| s.expose()),
        Some("generated-by-keycloak")
    );
    assert!(
        !format!("{run:?}").contains("generated-by-keycloak"),
        "a secret never shows in a run's debug output"
    );
}

#[tokio::test]
async fn a_client_edited_in_the_console_is_written_back_and_the_drift_reported() {
    let keycloak = realm().await;
    let mut edited = managed_client("uuid-bikes", "helsinki", "bikes");
    edited["directAccessGrantsEnabled"] = json!(true);
    edited["redirectUris"] = json!(["*"]);
    client_lookup(&keycloak, "app-bikes", json!([edited])).await;
    Mock::given(method("PUT"))
        .and(path(format!("{REALM}/clients/uuid-bikes")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;
    secret_of(&keycloak, "uuid-bikes", "kept").await;
    app_client_list(&keycloak, json!([])).await;
    roles_in_place(&keycloak, "uuid-bikes", "bikes").await;

    let run = sync(&keycloak)
        .converge(&mirror_with(vec![app("helsinki", "bikes", "published")]))
        .await;

    assert_eq!(
        wrote(&keycloak).await,
        vec![format!("PUT {REALM}/clients/uuid-bikes")]
    );
    let drift = &run.outcomes[0].drift;
    assert_eq!(drift.len(), 2, "{drift:?}");
    assert!(drift
        .iter()
        .any(|d| d.starts_with("directAccessGrantsEnabled")));
    assert!(drift.iter().any(|d| d.starts_with("redirectUris")));
}

#[tokio::test]
async fn a_client_this_platform_did_not_create_is_never_touched() {
    let keycloak = realm().await;
    client_lookup(
        &keycloak,
        "app-bikes",
        json!([{ "id": "uuid-foreign", "clientId": "app-bikes", "attributes": {} }]),
    )
    .await;
    app_client_list(
        &keycloak,
        json!([{ "id": "uuid-foreign", "clientId": "app-bikes", "attributes": {} }]),
    )
    .await;

    let run = sync(&keycloak)
        .converge(&mirror_with(vec![app("helsinki", "bikes", "published")]))
        .await;

    assert!(wrote(&keycloak).await.is_empty());
    assert!(run.secrets.is_empty());
    let error = run.outcomes[0].error.as_deref().unwrap_or_default();
    assert!(error.contains("did not create"), "{error}");
}

#[tokio::test]
async fn a_managed_client_whose_app_is_gone_is_deleted_and_a_draft_gets_none() {
    let keycloak = realm().await;
    app_client_list(
        &keycloak,
        json!([
            managed_client("uuid-old", "helsinki", "old"),
            // An unmanaged `app-*` client of somebody else stays.
            { "id": "uuid-theirs", "clientId": "app-theirs", "attributes": {} },
            // Not an app client at all.
            { "id": "uuid-edge", "clientId": "edge", "attributes": { MANAGED_BY: MANAGED_VALUE } },
        ]),
    )
    .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{REALM}/clients/uuid-old")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;

    let foreign = Arc::new(joinedcontext_portal::reconciler::foreign::ForeignNames::default());
    let run = sync(&keycloak)
        .with_foreign(Arc::clone(&foreign))
        .converge(&mirror_with(vec![app("helsinki", "drafty", "draft")]))
        .await;

    assert_eq!(
        wrote(&keycloak).await,
        vec![format!("DELETE {REALM}/clients/uuid-old")]
    );
    assert_eq!(run.outcomes.len(), 1);
    assert_eq!(run.outcomes[0].app, "old");
    assert!(run.secrets.is_empty());
    // AP-114: the unmanaged `app-*` client is what the write doors refuse a new App for.
    assert!(foreign.has_client("app-theirs"));
    assert!(!foreign.has_client("app-old") && !foreign.has_client("edge"));
}

#[tokio::test]
async fn a_realm_that_refuses_the_token_is_one_failure_and_writes_nothing() {
    let keycloak = MockServer::start().await;
    no_optional_scopes(&keycloak).await;
    Mock::given(method("POST"))
        .and(path("/realms/bb/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&keycloak)
        .await;

    let run = sync(&keycloak)
        .converge(&mirror_with(vec![app("helsinki", "bikes", "published")]))
        .await;

    assert_eq!(run.outcomes.len(), 1);
    assert_eq!(run.outcomes[0].app, "*");
    assert!(run.outcomes[0].error.is_some());
    assert!(wrote(&keycloak).await.is_empty());
}

/// A published App with roles and access, whose client is already in place.
async fn client_in_place(keycloak: &MockServer) {
    client_lookup(
        keycloak,
        "app-alerts",
        json!([managed_client("uuid-a", "helsinki", "alerts")]),
    )
    .await;
    secret_of(keycloak, "uuid-a", "kept").await;
    app_client_list(keycloak, json!([])).await;
}

fn alerts(extra: Value) -> ResourceEnvelope {
    app_with("helsinki", "alerts", "published", extra)
}

async fn answer(keycloak: &MockServer, at: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(format!("{REALM}{at}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(keycloak)
        .await;
}

async fn body_of(keycloak: &MockServer, verb: &str, at: &str) -> Vec<Value> {
    keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == verb && r.url.path() == format!("{REALM}{at}"))
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect()
}

#[tokio::test]
async fn the_apps_roles_become_client_roles_mapped_to_its_groups_and_users_and_nothing_else() {
    let keycloak = realm().await;
    client_in_place(&keycloak).await;
    // Held: viewer, and `stale`, which somebody added in the console. After the run creates
    // steward, the list has both roles the App declares.
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients/uuid-a/roles")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "r-viewer", "name": "viewer" }, { "id": "r-stale", "name": "stale" },
        ])))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&keycloak)
        .await;
    answer(
        &keycloak,
        "/clients/uuid-a/roles",
        json!([
            { "id": "r-viewer", "name": "viewer" }, { "id": "r-steward", "name": "steward" },
        ]),
    )
    .await;
    // viewer is held by `editors` (a console mapping) and by jana, whom the App names.
    answer(
        &keycloak,
        "/clients/uuid-a/roles/viewer/groups",
        json!([{ "id": "g-editors", "name": "editors" }]),
    )
    .await;
    answer(
        &keycloak,
        "/clients/uuid-a/roles/viewer/users",
        json!([
            { "id": "u-jana", "username": "jana", "email": "Jana@hel.fi" },
            { "id": "u-eve", "username": "eve", "email": "eve@hel.fi" },
        ]),
    )
    .await;
    answer(&keycloak, "/clients/uuid-a/roles/steward/groups", json!([])).await;
    answer(&keycloak, "/clients/uuid-a/roles/steward/users", json!([])).await;
    answer(
        &keycloak,
        "/groups",
        json!([{ "id": "g-stewards", "name": "stewards" }]),
    )
    .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/users")))
        .and(query_param("email", "petra@hel.fi"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!([{ "id": "u-petra", "username": "petra", "email": "petra@hel.fi" }]),
        ))
        .mount(&keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/users")))
        .and(query_param("email", "nobody@hel.fi"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&keycloak)
        .await;
    answer(
        &keycloak,
        "/clients/uuid-a/protocol-mappers/models",
        json!([
            mapper("m-1", "app-alerts"),
            groups_held(),
            org_held(0),
            org_held(1)
        ]),
    )
    .await;
    writes_succeed(&keycloak).await;

    let run = sync(&keycloak)
        .converge(&mirror_with(vec![alerts(json!({
            "roles": [{ "name": "viewer" }, { "name": "steward" }],
            "access": [
                { "role": "viewer", "subjects": [{ "group": "stewards" }, { "user": "jana@hel.fi" }] },
                { "role": "steward", "subjects": [{ "user": "petra@hel.fi" }, { "group": "admins" }, { "user": "nobody@hel.fi" }] },
            ],
        }))]))
        .await;

    let outcome = &run.outcomes[0];
    assert_eq!(outcome.error, None, "{outcome:?}");
    let mut writes = wrote(&keycloak).await;
    writes.sort();
    let mut expected = vec![
        format!("DELETE {REALM}/clients/uuid-a/roles/stale"),
        format!("POST {REALM}/clients/uuid-a/roles"),
        format!("DELETE {REALM}/groups/g-editors/role-mappings/clients/uuid-a"),
        format!("DELETE {REALM}/users/u-eve/role-mappings/clients/uuid-a"),
        format!("POST {REALM}/groups/g-stewards/role-mappings/clients/uuid-a"),
        format!("POST {REALM}/users/u-petra/role-mappings/clients/uuid-a"),
    ];
    expected.sort();
    assert_eq!(
        writes, expected,
        "jana keeps her mapping; nothing else is written"
    );
    assert_eq!(
        body_of(&keycloak, "POST", "/clients/uuid-a/roles").await,
        vec![json!({ "name": "steward" })]
    );
    assert_eq!(
        body_of(
            &keycloak,
            "POST",
            "/groups/g-stewards/role-mappings/clients/uuid-a"
        )
        .await,
        vec![json!([{ "id": "r-viewer", "name": "viewer" }])]
    );
    assert_eq!(
        body_of(
            &keycloak,
            "POST",
            "/users/u-petra/role-mappings/clients/uuid-a"
        )
        .await,
        vec![json!([{ "id": "r-steward", "name": "steward" }])]
    );
    assert_eq!(outcome.drift.len(), 3, "{:?}", outcome.drift);
    assert!(outcome.drift.iter().any(|d| d.contains("stale")));
    assert!(outcome.drift.iter().any(|d| d.contains("editors")));
    assert!(outcome.drift.iter().any(|d| d.contains("eve@hel.fi")));
    assert_eq!(outcome.warnings.len(), 2, "{:?}", outcome.warnings);
    assert!(outcome.warnings.iter().any(|w| w.contains("group admins")));
    assert!(outcome
        .warnings
        .iter()
        .any(|w| w.contains("user nobody@hel.fi")));
    assert_eq!(
        run.secrets.get("alerts").map(|s| s.expose()),
        Some("kept"),
        "the login stays"
    );
}

/// ADR-N-061 (T-5711): an App names a person by their Keycloak user id. The holder of that id
/// keeps the role, a holder whose username spells the id loses it, a known id is mapped by that
/// id, and an id the realm does not have is a warning, never a lookup by e-mail or username.
#[tokio::test]
async fn an_apps_role_member_named_by_keycloak_id_is_matched_by_the_id_alone() {
    const JANA: &str = "0000000a-0000-4000-8000-00000000000a";
    const PETRA: &str = "0000000b-0000-4000-8000-00000000000b";
    const GHOST: &str = "0000000c-0000-4000-8000-00000000000c";
    let keycloak = realm().await;
    client_in_place(&keycloak).await;
    answer(
        &keycloak,
        "/clients/uuid-a/roles",
        json!([{ "id": "r-viewer", "name": "viewer" }]),
    )
    .await;
    answer(&keycloak, "/clients/uuid-a/roles/viewer/groups", json!([])).await;
    answer(
        &keycloak,
        "/clients/uuid-a/roles/viewer/users",
        json!([
            { "id": JANA, "username": "jana", "email": "jana@hel.fi" },
            { "id": "u-mallory", "username": PETRA, "email": "mallory@hel.fi" },
        ]),
    )
    .await;
    answer(
        &keycloak,
        &format!("/users/{PETRA}"),
        json!({ "id": PETRA, "username": "petra" }),
    )
    .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/users/{GHOST}")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&keycloak)
        .await;
    answer(
        &keycloak,
        "/clients/uuid-a/protocol-mappers/models",
        json!([
            mapper("m-1", "app-alerts"),
            groups_held(),
            org_held(0),
            org_held(1)
        ]),
    )
    .await;
    writes_succeed(&keycloak).await;

    let run = sync(&keycloak)
        .converge(&mirror_with(vec![alerts(json!({
            "roles": [{ "name": "viewer" }],
            "access": [
                { "role": "viewer", "subjects": [{ "user": JANA }, { "user": PETRA }, { "user": GHOST }] },
            ],
        }))]))
        .await;

    let outcome = &run.outcomes[0];
    assert_eq!(outcome.error, None, "{outcome:?}");
    let mut writes = wrote(&keycloak).await;
    writes.sort();
    let mut expected = vec![
        format!("DELETE {REALM}/users/u-mallory/role-mappings/clients/uuid-a"),
        format!("POST {REALM}/users/{PETRA}/role-mappings/clients/uuid-a"),
    ];
    expected.sort();
    assert_eq!(writes, expected, "jana keeps her mapping by id");
    assert_eq!(outcome.warnings.len(), 1, "{:?}", outcome.warnings);
    assert!(
        outcome.warnings[0].contains(GHOST),
        "{:?}",
        outcome.warnings
    );
}

#[tokio::test]
async fn the_audiences_are_the_app_and_every_endpoint_it_reads_and_nothing_else() {
    let keycloak = realm().await;
    client_in_place(&keycloak).await;
    answer(&keycloak, "/clients/uuid-a/roles", json!([])).await;
    let mut edited = mapper("m-app", "app-alerts");
    edited["config"]["access.token.claim"] = json!("false");
    answer(&keycloak, "/clients/uuid-a/protocol-mappers/models", json!([
        edited,
        groups_held(), org_held(0), org_held(1),
        mapper("m-old", "ep-gone"),
        // Another kind under the wave's prefix is still not an audience the App reads (T-2857).
        { "id": "m-theirs", "name": "audience-by-hand", "protocolMapper": "oidc-hardcoded-claim-mapper", "config": {} },
    ]))
    .await;
    writes_succeed(&keycloak).await;
    let endpoint = ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "Endpoint".to_owned(),
        metadata: ObjectMeta::new("air", "helsinki"),
        spec: json!({ "slug": "ep-air-1234", "contextSpaceRef": { "name": "air" } }),
        status: None,
    };
    let mirror = mirror_with(vec![
        alerts(json!({ "dataNeeds": [{
            "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
            "types": ["AirQualityObserved"],
            "operations": ["queryEntity"],
        }] })),
        endpoint,
    ]);

    let run = sync(&keycloak).converge(&mirror).await;

    let outcome = &run.outcomes[0];
    assert_eq!(outcome.error, None, "{outcome:?}");
    let mut writes = wrote(&keycloak).await;
    writes.sort();
    assert_eq!(
        writes,
        vec![
            format!("DELETE {REALM}/clients/uuid-a/protocol-mappers/models/m-old"),
            format!("DELETE {REALM}/clients/uuid-a/protocol-mappers/models/m-theirs"),
            format!("POST {REALM}/clients/uuid-a/protocol-mappers/models"),
            format!("PUT {REALM}/clients/uuid-a/protocol-mappers/models/m-app"),
        ]
    );
    let created = body_of(&keycloak, "POST", "/clients/uuid-a/protocol-mappers/models").await;
    assert_eq!(
        created[0]["config"]["included.custom.audience"],
        "ep-air-1234"
    );
    assert_eq!(created[0]["protocolMapper"], "oidc-audience-mapper");
    let written = body_of(
        &keycloak,
        "PUT",
        "/clients/uuid-a/protocol-mappers/models/m-app",
    )
    .await;
    assert_eq!(written[0]["config"]["access.token.claim"], "true");
    assert_eq!(outcome.drift.len(), 3, "{:?}", outcome.drift);
    assert_eq!(
        outcome.removed_mappers,
        vec!["oidc-hardcoded-claim-mapper mapper audience-by-hand".to_owned()]
    );
}

#[tokio::test]
async fn an_apps_client_carries_the_platforms_groups_mapper_and_keeps_it() {
    // T-3033: the person's App token named no project, so the gateway refused every
    // project-bound Endpoint the App reads, silently. The client carries the groups mapper every
    // platform client carries; a changed one is written back, and a foreign mapper that took its
    // name is removed before ours is written.
    let keycloak = realm().await;
    client_in_place(&keycloak).await;
    answer(&keycloak, "/clients/uuid-a/roles", json!([])).await;
    let mut widened = groups_held();
    widened["config"]["full.path"] = json!("false");
    answer(
        &keycloak,
        "/clients/uuid-a/protocol-mappers/models",
        json!([
            mapper("m-app", "app-alerts"),
            widened,
            org_held(0),
            org_held(1)
        ]),
    )
    .await;
    writes_succeed(&keycloak).await;

    let run = sync(&keycloak)
        .converge(&mirror_with(vec![alerts(json!({}))]))
        .await;

    let outcome = &run.outcomes[0];
    assert_eq!(outcome.error, None, "{outcome:?}");
    assert_eq!(
        wrote(&keycloak).await,
        vec![format!(
            "PUT {REALM}/clients/uuid-a/protocol-mappers/models/m-groups"
        )]
    );
    let written = body_of(
        &keycloak,
        "PUT",
        "/clients/uuid-a/protocol-mappers/models/m-groups",
    )
    .await;
    assert_eq!(written[0]["config"], groups_mapper()["config"]);
    assert_eq!(
        outcome.drift,
        vec!["the groups mapper was changed; it was written back".to_owned()]
    );
    assert!(outcome.removed_mappers.is_empty());
}

#[tokio::test]
async fn a_foreign_mapper_named_groups_is_removed_and_the_platforms_is_written() {
    let keycloak = realm().await;
    client_in_place(&keycloak).await;
    answer(&keycloak, "/clients/uuid-a/roles", json!([])).await;
    answer(
        &keycloak,
        "/clients/uuid-a/protocol-mappers/models",
        json!([
            mapper("m-app", "app-alerts"),
            { "id": "m-fake", "name": "groups", "protocolMapper": "oidc-hardcoded-claim-mapper",
              "config": { "claim.name": "groups", "claim.value": "helsinki-admins" } },
            org_held(0),
            org_held(1),
        ]),
    )
    .await;
    writes_succeed(&keycloak).await;

    let run = sync(&keycloak)
        .converge(&mirror_with(vec![alerts(json!({}))]))
        .await;

    let outcome = &run.outcomes[0];
    assert_eq!(outcome.error, None, "{outcome:?}");
    assert_eq!(
        wrote(&keycloak).await,
        vec![
            format!("DELETE {REALM}/clients/uuid-a/protocol-mappers/models/m-fake"),
            format!("POST {REALM}/clients/uuid-a/protocol-mappers/models"),
        ]
    );
    assert_eq!(
        body_of(&keycloak, "POST", "/clients/uuid-a/protocol-mappers/models").await,
        vec![groups_mapper()]
    );
    assert_eq!(
        outcome.removed_mappers,
        vec!["oidc-hardcoded-claim-mapper mapper groups".to_owned()]
    );
    assert!(!format!("{outcome:?}").contains("helsinki-admins"));
}

/// A client whose mappers are exactly the App's audiences: nothing to read back as wrong.
async fn only_the_apps_audience(keycloak: &MockServer, extra: Vec<Value>) {
    answer(keycloak, "/clients/uuid-a/roles", json!([])).await;
    let mut held = vec![
        mapper("m-app", "app-alerts"),
        groups_held(),
        org_held(0),
        org_held(1),
    ];
    held.extend(extra);
    answer(
        keycloak,
        "/clients/uuid-a/protocol-mappers/models",
        Value::Array(held),
    )
    .await;
}

#[tokio::test]
async fn a_mapper_the_app_does_not_declare_is_removed_and_the_app_names_it() {
    // T-2857: FGAP bounds which clients portal-api manages, never what a mapper writes, so a
    // hardcoded claim put on the client would mint tokens with a role the App never gave.
    let keycloak = realm().await;
    client_in_place(&keycloak).await;
    only_the_apps_audience(
        &keycloak,
        vec![
            json!({ "id": "m-claim", "name": "everyone-admin", "protocolMapper": "oidc-hardcoded-claim-mapper",
                    "config": { "claim.name": "realm_access.roles", "claim.value": "org-admin-7f3a" } }),
            json!({ "id": "m-aud", "name": "other\nendpoint", "protocolMapper": "oidc-audience-mapper",
                    "config": { "included.custom.audience": "ep-someone-else" } }),
        ],
    )
    .await;
    writes_succeed(&keycloak).await;
    let mut app = alerts(json!({}));
    app.status = Some(serde_json::from_value(json!({ "phase": "Live" })).expect("a status"));
    let mirror = mirror_with(vec![app]);

    let run = sync(&keycloak).converge(&mirror).await;

    let outcome = &run.outcomes[0];
    assert_eq!(outcome.error, None, "{outcome:?}");
    let mut writes = wrote(&keycloak).await;
    writes.sort();
    assert_eq!(
        writes,
        vec![
            format!("DELETE {REALM}/clients/uuid-a/protocol-mappers/models/m-aud"),
            format!("DELETE {REALM}/clients/uuid-a/protocol-mappers/models/m-claim"),
        ]
    );
    // Type and name only, the control character stripped; never the claim it wrote.
    assert_eq!(
        outcome.removed_mappers,
        vec![
            "oidc-hardcoded-claim-mapper mapper everyone-admin".to_owned(),
            "oidc-audience-mapper mapper otherendpoint".to_owned(),
        ]
    );
    assert!(!format!("{outcome:?}").contains("org-admin-7f3a"));

    record(&mirror, &run.outcomes);
    let status = mirror
        .find(|env| env.kind == "App" && env.metadata.name == "alerts")
        .and_then(|env| env.status)
        .expect("the App's status");
    let condition = status
        .conditions
        .iter()
        .find(|c| c.r#type == "ClientSynced")
        .expect("the condition");
    assert_eq!(condition.reason.as_deref(), Some("MapperRemoved"));
    let message = condition.message.as_deref().unwrap_or_default();
    assert!(message.contains("everyone-admin"), "{message}");
    assert!(!message.contains("org-admin-7f3a"), "{message}");
}

#[tokio::test]
async fn a_client_with_only_the_apps_audiences_is_not_written_and_the_app_keeps_its_status() {
    let keycloak = realm().await;
    client_in_place(&keycloak).await;
    only_the_apps_audience(&keycloak, Vec::new()).await;
    writes_succeed(&keycloak).await;
    let mut app = alerts(json!({}));
    app.status = Some(serde_json::from_value(json!({ "phase": "Live" })).expect("a status"));
    let mirror = mirror_with(vec![app]);

    let run = sync(&keycloak).converge(&mirror).await;
    record(&mirror, &run.outcomes);

    assert_eq!(run.outcomes[0].error, None, "{:?}", run.outcomes[0]);
    assert!(wrote(&keycloak).await.is_empty());
    assert!(run.outcomes[0].removed_mappers.is_empty());
    let status = mirror
        .find(|env| env.kind == "App" && env.metadata.name == "alerts")
        .and_then(|env| env.status)
        .expect("the App's status");
    assert!(status.conditions.is_empty(), "{:?}", status.conditions);
}

#[tokio::test]
async fn a_realm_that_fails_on_the_roles_keeps_the_login_and_says_so() {
    let keycloak = realm().await;
    client_in_place(&keycloak).await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients/uuid-a/roles")))
        .respond_with(ResponseTemplate::new(503))
        .mount(&keycloak)
        .await;

    let run = sync(&keycloak)
        .converge(&mirror_with(vec![alerts(json!({}))]))
        .await;

    let error = run.outcomes[0].error.as_deref().unwrap_or_default();
    assert!(error.starts_with("roles of app-alerts"), "{error}");
    assert!(error.contains("503"), "{error}");
    assert_eq!(run.secrets.get("alerts").map(|s| s.expose()), Some("kept"));
    assert!(wrote(&keycloak).await.is_empty());
}

#[tokio::test]
async fn a_realm_that_fails_on_the_roles_still_gets_the_audiences() {
    // T-2965: the audiences are what the login's token reaches. A role the realm refuses kept the
    // login on the edge while its token named no endpoint, so every read the App made was a 401.
    let keycloak = realm().await;
    client_in_place(&keycloak).await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients/uuid-a/roles")))
        .respond_with(ResponseTemplate::new(503))
        .mount(&keycloak)
        .await;
    answer(
        &keycloak,
        "/clients/uuid-a/protocol-mappers/models",
        json!([
            mapper("m-app", "app-alerts"),
            groups_held(),
            org_held(0),
            org_held(1)
        ]),
    )
    .await;
    writes_succeed(&keycloak).await;
    let endpoint = ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "Endpoint".to_owned(),
        metadata: ObjectMeta::new("air", "helsinki"),
        spec: json!({ "slug": "ep-air-1234", "contextSpaceRef": { "name": "air" } }),
        status: None,
    };
    let mirror = mirror_with(vec![
        alerts(json!({ "dataNeeds": [{
            "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
            "types": ["AirQualityObserved"],
            "operations": ["queryEntity"],
        }] })),
        endpoint,
    ]);

    let run = sync(&keycloak).converge(&mirror).await;

    let error = run.outcomes[0].error.as_deref().unwrap_or_default();
    assert!(error.starts_with("roles of app-alerts"), "{error}");
    assert!(error.contains("503"), "{error}");
    assert_eq!(run.secrets.get("alerts").map(|s| s.expose()), Some("kept"));
    let created = body_of(&keycloak, "POST", "/clients/uuid-a/protocol-mappers/models").await;
    assert_eq!(created.len(), 1, "{created:?}");
    assert_eq!(
        created[0]["config"]["included.custom.audience"],
        "ep-air-1234"
    );
}

/// A realm where `portal-api` and `portal-reconciler` each get a token of their own, so every
/// call shows which credential made it.
async fn realm_of_two() -> MockServer {
    let keycloak = MockServer::start().await;
    no_optional_scopes(&keycloak).await;
    Mock::given(method("POST"))
        .and(path("/realms/bb/protocol/openid-connect/token"))
        .and(body_string_contains("client_id=portal-reconciler"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access_token": "members-token", "expires_in": 60 })),
        )
        .with_priority(1)
        .mount(&keycloak)
        .await;
    Mock::given(method("POST"))
        .and(path("/realms/bb/protocol/openid-connect/token"))
        .and(body_string_contains("client_id=portal-api"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access_token": "clients-token", "expires_in": 60 })),
        )
        .mount(&keycloak)
        .await;
    keycloak
}

fn sync_of_two(keycloak: &MockServer) -> AppClientSync {
    sync(keycloak).with_members(
        "portal-reconciler".to_owned(),
        ClientAuth::Secret("portal-reconciler-credential".into()),
    )
}

/// Every admin call the run made, as `METHOD path` and the bearer token it carried.
async fn calls_by_token(keycloak: &MockServer) -> Vec<(String, String)> {
    keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path().starts_with(REALM))
        .map(|r| {
            let bearer = r
                .headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .trim_start_matches("Bearer ")
                .to_owned();
            (format!("{} {}", r.method.as_str(), r.url.path()), bearer)
        })
        .collect()
}

/// The alerts App's role `viewer` is held by the console group `editors` and the user eve; the
/// App gives it to the group `stewards` and to petra.
async fn viewer_held_by_others(keycloak: &MockServer) {
    client_in_place(keycloak).await;
    answer(
        keycloak,
        "/clients/uuid-a/roles",
        json!([{ "id": "r-viewer", "name": "viewer" }]),
    )
    .await;
    answer(
        keycloak,
        "/clients/uuid-a/roles/viewer/groups",
        json!([{ "id": "g-editors", "name": "editors" }]),
    )
    .await;
    answer(
        keycloak,
        "/clients/uuid-a/roles/viewer/users",
        json!([{ "id": "u-eve", "username": "eve", "email": "eve@hel.fi" }]),
    )
    .await;
    answer(
        keycloak,
        "/groups",
        json!([{ "id": "g-stewards", "name": "stewards" }]),
    )
    .await;
    answer(
        keycloak,
        "/users",
        json!([{ "id": "u-petra", "username": "petra", "email": "petra@hel.fi" }]),
    )
    .await;
    answer(
        keycloak,
        "/clients/uuid-a/protocol-mappers/models",
        json!([
            mapper("m-1", "app-alerts"),
            groups_held(),
            org_held(0),
            org_held(1)
        ]),
    )
    .await;
    writes_succeed(keycloak).await;
}

fn alerts_for_stewards_and_petra() -> ResourceEnvelope {
    alerts(json!({
        "roles": [{ "name": "viewer" }],
        "access": [{ "role": "viewer", "subjects": [{ "group": "stewards" }, { "user": "petra@hel.fi" }] }],
    }))
}

/// AP-113, T-3022: the holder reads run as `portal-api` (it holds `query-users`, read-only);
/// every lookup of a group or user and every mapping written or removed runs as
/// `portal-reconciler`, which alone holds `manage-users`.
#[tokio::test]
async fn role_holders_are_read_by_the_login_client_and_mapped_by_the_reconciler() {
    let keycloak = realm_of_two().await;
    viewer_held_by_others(&keycloak).await;

    let run = sync_of_two(&keycloak)
        .converge(&mirror_with(vec![alerts_for_stewards_and_petra()]))
        .await;

    let outcome = &run.outcomes[0];
    assert_eq!(outcome.error, None, "{outcome:?}");
    let calls = calls_by_token(&keycloak).await;
    // `METHOD /path` under the realm: the tokens every such call carried.
    let by = |at: &str| -> Vec<&str> {
        let (verb, at) = at.split_once(' ').expect("METHOD /path");
        calls
            .iter()
            .filter(|(call, _)| *call == format!("{verb} {REALM}{at}"))
            .map(|(_, token)| token.as_str())
            .collect()
    };
    for members_call in [
        "GET /groups",
        "GET /users",
        "DELETE /groups/g-editors/role-mappings/clients/uuid-a",
        "DELETE /users/u-eve/role-mappings/clients/uuid-a",
        "POST /groups/g-stewards/role-mappings/clients/uuid-a",
        "POST /users/u-petra/role-mappings/clients/uuid-a",
    ] {
        assert_eq!(
            by(members_call),
            vec!["members-token"],
            "{members_call}: {calls:?}"
        );
    }
    for clients_call in [
        "GET /clients/uuid-a/roles",
        "GET /clients/uuid-a/roles/viewer/groups",
        "GET /clients/uuid-a/roles/viewer/users",
        "GET /clients/uuid-a/client-secret",
    ] {
        assert_eq!(
            by(clients_call),
            vec!["clients-token"],
            "{clients_call}: {calls:?}"
        );
    }
    let members_paths: Vec<&String> = calls
        .iter()
        .filter(|(_, token)| token == "members-token")
        .map(|(call, _)| call)
        .collect();
    assert!(
        members_paths
            .iter()
            .all(|call| call.contains("/groups") || call.contains("/users")),
        "the member credential touches no client: {members_paths:?}"
    );
    assert_eq!(run.secrets.get("alerts").map(|s| s.expose()), Some("kept"));
}

/// T-3022: a member credential the realm refuses fails the App's roles with the client named,
/// writes no mapping, and keeps the App's login and audiences.
#[tokio::test]
async fn a_refused_member_credential_fails_the_roles_and_keeps_the_login() {
    let keycloak = MockServer::start().await;
    no_optional_scopes(&keycloak).await;
    Mock::given(method("POST"))
        .and(path("/realms/bb/protocol/openid-connect/token"))
        .and(body_string_contains("client_id=portal-reconciler"))
        .respond_with(ResponseTemplate::new(401))
        .with_priority(1)
        .mount(&keycloak)
        .await;
    Mock::given(method("POST"))
        .and(path("/realms/bb/protocol/openid-connect/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "access_token": "clients-token", "expires_in": 60 })),
        )
        .mount(&keycloak)
        .await;
    viewer_held_by_others(&keycloak).await;

    let run = sync_of_two(&keycloak)
        .converge(&mirror_with(vec![alerts_for_stewards_and_petra()]))
        .await;

    let error = run.outcomes[0].error.as_deref().unwrap_or_default();
    assert!(error.starts_with("roles of app-alerts"), "{error}");
    assert!(error.contains("portal-reconciler"), "{error}");
    assert!(error.contains("401"), "{error}");
    assert!(
        !error.contains("portal-reconciler-credential"),
        "no secret in an outcome: {error}"
    );
    assert!(
        wrote(&keycloak).await.is_empty(),
        "no mapping, no client write"
    );
    assert_eq!(run.secrets.get("alerts").map(|s| s.expose()), Some("kept"));
}
