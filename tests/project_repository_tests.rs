//! Writes of layout 2 land in the repository that holds them (T-2642, CC-87, PF-86): a
//! project's kinds, its Changes and its workspaces in the project repository, nothing of it in
//! the organization repository.

mod common;

use axum::http::StatusCode;
use common::{checked_send, forge, mount_repository, person, send, state_on, REPO};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use std::collections::BTreeMap;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT_REPO: &str = "/api/v1/repos/test-owner/ovzdusie";

fn steward() -> Identity {
    Identity {
        client: None,
        groups: vec!["portal-approver".into()],
        ..person("jana")
    }
}

/// The organization repository `test-repo` and the project repository `ovzdusie`, both taking
/// every write, with `ovzdusie` registered as living in its own repository.
async fn world() -> (MockServer, AppState) {
    let server = forge().await;
    mount_repository(&server, PROJECT_REPO).await;
    for repo in [REPO, PROJECT_REPO] {
        Mock::given(method("GET"))
            .and(path(format!("{repo}/branches/main")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": "base1" } })),
            )
            .mount(&server)
            .await;
    }
    let state = state_on(&server);
    state.mirror.set_repositories(BTreeMap::from([(
        "ovzdusie".to_owned(),
        "ovzdusie".to_owned(),
    )]));
    (server, state)
}

/// Every request of `verb` whose path starts with `prefix`.
async fn requests(
    server: &MockServer,
    verb: &str,
    prefix: &str,
) -> Vec<(String, serde_json::Value)> {
    server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| r.method.as_str() == verb && r.url.path().starts_with(prefix))
        .map(|r| {
            (
                r.url.path().to_owned(),
                serde_json::from_slice(&r.body).unwrap_or_default(),
            )
        })
        .collect()
}

/// Nothing was written to the organization repository.
async fn organization_untouched(server: &MockServer) {
    for verb in ["POST", "PUT", "DELETE", "PATCH"] {
        let writes = requests(server, verb, &format!("{REPO}/")).await;
        assert!(
            writes.is_empty(),
            "{verb} on the organization repository: {writes:?}"
        );
    }
}

/// CC-87: a project's kind is committed to the project repository at its own root, on a branch
/// of that repository, and its merge request is opened there.
#[tokio::test]
async fn a_project_kind_lands_in_the_project_repository() {
    let (server, state) = world().await;
    let answer = checked_send(
        &state,
        steward(),
        "POST",
        "/api/v1/projects/ovzdusie/spaces",
        Some(json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "ContextSpace",
            "metadata": { "name": "mobility", "namespace": "ovzdusie" },
            "spec": { "isSandbox": true }
        })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let change: serde_json::Value = serde_json::from_str(&answer.text).expect("a Change");
    assert_eq!(
        change["status"]["repository"], "ovzdusie",
        "the Change names its repository"
    );

    let branches = requests(&server, "POST", &format!("{PROJECT_REPO}/branches")).await;
    assert_eq!(branches.len(), 1, "one branch, in the project repository");
    let written: Vec<String> = requests(&server, "PUT", &format!("{PROJECT_REPO}/contents/"))
        .await
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    // A new space's Change carries its empty draft model beside it (T-2699, DM-61), in the same
    // repository and under the same root.
    assert_eq!(
        written,
        [
            format!("{PROJECT_REPO}/contents/spaces/mobility/datamodels/mobility.yaml"),
            format!("{PROJECT_REPO}/contents/spaces/mobility/datamodels/mobility.linkml.yaml"),
            format!("{PROJECT_REPO}/contents/spaces/mobility/space.yaml"),
        ],
        "the paths inside the project repository, without projects/ovzdusie/"
    );
    assert_eq!(
        requests(&server, "POST", &format!("{PROJECT_REPO}/pulls"))
            .await
            .len(),
        1
    );
    organization_untouched(&server).await;
}

/// CC-87: a project's Changes are the merge requests of its own repository.
#[tokio::test]
async fn a_projects_changes_are_read_from_its_repository() {
    let (server, state) = world().await;
    let answer = send(
        &state,
        steward(),
        "GET",
        "/api/v1/projects/ovzdusie/changes",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(!requests(&server, "GET", &format!("{PROJECT_REPO}/pulls"))
        .await
        .is_empty());
    assert!(requests(&server, "GET", &format!("{REPO}/pulls"))
        .await
        .is_empty());
}

/// CC-87: a project workspace is a branch of the project repository only.
#[tokio::test]
async fn a_workspace_is_a_branch_of_the_project_repository() {
    let (server, state) = world().await;
    let answer = send(
        &state,
        steward(),
        "POST",
        "/api/v1/projects/ovzdusie/workspaces",
        Some(json!({ "name": "air-v2" })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CREATED, "{}", answer.text);
    let branches = requests(&server, "POST", &format!("{PROJECT_REPO}/branches")).await;
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0].1["new_branch_name"], "workspace/air-v2");
    organization_untouched(&server).await;
}
