//! Every named MCP server gets its own sign-in client (EP-96, ADR-N-043 §2.2, T-3156).
//!
//! Against a mocked Keycloak admin API: a declared server gets `mcp-{project}-{name}`, public,
//! PKCE S256, consent on, the known AI-client callbacks only, with its own audience and the
//! person's groups and no other mapper; a console change is written back; a client of that id
//! this platform did not create is never touched; and a managed client whose server is gone is
//! deleted.

use joinedcontext_portal::config::ClientAuth;
use joinedcontext_portal::reconciler::app_clients::{
    audience_mapper, groups_mapper, organization_mappers,
};
use joinedcontext_portal::reconciler::groups::{MANAGED_BY, MANAGED_VALUE};
use joinedcontext_portal::reconciler::mcp_clients::{
    desired, McpClientSync, CALLBACKS, SERVER_ATTRIBUTE,
};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex, query_param};
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

fn server(project: &str, name: &str) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "McpServer".to_owned(),
        metadata: ObjectMeta::new(name, project),
        spec: json!({ "members": [{ "kind": "Endpoint", "name": "bikes", "namespace": "helsinki" }], "audience": "organization" }),
        status: None,
    }
}

fn mirror_with(items: Vec<ResourceEnvelope>) -> Mirror {
    let mirror = Mirror::new();
    for item in items {
        mirror.upsert(item);
    }
    mirror
}

async fn realm(listed: Value) -> MockServer {
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
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("briefRepresentation", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(listed))
        .mount(&keycloak)
        .await;
    // The organization's built-in `org/all` (EP-122) is declared on every run; here its client is
    // already as wanted, so it writes nothing. A test that wants it otherwise mounts its own.
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", "mcp-org-all"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([held("uuid-all", "org", "all")])),
        )
        .with_priority(20)
        .mount(&keycloak)
        .await;
    let mut own = audience_mapper("mcp-org-all");
    own["id"] = json!("all-a");
    let mut groups = groups_mapper();
    groups["id"] = json!("all-g");
    Mock::given(method("GET"))
        .and(path(format!(
            "{REALM}/clients/uuid-all/protocol-mappers/models"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            own,
            groups,
            org_held(0),
            org_held(1)
        ])))
        .with_priority(20)
        .mount(&keycloak)
        .await;
    for verb in ["POST", "PUT", "DELETE"] {
        Mock::given(method(verb))
            .and(path_regex(format!(
                "^{REALM}/clients(/[^/]+(/protocol-mappers/models.*)?)?$"
            )))
            .respond_with(ResponseTemplate::new(204))
            .with_priority(10)
            .mount(&keycloak)
            .await;
    }
    keycloak
}

fn sync(keycloak: &MockServer) -> McpClientSync {
    McpClientSync::new(
        &format!("{}/realms/bb", keycloak.uri()),
        "portal-api".to_owned(),
        ClientAuth::Secret("portal-api-credential".into()),
    )
    .expect("a realm issuer")
}

async fn lookup(keycloak: &MockServer, id: &str, found: Value) {
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", id))
        .respond_with(ResponseTemplate::new(200).set_body_json(found))
        .mount(keycloak)
        .await;
}

async fn mappers(keycloak: &MockServer, uuid: &str, held: Value) {
    Mock::given(method("GET"))
        .and(path(format!(
            "{REALM}/clients/{uuid}/protocol-mappers/models"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(held))
        .mount(keycloak)
        .await;
}

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

async fn bodies(keycloak: &MockServer, verb: &str, at: &str) -> Vec<Value> {
    keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == verb && r.url.path() == at)
        .map(|r| serde_json::from_slice(&r.body).expect("json"))
        .collect()
}

fn held(uuid: &str, project: &str, name: &str) -> Value {
    let mut client = desired(project, name);
    client["id"] = json!(uuid);
    client
}

/// EP-96: a declared server gets its public PKCE client with its own audience and the groups.
#[tokio::test]
async fn a_server_gets_a_public_pkce_client_naming_itself() {
    let keycloak = realm(json!([])).await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", "mcp-helsinki-mobility"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .up_to_n_times(1)
        .mount(&keycloak)
        .await;
    lookup(
        &keycloak,
        "mcp-helsinki-mobility",
        json!([held("uuid-m", "helsinki", "mobility")]),
    )
    .await;
    mappers(&keycloak, "uuid-m", json!([])).await;

    let outcomes = sync(&keycloak)
        .converge(&mirror_with(vec![server("helsinki", "mobility")]))
        .await;
    assert_eq!(outcomes.len(), 2, "the server and org/all: {outcomes:?}");
    assert!(outcomes.iter().all(|o| o.error.is_none()), "{outcomes:?}");
    assert_eq!(
        wrote(&keycloak).await,
        vec![
            format!("POST {REALM}/clients"),
            format!("POST {REALM}/clients/uuid-m/protocol-mappers/models"),
            format!("POST {REALM}/clients/uuid-m/protocol-mappers/models"),
            format!("POST {REALM}/clients/uuid-m/protocol-mappers/models"),
            format!("POST {REALM}/clients/uuid-m/protocol-mappers/models"),
        ]
    );
    let created = &bodies(&keycloak, "POST", &format!("{REALM}/clients")).await[0];
    assert_eq!(created["clientId"], "mcp-helsinki-mobility");
    assert_eq!(created["publicClient"], true);
    assert_eq!(created["consentRequired"], true);
    assert_eq!(created["attributes"]["pkce.code.challenge.method"], "S256");
    assert_eq!(created["attributes"][MANAGED_BY], MANAGED_VALUE);
    assert_eq!(created["attributes"][SERVER_ATTRIBUTE], "helsinki/mobility");
    assert_eq!(created["redirectUris"], json!(CALLBACKS));
    for flow in [
        "implicitFlowEnabled",
        "directAccessGrantsEnabled",
        "serviceAccountsEnabled",
    ] {
        assert_eq!(created[flow], false, "{flow}");
    }
    let sent = bodies(
        &keycloak,
        "POST",
        &format!("{REALM}/clients/uuid-m/protocol-mappers/models"),
    )
    .await;
    assert_eq!(
        sent,
        [
            vec![audience_mapper("mcp-helsinki-mobility"), groups_mapper()],
            organization_mappers().to_vec()
        ]
        .concat()
    );
}

/// A console change is written back, and a stray mapper that would widen the token is removed.
#[tokio::test]
async fn a_changed_client_is_written_back_and_a_stray_mapper_removed() {
    let keycloak = realm(json!([])).await;
    let mut changed = held("uuid-m", "helsinki", "mobility");
    changed["directAccessGrantsEnabled"] = json!(true);
    changed["redirectUris"] = json!(["https://evil.example/*"]);
    lookup(&keycloak, "mcp-helsinki-mobility", json!([changed])).await;
    let mut stray = audience_mapper("context-gateway");
    stray["id"] = json!("mapper-stray");
    let mut own = audience_mapper("mcp-helsinki-mobility");
    own["id"] = json!("mapper-own");
    let mut groups = groups_mapper();
    groups["id"] = json!("mapper-groups");
    mappers(
        &keycloak,
        "uuid-m",
        json!([own, groups, org_held(0), org_held(1), stray]),
    )
    .await;

    let outcomes = sync(&keycloak)
        .converge(&mirror_with(vec![server("helsinki", "mobility")]))
        .await;
    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    assert!(
        outcomes[0]
            .drift
            .iter()
            .any(|d| d.contains("directAccessGrantsEnabled")),
        "{outcomes:?}"
    );
    assert!(
        outcomes[0].drift.iter().any(|d| d.contains("redirectUris")),
        "{outcomes:?}"
    );
    assert_eq!(
        wrote(&keycloak).await,
        vec![
            format!("PUT {REALM}/clients/uuid-m"),
            format!("DELETE {REALM}/clients/uuid-m/protocol-mappers/models/mapper-stray"),
        ]
    );
    let put = &bodies(&keycloak, "PUT", &format!("{REALM}/clients/uuid-m")).await[0];
    assert_eq!(put["directAccessGrantsEnabled"], false);
    assert_eq!(put["redirectUris"], json!(CALLBACKS));
}

/// Keycloak answers the callbacks in its own order: the same set is no drift and writes nothing.
#[tokio::test]
async fn reordered_callbacks_are_no_drift_and_no_put() {
    let keycloak = realm(json!([])).await;
    let mut reordered = held("uuid-m", "helsinki", "mobility");
    let mut callbacks: Vec<&str> = CALLBACKS.to_vec();
    callbacks.reverse();
    reordered["redirectUris"] = json!(callbacks);
    lookup(&keycloak, "mcp-helsinki-mobility", json!([reordered])).await;
    let mut own = audience_mapper("mcp-helsinki-mobility");
    own["id"] = json!("mapper-own");
    let mut groups = groups_mapper();
    groups["id"] = json!("mapper-groups");
    mappers(
        &keycloak,
        "uuid-m",
        json!([own, groups, org_held(0), org_held(1)]),
    )
    .await;

    let outcomes = sync(&keycloak)
        .converge(&mirror_with(vec![server("helsinki", "mobility")]))
        .await;
    assert!(
        outcomes
            .iter()
            .all(|o| o.error.is_none() && o.drift.is_empty()),
        "{outcomes:?}"
    );
    assert_eq!(wrote(&keycloak).await, Vec::<String>::new());
}

/// T-6301: a server's client inherits the realm's optional scopes (the CIMD clients' `connector`
/// and `offline_access`) at creation; the pass unlinks both and reports it.
#[tokio::test]
async fn inherited_optional_scopes_are_unlinked_from_a_servers_client() {
    let keycloak = realm(json!([])).await;
    lookup(
        &keycloak,
        "mcp-helsinki-mobility",
        json!([held("uuid-m", "helsinki", "mobility")]),
    )
    .await;
    let mut own = audience_mapper("mcp-helsinki-mobility");
    own["id"] = json!("mapper-own");
    let mut groups = groups_mapper();
    groups["id"] = json!("mapper-groups");
    mappers(
        &keycloak,
        "uuid-m",
        json!([own, groups, org_held(0), org_held(1)]),
    )
    .await;
    let scopes = format!("{REALM}/clients/uuid-m/optional-client-scopes");
    Mock::given(method("GET"))
        .and(path(scopes.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "s-con", "name": "connector" },
            { "id": "s-off", "name": "offline_access" },
        ])))
        .with_priority(1)
        .mount(&keycloak)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(format!("^{scopes}/s-(con|off)$")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&keycloak)
        .await;

    let outcomes = sync(&keycloak)
        .converge(&mirror_with(vec![server("helsinki", "mobility")]))
        .await;
    let outcome = outcomes
        .iter()
        .find(|o| {
            o.drift
                .iter()
                .any(|d| d.contains("connector, offline_access"))
        })
        .unwrap_or_else(|| panic!("no outcome names the scopes: {outcomes:?}"));
    assert!(outcome.error.is_none(), "{outcome:?}");
    assert_eq!(
        wrote(&keycloak).await,
        vec![
            format!("DELETE {scopes}/s-con"),
            format!("DELETE {scopes}/s-off")
        ]
    );
}

/// A client of that id the platform did not create is left alone, and the server is reported.
#[tokio::test]
async fn a_foreign_client_of_that_id_is_never_touched() {
    let keycloak = realm(json!([])).await;
    lookup(
        &keycloak,
        "mcp-helsinki-mobility",
        json!([{ "id": "uuid-foreign", "clientId": "mcp-helsinki-mobility", "publicClient": true, "attributes": {} }]),
    )
    .await;
    let outcomes = sync(&keycloak)
        .converge(&mirror_with(vec![server("helsinki", "mobility")]))
        .await;
    assert!(
        outcomes[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("did not create")),
        "{outcomes:?}"
    );
    assert!(wrote(&keycloak).await.is_empty());
}

/// A managed server client whose server is gone is deleted; a foreign or another wave's is not.
#[tokio::test]
async fn the_client_of_a_removed_server_is_deleted_and_nothing_else() {
    let mut app_client = json!({ "id": "uuid-app", "clientId": "app-mapa", "attributes": { MANAGED_BY: MANAGED_VALUE, "joinedcontext.app": "helsinki/mapa" } });
    app_client["publicClient"] = json!(false);
    let keycloak = realm(json!([
        held("uuid-gone", "helsinki", "gone"),
        held("uuid-org", "org", "city"),
        app_client,
        { "id": "uuid-hand", "clientId": "mcp-helsinki-hand", "attributes": { SERVER_ATTRIBUTE: "helsinki/hand" } },
    ]))
    .await;
    lookup(
        &keycloak,
        "mcp-org-city",
        json!([held("uuid-org", "org", "city")]),
    )
    .await;
    let mut own = audience_mapper("mcp-org-city");
    own["id"] = json!("a");
    let mut groups = groups_mapper();
    groups["id"] = json!("g");
    mappers(
        &keycloak,
        "uuid-org",
        json!([own, groups, org_held(0), org_held(1)]),
    )
    .await;

    let outcomes = sync(&keycloak)
        .converge(&mirror_with(vec![server("org", "city")]))
        .await;
    assert!(outcomes.iter().all(|o| o.error.is_none()), "{outcomes:?}");
    assert_eq!(
        wrote(&keycloak).await,
        vec![format!("DELETE {REALM}/clients/uuid-gone")]
    );
    assert!(outcomes
        .iter()
        .any(|o| o.app == "helsinki/gone" && o.drift.iter().any(|d| d.contains("server is gone"))));
}

/// EP-122: `org/all` has no manifest, and still gets its client in a realm that has none.
#[tokio::test]
async fn org_all_gets_its_client_without_a_manifest() {
    let keycloak = realm(json!([])).await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients")))
        .and(query_param("clientId", "mcp-org-all"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .up_to_n_times(1)
        .mount(&keycloak)
        .await;

    let outcomes = sync(&keycloak).converge(&Mirror::new()).await;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert_eq!(outcomes[0].app, "org/all");
    assert_eq!(outcomes[0].error, None, "{outcomes:?}");
    let created = &bodies(&keycloak, "POST", &format!("{REALM}/clients")).await[0];
    assert_eq!(created["clientId"], "mcp-org-all");
    assert_eq!(created["publicClient"], true);
    assert_eq!(created["attributes"][SERVER_ATTRIBUTE], "org/all");
}

/// The Keycloak copy of the Organization mapper `index` (ADR-N-067 2.2), with its id.
fn org_held(index: usize) -> Value {
    let mut held = organization_mappers()[index].clone();
    held["id"] = json!(format!("org-{index}"));
    held
}
