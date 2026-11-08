//! A workspace brought back is a Change like any other (CC-79, UI-63): listed for the approver,
//! readable, and it says which workspace it came from.

mod common;

use axum::http::StatusCode;
use common::{encode, forge, person, send, state_on, REPO};
use joinedcontext_portal::auth::session::Identity;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, ResponseTemplate};

const SPACE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: air\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
const FILE: &str = "projects/ovzdusie/spaces/air/space.yaml";

fn approver() -> Identity {
    Identity {
        client: None,
        groups: vec!["portal-approver".into()],
        ..person("petra")
    }
}

#[tokio::test]
async fn a_workspace_brought_back_is_listed_and_read_with_its_name() {
    let server = forge().await;
    let pull = json!({
        "Number": 5,
        "HtmlUrl": "https://shako.example/pulls/5",
        "State": "open",
        "Title": "workspace air-v2: Air",
        "Head": "workspace/air-v2",
        "Base": "main",
        "CreatedAt": "2026-09-18T10:00:00Z",
        "Author": "jana",
        "AuthorName": "Jana",
        "AuthorEmail": "jana@hel.fi",
        "Mergeable": true,
        "Merged": false
    });
    let other = json!({
        "Number": 6,
        "HtmlUrl": "https://shako.example/pulls/6",
        "State": "open",
        "Title": "someone's branch",
        "Head": "feature/x",
        "Base": "main",
        "CreatedAt": "2026-09-18T11:00:00Z",
        "Author": "x",
        "Mergeable": true,
        "Merged": false
    });
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "value": [pull.clone(), other] })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests(5)")))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests(5)/Files")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "value": [{ "Filename": FILE, "Status": "added" }] })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param("@p", format!("'{FILE}'")))
        .and(query_param("@r", "'workspace/air-v2'"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "Sha": "b", "Content": encode(SPACE) })),
        )
        .mount(&server)
        .await;
    let state = state_on(&server);

    let listed = send(
        &state,
        approver(),
        "GET",
        "/api/v1/projects/ovzdusie/changes",
        None,
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.text);
    let list: Value = serde_json::from_str(&listed.text).unwrap();
    let items = list["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "a feature branch is not a Change: {items:?}"
    );
    assert_eq!(items[0]["metadata"]["name"], "chg-00000005");
    assert_eq!(items[0]["workspace"], "air-v2");

    let one = send(
        &state,
        approver(),
        "GET",
        "/api/v1/projects/ovzdusie/changes/chg-00000005",
        None,
    )
    .await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.text);
    let change: Value = serde_json::from_str(&one.text).unwrap();
    assert_eq!(change["workspace"], "air-v2");
    assert_eq!(change["summary"]["params"]["name"], "air");
}
