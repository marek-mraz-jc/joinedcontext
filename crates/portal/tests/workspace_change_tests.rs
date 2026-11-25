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

/// Mounts merge request `number` on `branch` with `files` (path, forge status) and the file
/// bodies `bodies` (path, ref, text); a file with no body answers the forge's 404.
async fn workspace_pull(
    server: &wiremock::MockServer,
    number: u64,
    branch: &str,
    files: &[(&str, &str)],
    bodies: &[(&str, &str, &str)],
) {
    let pull = json!({
        "Number": number,
        "HtmlUrl": format!("https://shako.example/pulls/{number}"),
        "State": "open",
        "Title": format!("workspace {branch}"),
        "Head": branch,
        "Base": "main",
        "CreatedAt": "2026-09-18T10:00:00Z",
        "Author": "jana",
        "AuthorName": "Jana",
        "AuthorEmail": "jana@hel.fi",
        "Mergeable": true,
        "Merged": false
    });
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests({number})")))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull))
        .mount(server)
        .await;
    let listed: Vec<Value> = files
        .iter()
        .map(|(file, status)| json!({ "Filename": file, "Status": status }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests({number})/Files")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": listed })))
        .mount(server)
        .await;
    for (file, git_ref, body) in bodies {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
            .and(query_param("@p", format!("'{file}'")))
            .and(query_param("@r", format!("'{git_ref}'")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "Sha": "b", "Content": encode(body) })),
            )
            .mount(server)
            .await;
    }
}

async fn read_change(state: &joinedcontext_portal::state::AppState, number: u64) -> Value {
    let one = send(
        state,
        approver(),
        "GET",
        &format!("/api/v1/projects/ovzdusie/changes/chg-{number:08x}"),
        None,
    )
    .await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.text);
    serde_json::from_str(&one.text).unwrap()
}

#[tokio::test]
async fn a_workspace_is_headed_by_its_first_readable_manifest_and_a_seed_only_one_by_its_seed() {
    let server = forge().await;
    let edited = SPACE.replace("isSandbox: true", "isSandbox: false");
    // A manifest the forge no longer holds, then one that is no manifest: both are passed over,
    // and the edited space heads the Change as an update read against main.
    workspace_pull(
        &server,
        7,
        "workspace/air-v3",
        &[
            ("projects/ovzdusie/project.yaml", "modified"),
            ("projects/ovzdusie/broken/project.yaml", "added"),
            (FILE, "modified"),
        ],
        &[
            (
                "projects/ovzdusie/broken/project.yaml",
                "workspace/air-v3",
                "kind: [unclosed",
            ),
            (FILE, "workspace/air-v3", &edited),
            (FILE, "main", SPACE),
        ],
    )
    .await;
    // Seed files alone: a deleted one is never the headline, one the forge lost and one that
    // is no JSON are passed over, and the next is an update, since main holds it too.
    let seed = |name: &str| format!("projects/ovzdusie/spaces/air/entities/seed/{name}.json");
    let (gone, lost, text, kept) = (seed("gone"), seed("lost"), seed("text"), seed("kept"));
    workspace_pull(
        &server,
        8,
        "workspace/air-v4",
        &[
            (&gone, "deleted"),
            (&lost, "modified"),
            (&text, "modified"),
            (&kept, "modified"),
        ],
        &[
            (&text, "workspace/air-v4", "no json"),
            (
                &kept,
                "workspace/air-v4",
                r#"{"id":"urn:ngsi-ld:T:1","type":"T","v":2}"#,
            ),
            (
                &kept,
                "main",
                r#"{"id":"urn:ngsi-ld:T:1","type":"T","v":1}"#,
            ),
        ],
    )
    .await;
    // A new seed file: the same headline, now a create.
    let fresh = seed("fresh");
    workspace_pull(
        &server,
        9,
        "workspace/air-v5",
        &[(&fresh, "added")],
        &[(
            &fresh,
            "workspace/air-v5",
            r#"{"id":"urn:ngsi-ld:T:2","type":"T"}"#,
        )],
    )
    .await;
    let state = state_on(&server);

    let space = read_change(&state, 7).await;
    assert_eq!(space["summary"]["key"], "change.summary.update", "{space}");
    assert_eq!(space["summary"]["params"]["kind"], "ContextSpace");
    assert_eq!(space["summary"]["params"]["name"], "air");

    let kept = read_change(&state, 8).await;
    assert_eq!(kept["summary"]["key"], "change.summary.update", "{kept}");
    assert_eq!(kept["summary"]["params"]["kind"], "Entity");
    assert_eq!(kept["summary"]["params"]["name"], "kept");

    let fresh = read_change(&state, 9).await;
    assert_eq!(fresh["summary"]["key"], "change.summary.create", "{fresh}");
    assert_eq!(fresh["summary"]["params"]["name"], "fresh");
}
