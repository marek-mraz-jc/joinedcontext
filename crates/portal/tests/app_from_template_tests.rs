//! An App started from a gallery template as it ships, without the model (T-5617, AP-141): the
//! template's source and the sample's overlay land in a new repository `{project}_{app}` and the
//! App building from it is proposed as a Change, under the caller's `propose App`; the Endpoint
//! is one the caller reads, checked again on the server.

mod common;

use std::collections::BTreeMap;

use axum::http::StatusCode;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use common::{envelope, forge, mount_repository, person, send, state_on};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "/api/v1/repos/test-owner/sharing";
const LANDED: &str = "/api/v1/repos/test-owner/sharing_bike-finder";
const ROUTE: &str = "/api/v1/projects/sharing/app-templates/citizen-map/apps";
const FIRST: &str = "5d0f3c1f6a3e2b9d8c7a6b5e4d3c2b1a0f9e8d7c";

fn at(verb: &str, route: &str, status: u16, body: Value) -> Mock {
    Mock::given(method(verb))
        .and(path(route.to_owned()))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
}

/// The project `sharing` with two spaces, an Endpoint in each (`air` hides `name`, which the
/// citizen map reads). Ema proposes Apps and reads only the `mobility` space's Endpoints, the way
/// an app-editor does; Peter reads the project and proposes nothing. `sharing_bike-finder` is not
/// in the forge until a request creates it.
async fn world() -> (MockServer, AppState) {
    let server = forge().await;
    mount_repository(&server, PROJECT).await;
    at("GET", LANDED, 404, json!({ "message": "not found" }))
        .mount(&server)
        .await;
    at(
        "POST",
        "/api/v1/orgs/test-owner/repos",
        201,
        json!({ "name": "created" }),
    )
    .mount(&server)
    .await;
    at(
        "POST",
        &format!("{LANDED}/contents"),
        201,
        json!({ "commit": { "sha": FIRST } }),
    )
    .mount(&server)
    .await;
    for (verb, route) in [
        ("POST", format!("{LANDED}/branch_protections")),
        ("DELETE", LANDED.to_owned()),
    ] {
        at(verb, &route, 201, json!({})).mount(&server).await;
    }

    let mut state = state_on(&server);
    if let Some(gitea) = state.gitea.as_mut().and_then(std::sync::Arc::get_mut) {
        gitea.public_base = "https://forge.example/".parse().expect("a url");
    }
    state.mirror.set_layout(2);
    state.mirror.set_repositories(BTreeMap::from([(
        "sharing".to_owned(),
        "sharing".to_owned(),
    )]));
    state.mirror.upsert(envelope(
        "Project",
        "sharing",
        ORG_NAMESPACE,
        json!({ "organizationRef": "bb", "repository": { "name": "sharing" }, "ref": "main" }),
    ));
    for space in ["mobility", "air"] {
        state
            .mirror
            .upsert(envelope("ContextSpace", space, "sharing", json!({})));
    }
    state.mirror.upsert(envelope(
        "Endpoint",
        "bikes",
        "sharing",
        json!({ "slug": "bikes", "contextSpaceRef": "mobility" }),
    ));
    state.mirror.upsert(envelope(
        "Endpoint",
        "air",
        "sharing",
        json!({ "slug": "air", "contextSpaceRef": "air", "projection": { "hiddenAttributes": ["name"] } }),
    ));
    for (role, kinds, verbs) in [
        ("app-editor", json!(["App"]), json!(["read", "propose"])),
        ("endpoint-reader", json!(["Endpoint"]), json!(["read"])),
        (
            "reader",
            json!(["Project", "App", "Endpoint"]),
            json!(["read"]),
        ),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": kinds, "verbs": verbs }] }),
        ));
    }
    for (name, who, role, scope) in [
        (
            "ema-edits",
            "ema",
            "app-editor",
            json!({ "project": "sharing" }),
        ),
        (
            "ema-reads",
            "ema",
            "endpoint-reader",
            json!({ "contextSpace": "mobility" }),
        ),
        (
            "peter-reads",
            "peter",
            "reader",
            json!({ "project": "sharing" }),
        ),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            name,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": role, "scope": scope }),
        ));
    }
    (server, state)
}

async fn create(state: &AppState, who: &str, body: Value) -> common::Answer {
    send(state, person(who), "POST", ROUTE, Some(body)).await
}

async fn sent(server: &MockServer, verb: &str, route: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| r.method.as_str() == verb && r.url.path() == route)
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect()
}

async fn nothing_created(server: &MockServer) {
    assert!(sent(server, "POST", "/api/v1/orgs/test-owner/repos")
        .await
        .is_empty());
    assert!(sent(server, "POST", &format!("{LANDED}/contents"))
        .await
        .is_empty());
}

/// The files of a `/contents` commit, path to text.
fn committed(body: &Value) -> BTreeMap<String, String> {
    body["files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|file| {
            let text = STANDARD
                .decode(file["content"].as_str().unwrap_or_default())
                .unwrap_or_default();
            (
                file["path"].as_str().unwrap_or_default().to_owned(),
                String::from_utf8_lossy(&text).into_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_template_lands_in_its_own_repository_and_the_app_is_proposed_without_the_model() {
    let (server, state) = world().await;
    let answer = create(
        &state,
        "ema",
        json!({ "name": "bike-finder", "endpointName": "bikes" }),
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);

    let created = sent(&server, "POST", "/api/v1/orgs/test-owner/repos").await;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["name"], "sharing_bike-finder");
    assert_eq!(created[0]["private"], true);
    let commits = sent(&server, "POST", &format!("{LANDED}/contents")).await;
    assert_eq!(commits.len(), 1, "one first commit");
    assert_eq!(commits[0]["branch"], "main");
    let files = committed(&commits[0]);
    // The sample's own App over the template, with its tests, and the Portal's build.
    let sample = std::fs::read_to_string("sdk/samples/citizen-map/src/App.tsx").expect("sample");
    assert_eq!(files.get("src/App.tsx"), Some(&sample));
    assert!(files.contains_key("src/App.test.tsx"));
    assert!(files.contains_key("src/fixtures.ts"));
    assert!(files.contains_key("package.json"));
    assert!(files.contains_key("README.md"));
    assert_eq!(
        files.get(".gitea/workflows/build.yml").map(String::as_str),
        Some(joinedcontext_portal::agents::repository::WORKFLOW_TEXT)
    );
    // No credential of the platform rides in the source.
    assert!(files.values().all(|text| !text.contains("token-xyz")));
    assert_eq!(
        sent(&server, "POST", &format!("{LANDED}/branch_protections"))
            .await
            .len(),
        1
    );

    // The App, proposed in the project's repository, builds from the first commit and reads the
    // Endpoint's space.
    let proposed: Vec<String> = server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| {
            matches!(r.method.as_str(), "PUT" | "POST")
                && r.url.path().starts_with(&format!("{PROJECT}/contents"))
        })
        .flat_map(|r| {
            let body: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
            let mut texts: Vec<String> = committed(&body).into_values().collect();
            if let Some(one) = body["content"].as_str() {
                texts.push(
                    String::from_utf8_lossy(&STANDARD.decode(one).unwrap_or_default()).into_owned(),
                );
            }
            texts
        })
        .collect();
    let app = proposed
        .iter()
        .find(|text| text.contains("kind: App"))
        .unwrap_or_else(|| panic!("no App proposed: {proposed:?}"));
    assert!(
        app.contains("https://forge.example/test-owner/sharing_bike-finder.git"),
        "{app}"
    );
    assert!(app.contains(FIRST), "{app}");
    assert!(app.contains("BikeHireDockingStation"), "{app}");
    assert!(app.contains("mobility"), "{app}");
    assert_eq!(
        sent(&server, "DELETE", LANDED).await.len(),
        0,
        "the repository stays"
    );

    // Only the forge was asked: no model, no agent proxy.
    for request in server.received_requests().await.expect("requests") {
        let at = request.url.path();
        assert!(
            at.starts_with("/api/v1/repos/")
                || at.starts_with("/api/v1/orgs/")
                || at.starts_with("/api/v1/user"),
            "asked {at}"
        );
    }
}

#[tokio::test]
async fn a_person_without_propose_app_is_refused_and_nothing_is_created() {
    let (server, state) = world().await;
    let answer = create(
        &state,
        "peter",
        json!({ "name": "bike-finder", "endpointName": "bikes" }),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    nothing_created(&server).await;
}

#[tokio::test]
async fn an_endpoint_the_person_may_not_read_is_a_422_naming_the_field() {
    let (server, state) = world().await;
    for endpoint in ["air", "no-such"] {
        let answer = create(
            &state,
            "ema",
            json!({ "name": "bike-finder", "endpointName": endpoint }),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            answer.text
        );
        assert!(answer.text.contains("endpointName"), "{}", answer.text);
        assert!(answer.text.contains("you may read"), "{}", answer.text);
    }
    nothing_created(&server).await;
}

#[tokio::test]
async fn an_endpoint_hiding_what_the_template_reads_is_a_422_naming_the_field() {
    let (server, state) = world().await;
    // Ema reads the `air` space too here; its Endpoint hides `name`.
    state.mirror.upsert(envelope(
        "RoleBinding",
        "ema-reads-air",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "ema@hel.fi" }], "role": "endpoint-reader", "scope": { "contextSpace": "air" } }),
    ));
    let answer = create(
        &state,
        "ema",
        json!({ "name": "bike-finder", "endpointName": "air" }),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        answer.text
    );
    assert!(
        answer.names_field("endpointName", "attribute 'name' is hidden"),
        "{}",
        answer.text
    );
    nothing_created(&server).await;
}

#[tokio::test]
async fn a_bad_or_taken_name_is_a_422_naming_the_field_and_an_unknown_template_a_404() {
    let (server, state) = world().await;
    state
        .mirror
        .upsert(envelope("App", "taken", "sharing", json!({})));
    for name in ["Bike_Finder", "taken", &"a".repeat(99)] {
        let answer = create(
            &state,
            "ema",
            json!({ "name": name, "endpointName": "bikes" }),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{name}: {}",
            answer.text
        );
        assert!(answer.text.contains("name:"), "{}", answer.text);
    }
    let unknown = send(
        &state,
        person("ema"),
        "POST",
        "/api/v1/projects/sharing/app-templates/no-such/apps",
        Some(json!({ "name": "bike-finder", "endpointName": "bikes" })),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{}", unknown.text);
    let extra = create(
        &state,
        "ema",
        json!({ "name": "bike-finder", "endpointName": "bikes", "prompt": "x" }),
    )
    .await;
    assert_eq!(
        extra.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        extra.text
    );
    nothing_created(&server).await;
}

#[tokio::test]
async fn a_repository_already_in_the_forge_is_never_adopted() {
    let (server, state) = world().await;
    at("GET", LANDED, 200, json!({ "default_branch": "main" }))
        .with_priority(1)
        .mount(&server)
        .await;
    let answer = create(
        &state,
        "ema",
        json!({ "name": "bike-finder", "endpointName": "bikes" }),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    nothing_created(&server).await;
}
