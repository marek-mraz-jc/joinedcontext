//! A proposal into a workspace commits to its branch and opens nothing; the checks are the
//! ones every proposal passes (CC-76, PF-82; T-1233).

use std::sync::Arc;

use joinedcontext_portal::api::mutate::{
    propose_into_workspace, propose_with_identity, ProposeOutcome,
};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::change::Operation;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::ops::workspaces::{Opening, Scope};
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPO: &str = "/api/v1/repos/test-owner/test-repo";

fn steward() -> Identity {
    Identity {
        client: None,
        subject: "f:1:demo.steward".into(),
        username: "demo.steward".into(),
        email: Some("demo.steward@banskabystrica.sk".into()),
        name: Some("Demo Steward".into()),
        roles: Vec::new(),
        groups: vec!["portal-approver".into()],
        second_factor: false,
    }
}

async fn forge() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/branches")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{REPO}/contents/.*")))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(format!("^{REPO}/contents/.*")))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    server
}

async fn state_on(server: &MockServer) -> AppState {
    let client = GiteaClient::new(
        server.uri().parse().unwrap(),
        "test-owner",
        "test-repo",
        "t",
    )
    .unwrap();
    AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client))
}

fn space(name: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": name, "namespace": "ovzdusie" },
        "spec": { "isSandbox": true },
    })
}

async fn open(state: &AppState, name: &str, scope: Scope) {
    state
        .workspaces
        .create(Opening {
            name,
            title: None,
            project: "ovzdusie",
            owner: "demo.steward@banskabystrica.sk",
            base_revision: "abc1234",
            scope,
            ttl_hours: 4,
        })
        .await
        .expect("opened");
}

fn whole_project() -> Scope {
    Scope::Project {}
}

async fn calls(server: &MockServer) -> Vec<(String, String, Value)> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .map(|r| {
            let body = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
            (r.method.to_string(), r.url.path().to_owned(), body)
        })
        .collect()
}

#[tokio::test]
async fn propose_with_workspace_commits_to_workspace_branch() {
    let server = forge().await;
    let state = state_on(&server).await;
    open(&state, "air-v2", whole_project()).await;

    let outcome = propose_into_workspace(
        &steward(),
        &state,
        "ovzdusie",
        "spaces",
        None,
        Operation::Create,
        space("mobility"),
        "air-v2",
    )
    .await
    .expect("committed");
    let ProposeOutcome::Workspace(commit) = outcome else {
        panic!("not a workspace commit")
    };
    assert_eq!(commit.branch, "workspace/air-v2");
    assert_eq!(commit.path, "projects/ovzdusie/spaces/mobility/space.yaml");

    let calls = calls(&server).await;
    let branch = calls
        .iter()
        .find(|(m, p, _)| m == "POST" && p.ends_with("/branches"))
        .expect("branch");
    assert_eq!(branch.2["new_branch_name"], "workspace/air-v2");
    let put = calls.iter().find(|(m, _, _)| m == "PUT").expect("a commit");
    assert_eq!(put.2["branch"], "workspace/air-v2");
}

#[tokio::test]
async fn propose_with_workspace_does_not_open_pr() {
    let server = forge().await;
    let state = state_on(&server).await;
    open(&state, "air-v2", whole_project()).await;
    propose_into_workspace(
        &steward(),
        &state,
        "ovzdusie",
        "spaces",
        None,
        Operation::Create,
        space("mobility"),
        "air-v2",
    )
    .await
    .expect("committed");
    assert!(
        !calls(&server)
            .await
            .iter()
            .any(|(m, p, _)| m == "POST" && p.ends_with("/pulls")),
        "no pull request: the workspace comes back as one Change (CC-79)"
    );
}

#[tokio::test]
async fn an_unknown_workspace_or_one_of_another_project_writes_nothing() {
    let server = forge().await;
    let state = state_on(&server).await;
    let err = propose_into_workspace(
        &steward(),
        &state,
        "ovzdusie",
        "spaces",
        None,
        Operation::Create,
        space("mobility"),
        "nope",
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("nope"), "{err}");

    state
        .workspaces
        .create(Opening {
            name: "elsewhere",
            title: None,
            project: "doprava",
            owner: "x@y.z",
            base_revision: "a",
            scope: Scope::Project {},
            ttl_hours: 1,
        })
        .await
        .unwrap();
    let err = propose_into_workspace(
        &steward(),
        &state,
        "ovzdusie",
        "spaces",
        None,
        Operation::Create,
        space("mobility"),
        "elsewhere",
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("does not cover project 'ovzdusie'"),
        "{err}"
    );
    assert!(
        !calls(&server).await.iter().any(|(m, _, _)| m == "PUT"),
        "nothing was written"
    );
}

#[tokio::test]
async fn a_resource_outside_the_workspace_scope_is_refused() {
    let server = forge().await;
    let state = state_on(&server).await;
    open(
        &state,
        "one-space",
        Scope::Space {
            name: "mobility".into(),
        },
    )
    .await;
    propose_into_workspace(
        &steward(),
        &state,
        "ovzdusie",
        "spaces",
        None,
        Operation::Create,
        space("mobility"),
        "one-space",
    )
    .await
    .expect("the space the workspace covers");
    let err = propose_into_workspace(
        &steward(),
        &state,
        "ovzdusie",
        "spaces",
        None,
        Operation::Create,
        space("parking"),
        "one-space",
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("does not cover ContextSpace 'parking'"),
        "{err}"
    );
}

#[tokio::test]
async fn a_viewer_is_refused_in_a_workspace_as_everywhere() {
    let server = forge().await;
    let state = state_on(&server).await;
    open(&state, "air-v2", whole_project()).await;
    let viewer = Identity {
        client: None,
        groups: Vec::new(),
        ..steward()
    };
    let into_ws = propose_into_workspace(
        &viewer,
        &state,
        "ovzdusie",
        "spaces",
        None,
        Operation::Create,
        space("mobility"),
        "air-v2",
    )
    .await
    .unwrap_err();
    let plain = propose_with_identity(
        &viewer,
        &state,
        "ovzdusie",
        "spaces",
        None,
        Operation::Create,
        false,
        space("mobility"),
    )
    .await
    .unwrap_err();
    assert_eq!(
        into_ws.to_string(),
        plain.to_string(),
        "a workspace grants nothing (PF-82)"
    );
    assert!(!calls(&server).await.iter().any(|(m, _, _)| m == "PUT"));
}

// --- PF-82: a workspace grants nothing (T-1256) --------------------------------------------

fn pipeline(name: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Pipeline",
        "metadata": { "name": name, "namespace": "ovzdusie" },
        "spec": {
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt-mesto" } },
            "compute": { "kind": "bloblang", "bloblang": "root = this" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air"
        }
    })
}

#[tokio::test]
async fn workspace_write_refused_when_quota_exceeded() {
    let server = forge().await;
    let state = state_on(&server).await;
    open(&state, "air-v2", whole_project()).await;
    state.mirror.upsert(
        serde_json::from_value(json!({
            "apiVersion": API_VERSION, "kind": "Organization",
            "metadata": { "name": "bb", "namespace": "org" },
            "spec": { "domain": "banskabystrica.sk", "projects": { "quota": { "residentPipelines": 1 } } }
        }))
        .unwrap(),
    );
    // An MQTT feed waits for pushes, so its pipeline stays resident (PL-78).
    state.mirror.upsert(
        serde_json::from_value(json!({
            "apiVersion": API_VERSION, "kind": "DataSource",
            "metadata": { "name": "mqtt-mesto", "namespace": "ovzdusie" },
            "spec": { "type": "mqtt", "mqtt": { "urls": ["tcp://mqtt.banskabystrica.sk:1883"], "topics": ["air/#"] } }
        }))
        .unwrap(),
    );
    // The Endpoint both pipelines write through, so only the quota refuses the second (PL-111).
    state.mirror.upsert(
        serde_json::from_value(json!({
            "apiVersion": API_VERSION, "kind": "Endpoint",
            "metadata": { "name": "public-air", "namespace": "ovzdusie" },
            "spec": { "contextSpaceRef": "air", "slug": "k7m2qz4tv6xh3n5jb2ryd3wcfa", "audience": "public" }
        }))
        .unwrap(),
    );
    state
        .mirror
        .upsert(serde_json::from_value(pipeline("first")).unwrap());
    let err = propose_into_workspace(
        &steward(),
        &state,
        "ovzdusie",
        "pipelines",
        None,
        Operation::Create,
        pipeline("second"),
        "air-v2",
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("residentPipelines"), "{err}");
    assert!(
        !calls(&server).await.iter().any(|(m, _, _)| m == "PUT"),
        "nothing was written"
    );
}

#[tokio::test]
async fn workspace_write_refused_when_secret_pasted() {
    let server = forge().await;
    let state = state_on(&server).await;
    open(&state, "air-v2", whole_project()).await;
    let source = json!({
        "apiVersion": API_VERSION,
        "kind": "DataSource",
        "metadata": { "name": "feed", "namespace": "ovzdusie" },
        "spec": { "type": "http", "connection": { "url": "https://example.invalid/feed", "password": "hunter2" } }
    });
    let err = propose_into_workspace(
        &steward(),
        &state,
        "ovzdusie",
        "datasources",
        None,
        Operation::Create,
        source,
        "air-v2",
    )
    .await
    .unwrap_err();
    let said = err.to_string();
    assert!(
        said.contains("secretRef") || said.contains("secret"),
        "{said}"
    );
    assert!(
        !said.contains("hunter2"),
        "the value is never echoed: {said}"
    );
    assert!(!calls(&server).await.iter().any(|(m, _, _)| m == "PUT"));
}

#[tokio::test]
async fn workspace_write_refused_above_own_rights() {
    let server = forge().await;
    let state = state_on(&server).await;
    open(&state, "access", whole_project()).await;
    // A person who may propose bindings but holds no `org-admin` themselves cannot grant it
    // through a workspace any more than outside one (PF-52).
    let binding = json!({
        "apiVersion": API_VERSION,
        "kind": "RoleBinding",
        "metadata": { "name": "grab", "namespace": "ovzdusie" },
        "spec": { "subjects": [{ "user": "friend@hel.fi" }], "role": "org-admin", "scope": { "project": "ovzdusie" } }
    });
    let into_ws = propose_into_workspace(
        &steward(),
        &state,
        "ovzdusie",
        "rolebindings",
        None,
        Operation::Create,
        binding.clone(),
        "access",
    )
    .await
    .unwrap_err();
    let plain = propose_with_identity(
        &steward(),
        &state,
        "ovzdusie",
        "rolebindings",
        None,
        Operation::Create,
        false,
        binding,
    )
    .await
    .unwrap_err();
    assert_eq!(into_ws.to_string(), plain.to_string());
    assert!(!calls(&server).await.iter().any(|(m, _, _)| m == "PUT"));
}
