//! The MCP hub client's `endpoint:{slug}` scopes (T-2490, ADR-N-025 §4, EP-88).
//!
//! Against a mocked Keycloak admin API: an Endpoint that serves MCP gets its scope created and
//! linked to `mcp-hub` as an optional scope, one with `mcp: false` gets none; a managed scope
//! whose Endpoint is gone is deleted; a scope of a wanted name this platform did not create is
//! left alone and reported; an `endpoint:` scope linked to the hub that is not ours is unlinked,
//! and none stays a default scope; without the hub client nothing is written.

use joinedcontext_portal::config::ClientAuth;
use joinedcontext_portal::reconciler::hub_scopes::{
    desired, linked, serving, HubEndpoint, HubScopeSync, PENDING,
};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REALM: &str = "/admin/realms/bb";
const AIR: &str = "zt4qm7ge2xdv6ksb3ncf5arw2y";
const QUIET: &str = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

fn endpoint(project: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "Endpoint".to_owned(),
        metadata: ObjectMeta::new(name, project),
        spec,
        status: None,
    }
}

fn mirror() -> Mirror {
    let mirror = Mirror::new();
    mirror.upsert(endpoint(
        "ovzdusie",
        "air",
        json!({ "contextSpaceRef": "ovzdusie", "slug": AIR, "audience": "project-list",
                "allowedProjects": ["doprava"], "enabledRepresentations": ["ngsi-ld"] }),
    ));
    mirror.upsert(endpoint(
        "ovzdusie",
        "quiet",
        json!({ "contextSpaceRef": "ovzdusie", "slug": QUIET, "audience": "public",
                "enabledRepresentations": ["ngsi-ld"], "mcp": false }),
    ));
    mirror
}

fn air() -> HubEndpoint {
    HubEndpoint {
        project: "ovzdusie".to_owned(),
        name: "air".to_owned(),
        slug: AIR.to_owned(),
    }
}

/// A scope as the realm lists it: `desired` plus an id.
fn held(id: &str, mut scope: Value) -> Value {
    scope["id"] = json!(id);
    scope
}

async fn realm(hub: Option<Value>) -> MockServer {
    let keycloak = MockServer::start().await;
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
        .and(query_param("clientId", "mcp-hub"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!(hub.into_iter().collect::<Vec<_>>())),
        )
        .mount(&keycloak)
        .await;
    for verb in ["POST", "PUT", "DELETE"] {
        Mock::given(method(verb))
            .respond_with(ResponseTemplate::new(204))
            .with_priority(10)
            .mount(&keycloak)
            .await;
    }
    keycloak
}

/// The realm's scopes: `first` on the first listing, `then` on every later one.
async fn scopes(keycloak: &MockServer, first: Value, then: Value) {
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/client-scopes")))
        .respond_with(ResponseTemplate::new(200).set_body_json(first))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(keycloak)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/client-scopes")))
        .respond_with(ResponseTemplate::new(200).set_body_json(then))
        .with_priority(2)
        .mount(keycloak)
        .await;
}

async fn links(keycloak: &MockServer, kind: &str, linked: Value) {
    Mock::given(method("GET"))
        .and(path(format!("{REALM}/clients/hub-1/{kind}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(linked))
        .mount(keycloak)
        .await;
}

fn sync(keycloak: &MockServer) -> HubScopeSync {
    HubScopeSync::new(
        &format!("{}/realms/bb", keycloak.uri()),
        "portal-api".to_owned(),
        ClientAuth::Secret("portal-api-credential".into()),
    )
    .expect("a realm issuer")
}

/// The writes the run made, as `VERB path` lines.
async fn writes(keycloak: &MockServer) -> Vec<String> {
    keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| {
            request.method.as_str() != "GET" && !request.url.path().ends_with("/token")
        })
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect()
}

#[test]
fn every_endpoint_that_serves_mcp_has_one_scope_and_an_opt_out_has_none() {
    assert_eq!(serving(&mirror()), [air()]);
    let scope = desired(&air());
    assert_eq!(scope["name"], json!(format!("endpoint:{AIR}")));
    assert_eq!(scope["attributes"]["include.in.token.scope"], json!("true"));
    assert_eq!(
        scope["attributes"]["display.on.consent.screen"],
        json!("true")
    );
    assert_eq!(scope["attributes"]["managed-by"], json!("joinedcontext"));
    assert_eq!(
        scope["attributes"]["joinedcontext.endpoint"],
        json!("ovzdusie/air")
    );
}

#[test]
fn a_slug_that_is_not_opaque_names_no_scope() {
    let mirror = Mirror::new();
    for (name, slug) in [
        ("dots", "../../admin"),
        ("space", "a b c d e f g h"),
        ("upper", "ABCDEFGHIJKL"),
    ] {
        mirror.upsert(endpoint(
            "ovzdusie",
            name,
            json!({ "contextSpaceRef": "ovzdusie", "slug": slug, "audience": "public",
                    "enabledRepresentations": ["ngsi-ld"] }),
        ));
    }
    assert!(serving(&mirror).is_empty());
}

/// The first run: the scope is created, and waits for the deployment to link it to the hub
/// (T-3324): this client writes nothing on `mcp-hub`, which the T-2725 deny refuses.
#[tokio::test]
async fn a_serving_endpoint_gets_its_scope_created_and_waits_for_the_link() {
    let keycloak = realm(Some(json!({ "id": "hub-1", "clientId": "mcp-hub" }))).await;
    scopes(&keycloak, json!([]), json!([held("s-1", desired(&air()))])).await;
    links(&keycloak, "default-client-scopes", json!([])).await;
    links(&keycloak, "optional-client-scopes", json!([])).await;

    let outcomes = sync(&keycloak).converge(&mirror()).await;
    assert!(outcomes.iter().all(|o| o.error.is_none()), "{outcomes:?}");
    assert_eq!(
        writes(&keycloak).await,
        [format!("POST {REALM}/client-scopes")]
    );
    let waiting = outcomes
        .iter()
        .find(|o| o.app == air().scope())
        .unwrap_or_else(|| panic!("the unlinked scope was not reported: {outcomes:?}"));
    assert_eq!(waiting.warnings, [PENDING.to_owned()]);
    assert_eq!(
        linked(&outcomes, &air().scope()),
        Some(("False", "AwaitsDeploymentSync", PENDING.to_owned()))
    );
    let created = keycloak
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|request| {
            request.method.as_str() == "POST" && request.url.path().ends_with("/client-scopes")
        })
        .expect("the scope was created");
    let body: Value = serde_json::from_slice(&created.body).expect("a JSON body");
    assert_eq!(body, desired(&air()));
}

/// A converged realm: nothing is written.
#[tokio::test]
async fn a_converged_realm_is_left_as_it_is() {
    let keycloak = realm(Some(json!({ "id": "hub-1", "clientId": "mcp-hub" }))).await;
    let ours = held("s-1", desired(&air()));
    scopes(&keycloak, json!([ours.clone()]), json!([ours.clone()])).await;
    links(&keycloak, "default-client-scopes", json!([])).await;
    links(&keycloak, "optional-client-scopes", json!([ours])).await;

    let outcomes = sync(&keycloak).converge(&mirror()).await;
    assert!(outcomes.is_empty(), "{outcomes:?}");
    assert!(writes(&keycloak).await.is_empty());
    assert_eq!(
        linked(&outcomes, &air().scope()).map(|c| c.1),
        Some("Linked")
    );
}

/// Stale, foreign, stray and default: the stale scope is deleted, the rest reported; the hub's
/// links are the deployment's to fix (T-3324), so none is unlinked here.
#[tokio::test]
async fn the_hub_holds_our_scopes_only_and_none_as_a_default() {
    let keycloak = realm(Some(json!({ "id": "hub-1", "clientId": "mcp-hub" }))).await;
    let stale = held(
        "s-old",
        desired(&HubEndpoint {
            project: "ovzdusie".to_owned(),
            name: "retired".to_owned(),
            slug: "n4t8xq2vhm6zc9wrb5sdj3kfp7".to_owned(),
        }),
    );
    let foreign = json!({ "id": "s-foreign", "name": format!("endpoint:{AIR}"), "attributes": {} });
    let stray = json!({ "id": "s-stray", "name": "endpoint:hand-made", "attributes": {} });
    let unrelated = json!({ "id": "s-email", "name": "email", "attributes": {} });
    let listed = json!([stale, foreign, stray, unrelated]);
    scopes(&keycloak, listed.clone(), listed).await;
    links(
        &keycloak,
        "default-client-scopes",
        json!([{ "id": "s-stray", "name": "endpoint:hand-made" }, { "id": "s-email", "name": "email" }]),
    )
    .await;
    links(
        &keycloak,
        "optional-client-scopes",
        json!([{ "id": "s-foreign", "name": format!("endpoint:{AIR}") }]),
    )
    .await;

    let outcomes = sync(&keycloak).converge(&mirror()).await;
    let refused = outcomes
        .iter()
        .find(|o| o.app == format!("endpoint:{AIR}") && o.error.is_some())
        .unwrap_or_else(|| panic!("the foreign scope was not reported: {outcomes:?}"));
    assert!(refused
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("did not create"));

    let written = writes(&keycloak).await;
    assert_eq!(written, [format!("DELETE {REALM}/client-scopes/s-old")]);
    assert_eq!(
        linked(&outcomes, &air().scope()).map(|c| c.1),
        Some("RealmRefused")
    );
    for (scope, kind) in [
        ("endpoint:hand-made", "a default"),
        (format!("endpoint:{AIR}").as_str(), "an optional"),
    ] {
        let reported = outcomes
            .iter()
            .filter(|o| o.app == scope)
            .flat_map(|o| &o.warnings)
            .any(|w| w.contains(&format!("as {kind} scope")) && w.contains("next deployment sync"));
        assert!(
            reported,
            "{scope} as {kind} link was not reported: {outcomes:?}"
        );
    }
    assert!(
        !written.iter().any(|w| w.contains("s-email")),
        "a scope of another prefix was touched"
    );
}

/// A scope changed in the console is written back.
#[tokio::test]
async fn a_scope_changed_in_the_realm_is_written_back() {
    let keycloak = realm(Some(json!({ "id": "hub-1", "clientId": "mcp-hub" }))).await;
    let mut edited = held("s-1", desired(&air()));
    edited["attributes"]["include.in.token.scope"] = json!("false");
    scopes(&keycloak, json!([edited.clone()]), json!([edited.clone()])).await;
    links(&keycloak, "default-client-scopes", json!([])).await;
    links(&keycloak, "optional-client-scopes", json!([edited])).await;

    let outcomes = sync(&keycloak).converge(&mirror()).await;
    assert_eq!(
        writes(&keycloak).await,
        [format!("PUT {REALM}/client-scopes/s-1")]
    );
    assert!(outcomes.iter().any(|o| !o.drift.is_empty()), "{outcomes:?}");
}

/// Before the deployment renders the hub client, the run writes nothing and says why.
#[tokio::test]
async fn without_the_hub_client_nothing_is_written() {
    let keycloak = realm(None).await;
    let outcomes = sync(&keycloak).converge(&mirror()).await;
    assert!(writes(&keycloak).await.is_empty());
    assert!(
        outcomes
            .iter()
            .any(|o| o.warnings.iter().any(|w| w.contains("mcp-hub"))),
        "{outcomes:?}"
    );
    assert!(outcomes.iter().all(|o| o.error.is_none()));
    assert_eq!(
        linked(&outcomes, &air().scope()),
        None,
        "no hub, no condition"
    );
}

/// T-4445: a converged realm of 10,000 Endpoints and their 10,000 scopes is one linear wave
/// that writes nothing. Before, each Endpoint searched the whole listing, and each scope the
/// whole link list: 10^8 comparisons a wave.
#[tokio::test]
async fn ten_thousand_converged_scopes_take_one_quick_wave_and_no_write() {
    let mirror = Mirror::new();
    let mut listed = Vec::new();
    for i in 0..10_000 {
        // An opaque 26-letter slug, like the generated ones.
        let slug: String = format!("{i:05}")
            .bytes()
            .map(|digit| char::from(b'a' + digit - b'0'))
            .chain(std::iter::repeat_n('q', 21))
            .collect();
        let name = format!("e{i:05}");
        mirror.upsert(endpoint(
            "p",
            &name,
            json!({ "contextSpaceRef": "p", "slug": slug, "audience": "public",
                    "enabledRepresentations": ["ngsi-ld"] }),
        ));
        let one = HubEndpoint {
            project: "p".to_owned(),
            name,
            slug,
        };
        listed.push(held(&format!("s-{i}"), desired(&one)));
    }
    assert_eq!(serving(&mirror).len(), 10_000);
    let keycloak = realm(Some(json!({ "id": "hub-1", "clientId": "mcp-hub" }))).await;
    let listed = Value::Array(listed);
    scopes(&keycloak, listed.clone(), listed.clone()).await;
    links(&keycloak, "default-client-scopes", json!([])).await;
    links(&keycloak, "optional-client-scopes", listed).await;

    let started = std::time::Instant::now();
    let outcomes = sync(&keycloak).converge(&mirror).await;
    let took = started.elapsed();
    assert!(outcomes.is_empty(), "{:?}", outcomes.first());
    assert!(writes(&keycloak).await.is_empty());
    eprintln!("a converged wave over 10,000 scopes took {took:?}");
    assert!(took < std::time::Duration::from_secs(2), "{took:?}");
}
