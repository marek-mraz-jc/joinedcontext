//! `jc_change_get` through every door of the registry (T-1527; AG-64, PF-50, PF-59, CC-79).
//!
//! What was there before this file: the route `GET /api/v1/projects/{project}/changes/{id}` is
//! tested in `changes_tests.rs`, `edge_changes_tests.rs` (`a_change_is_not_readable_through_another_projects_path`)
//! and `workspace_change_tests.rs` (a workspace's change carries its name). The operation wraps
//! the same handler (src/ops/views.rs), and no test called it by name as a viewer, a stranger, an
//! agent run or an MCP client.

mod common;

use axum::http::StatusCode;
use common::doors::{self, mcp, run, session, steward, stranger, viewer};
use common::{encode, forge, state_on, REPO};
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SPACE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: air\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
const SPACE_FILE: &str = "projects/ovzdusie/spaces/air/space.yaml";
const OTHER: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Pipeline\nmetadata:\n  name: buses\n  namespace: doprava\nspec:\n  contextSpaceRef: traffic\n";
const OTHER_FILE: &str = "projects/doprava/pipelines/buses.yaml";

fn pull(number: u64, branch: &str) -> Value {
    json!({
        "number": number, "html_url": format!("https://gitea.example/pulls/{number}"),
        "state": "open", "title": "a change", "head": { "ref": branch }, "base": { "ref": "main" },
        "created_at": "2026-09-18T10:00:00Z",
        "user": { "login": "jana", "full_name": "Jana", "email": "jana@banskabystrica.sk" },
        "mergeable": true, "merged": false
    })
}

async fn mount_change(server: &MockServer, number: u64, branch: &str, file: &str, content: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls/{number}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull(number, branch)))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls/{number}/files")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{ "filename": file, "status": "added" }])),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/contents/{file}")))
        .and(query_param("ref", branch))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "sha": "b", "content": encode(content) })),
        )
        .mount(server)
        .await;
}

/// Change 5 brings workspace `air-v2` of `ovzdusie` back; change 7 is a Pipeline of `doprava`.
async fn world() -> (MockServer, AppState) {
    let server = forge().await;
    mount_change(&server, 5, "workspace/air-v2", SPACE_FILE, SPACE).await;
    mount_change(
        &server,
        7,
        "portal/create-pipeline-buses-1527aaaa",
        OTHER_FILE,
        OTHER,
    )
    .await;
    let state = doors::state_with(state_on(&server));
    (server, state)
}

fn id(number: u64) -> Value {
    json!({ "id": format!("chg-{number:08x}") })
}

/// CC-79, PF-50: the project's steward and its viewer read a change through the session, MCP and a
/// run whose profile names the operation, and a change brought back from a workspace says which.
#[tokio::test]
async fn every_door_reads_a_change_and_a_workspace_change_names_its_workspace() {
    let (_server, state) = world().await;
    for (who, caller) in [
        ("steward", session(steward())),
        ("steward over mcp", mcp(steward())),
        ("viewer", session(viewer())),
        ("steward's run", run(steward(), &["jc_change_get"])),
    ] {
        let read = doors::call("jc_change_get", &caller, &state, id(5)).await;
        assert_eq!(StatusCode::OK, read.status, "{who}: {}", read.text());
        assert_eq!("chg-00000005", read.body["metadata"]["name"], "{who}");
        assert_eq!("air-v2", read.body["workspace"], "{who}: {}", read.text());
    }
}

/// PF-59, R20: a change whose manifest lives in another project is not there when it is read
/// through this project's path: the answer is `404`, never a `403` that would say it exists.
#[tokio::test]
async fn a_change_of_another_project_is_not_there_rather_than_forbidden() {
    let (_server, state) = world().await;
    for caller in [
        session(steward()),
        mcp(steward()),
        run(steward(), &["jc_change_get"]),
    ] {
        let hidden = doors::call("jc_change_get", &caller, &state, id(7)).await;
        assert_eq!(StatusCode::NOT_FOUND, hidden.status, "{}", hidden.text());
        assert!(
            !hidden.text().contains("buses"),
            "the other project's change leaked: {}",
            hidden.text()
        );
    }
}

/// PF-59: a person with no binding in the project is answered as if the project were not there,
/// and reads nothing of the change.
#[tokio::test]
async fn a_stranger_reads_no_change_at_any_door() {
    let (_server, state) = world().await;
    for caller in [
        session(stranger()),
        mcp(stranger()),
        run(stranger(), &["jc_change_get"]),
    ] {
        let refused = doors::call("jc_change_get", &caller, &state, id(5)).await;
        assert_eq!(StatusCode::NOT_FOUND, refused.status, "{}", refused.text());
        assert!(!refused.text().contains("air-v2"), "{}", refused.text());
    }
}

/// AG-70: a run whose profile does not name the read is refused it and is not offered it.
#[tokio::test]
async fn a_run_whose_profile_does_not_name_the_read_is_refused_it() {
    let (_server, state) = world().await;
    let bare = run(steward(), &["jc_resource_list"]);
    let refused = doors::call("jc_change_get", &bare, &state, id(5)).await;
    assert_eq!(StatusCode::FORBIDDEN, refused.status, "{}", refused.text());
    assert!(!doors::offered(&bare, &state).contains(&"jc_change_get".to_owned()));
}

/// Input is validated before anything is read: an unknown field is a `422` naming it.
#[tokio::test]
async fn an_unknown_field_is_refused_before_the_forge_is_asked() {
    let (server, state) = world().await;
    let refused = doors::call(
        "jc_change_get",
        &session(steward()),
        &state,
        json!({ "id": "chg-00000005", "approve": true }),
    )
    .await;
    assert_eq!(
        StatusCode::UNPROCESSABLE_ENTITY,
        refused.status,
        "{}",
        refused.text()
    );
    let asked = server.received_requests().await.unwrap_or_default();
    assert!(
        !asked
            .iter()
            .any(|request| request.url.path().contains("/pulls/5")),
        "the forge was asked for a change the input never validly named",
    );
}

/// AG-64: the REST route and `POST …/ops/jc_change_get` answer each person alike.
#[tokio::test]
async fn the_rest_route_and_the_ops_door_answer_alike() {
    let (_server, state) = world().await;
    for (identity, number) in [
        (steward(), 5),
        (viewer(), 5),
        (stranger(), 5),
        (steward(), 7),
    ] {
        let rest = doors::http(
            &state,
            identity.clone(),
            "GET",
            &format!("/api/v1/projects/ovzdusie/changes/chg-{number:08x}"),
            None,
        )
        .await;
        let door = doors::post_op(&state, identity.clone(), "jc_change_get", id(number)).await;
        assert_eq!(
            rest.status,
            door.status,
            "{} on {number}: {} vs {}",
            identity.username,
            rest.text(),
            door.text()
        );
    }
}
