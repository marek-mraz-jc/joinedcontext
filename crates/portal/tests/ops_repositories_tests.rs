//! The repository reads through every door of the registry (`src/ops/repositories.rs`): who reads
//! what, and what is never there.
//!
//! The forge is a stub answering shako's OData shapes; `crates/shako/server/tests/
//! portal_repositories.rs` runs the same operations against real shako with git on disk.

mod common;

use std::collections::BTreeMap;

use axum::http::StatusCode;
use common::doors::{self, mcp, run, session, steward, stranger, viewer, PROJECT};
use common::state_on;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CONFIG_ID: &str = "00000000-0000-4000-8000-000000000001";
const APP_ID: &str = "00000000-0000-4000-8000-000000000002";
const OTHER_ID: &str = "00000000-0000-4000-8000-000000000003";
const ROOT: &str = "1111111111111111111111111111111111111111";
const SRC: &str = "2222222222222222222222222222222222222222";
const HEAD: &str = "3333333333333333333333333333333333333333";
const PARENT: &str = "4444444444444444444444444444444444444444";
const XSS: &str =
    "<script>alert(1)</script><img src=x onerror=alert(2)> [go](javascript:alert(3))\n";

const OPERATIONS: &[&str] = &[
    "jc_repository_list",
    "jc_repository_get",
    "jc_repository_tree",
    "jc_repository_file",
    "jc_repository_commits",
    "jc_repository_commit",
    "jc_repository_refs",
    "jc_repository_pulls",
    "jc_repository_pull",
    "jc_repository_settings",
];

fn repo(id: &str, name: &str, project: &str) -> Value {
    json!({
        "Id": id, "Owner": "test-owner", "Name": name, "Project": project,
        "Description": format!("{name} repository"), "DefaultBranch": "main", "Private": true,
        "Archived": false, "SizeBytes": 2048, "CloneUrl": format!("https://git.example/{name}.git"),
        "UpdatedAt": "2026-11-08T10:00:00Z",
    })
}

async fn get(server: &MockServer, at: &str, answer: Value) {
    Mock::given(method("GET"))
        .and(path(at.to_owned()))
        .respond_with(ResponseTemplate::new(200).set_body_json(answer))
        .mount(server)
        .await;
}

async fn get_with(server: &MockServer, at: &str, name: &str, value: &str, answer: Value) {
    Mock::given(method("GET"))
        .and(path(at.to_owned()))
        .and(query_param(name, value))
        .respond_with(ResponseTemplate::new(200).set_body_json(answer))
        .mount(server)
        .await;
}

fn blob(content: &[u8]) -> Value {
    use base64::Engine;
    json!({
        "sha": "b", "content": String::from_utf8_lossy(content),
        "contentBase64": base64::engine::general_purpose::STANDARD.encode(content),
    })
}

/// A forge holding the project's configuration repository, its application's repository, and
/// another project's application.
async fn world() -> (MockServer, AppState) {
    let server = MockServer::start().await;
    get(
        &server,
        "/odata/Repositories",
        json!({ "value": [
            repo(CONFIG_ID, "ovzdusie-config", ""),
            repo(APP_ID, "ovzdusie_dash", ""),
            repo(OTHER_ID, "doprava_maps", ""),
            // Another organization's repository that names this project: never this project's.
            json!({ "Id": "5d1e2f30-0000-4000-8000-00000000b120", "Owner": "brno", "Name": "ovzdusie-brno",
                    "Project": "ovzdusie", "DefaultBranch": "main", "Private": false }),
        ] }),
    )
    .await;
    let tree = format!("/odata/Repositories('{CONFIG_ID}')/Tree");
    get_with(
        &server,
        &tree,
        "Ref",
        "main",
        json!({ "sha": ROOT, "tree": [
        { "path": "README.md", "mode": "100644", "type": "blob", "size": XSS.len(), "sha": "r" },
        { "path": "logo.png", "mode": "100644", "type": "blob", "size": 6, "sha": "p" },
        { "path": "big.csv", "mode": "100644", "type": "blob", "size": 5_000_000, "sha": "c" },
        { "path": "src", "mode": "040000", "type": "tree", "size": 0, "sha": SRC },
    ] }),
    )
    .await;
    get_with(
        &server,
        &tree,
        "Ref",
        SRC,
        json!({ "sha": SRC, "tree": [
        { "path": "page.html", "mode": "100644", "type": "blob", "size": XSS.len(), "sha": "h" },
    ] }),
    )
    .await;
    let file = format!("/odata/Repositories('{CONFIG_ID}')/File");
    get_with(&server, &file, "Path", "README.md", blob(XSS.as_bytes())).await;
    get_with(
        &server,
        &file,
        "Path",
        "src/page.html",
        blob(XSS.as_bytes()),
    )
    .await;
    get_with(&server, &file, "Path", "logo.png", blob(b"PNG\0\x01\x02")).await;
    let history = format!("/odata/Repositories('{CONFIG_ID}')/History");
    get(
        &server,
        &history,
        json!({ "value": [{
        "sha": HEAD, "message": "Change b\n\nWhy it changed", "author_name": "Alice",
        "author_email": "alice@city.example", "author_date": "2026-11-08T10:00:00Z",
        "parent_shas": [PARENT],
    }] }),
    )
    .await;
    get(
        &server,
        &format!("/odata/Repositories('{CONFIG_ID}')/Compare"),
        json!({ "files": [{ "filename": "src/page.html", "status": "modified" }] }),
    )
    .await;
    get(
        &server,
        "/odata/Commits",
        json!({ "value": [{
        "Sha": HEAD, "Message": "Change b", "Author": "Alice", "Date": "2026-11-08T10:00:00Z",
    }], "@odata.count": 1 }),
    )
    .await;
    get(
        &server,
        "/odata/Branches",
        json!({ "value": [
        { "Name": "portal/x", "HeadSha": PARENT, "Protected": false },
        { "Name": "main", "HeadSha": HEAD, "Protected": true },
    ] }),
    )
    .await;
    get(
        &server,
        "/odata/Tags",
        json!({ "value": [{ "Name": "v1", "TargetSha": HEAD }] }),
    )
    .await;
    get(
        &server,
        "/odata/PullRequests",
        json!({ "value": [{
        "Number": 7, "Title": "Add c", "State": "open", "Merged": false, "Author": "jc-portal",
        "Head": "portal/x", "Base": "main", "CreatedAt": "2026-11-08T10:00:00Z", "ClosedAt": null,
    }], "@odata.count": 1 }),
    )
    .await;
    get(&server, &format!("/odata/PullRequests('{CONFIG_ID}:7')"), json!({
        "Number": 7, "Title": "Add c", "State": "open", "Merged": false, "Author": "jc-portal",
        "Head": "portal/x", "Base": "main", "Body": XSS,
        "Files": [{ "filename": "c.yaml", "status": "added" }],
        "Reviews": [{ "Author": "jana", "Verdict": "APPROVED", "Body": "ok", "CommitId": HEAD }],
    }))
    .await;
    get(
        &server,
        "/odata/ProtectionRules",
        json!({ "value": [{
        "Pattern": "main", "EnablePush": false, "EnableForcePush": false, "RequiredApprovals": 1,
    }] }),
    )
    .await;
    get(&server, "/odata/Webhooks", json!({ "value": [{
        "Id": "w", "Url": "https://portal:hook-secret@portal.example/api/v1/webhooks/shako?key=k", "Active": true,
        "Secret": "never-shown-secret",
    }] }))
    .await;
    get(
        &server,
        "/odata/Teams",
        json!({ "value": [{ "Group": "city-stewards", "Verb": "write" }] }),
    )
    .await;
    get(&server, "/odata/PackageVersions", json!({ "value": [] })).await;

    let state = doors::state_with(state_on(&server));
    state.mirror.set_layout(2);
    state.mirror.set_repositories(BTreeMap::from([
        (PROJECT.to_owned(), "ovzdusie-config".to_owned()),
        ("doprava".to_owned(), "doprava-config".to_owned()),
    ]));
    (server, state)
}

/// A valid input of each operation, naming the project's configuration repository.
fn input(name: &str) -> Value {
    let r = "ovzdusie-config";
    match name {
        "jc_repository_list" => json!({}),
        "jc_repository_tree" => json!({ "repository": r, "path": "src" }),
        "jc_repository_file" => json!({ "repository": r, "path": "README.md" }),
        "jc_repository_commit" => json!({ "repository": r, "sha": HEAD }),
        "jc_repository_pull" => json!({ "repository": r, "number": 7 }),
        _ => json!({ "repository": r }),
    }
}

#[tokio::test]
async fn a_steward_reads_everything_a_viewer_all_but_the_settings_a_stranger_nothing() {
    let (_server, state) = world().await;
    for name in OPERATIONS {
        let as_steward = doors::call(name, &session(steward()), &state, input(name)).await;
        assert_eq!(
            as_steward.status,
            StatusCode::OK,
            "{name}: {}",
            as_steward.text()
        );
        let as_viewer = doors::call(name, &session(viewer()), &state, input(name)).await;
        let expected = if *name == "jc_repository_settings" {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::OK
        };
        assert_eq!(as_viewer.status, expected, "{name}: {}", as_viewer.text());
        // A person with no binding here reads a project that is not there (PF-59, R20).
        let as_stranger = doors::call(name, &session(stranger()), &state, input(name)).await;
        assert_eq!(
            as_stranger.status,
            StatusCode::NOT_FOUND,
            "{name}: {}",
            as_stranger.text()
        );
        // The same through MCP.
        let over_mcp = doors::call(name, &mcp(stranger()), &state, input(name)).await;
        assert_eq!(over_mcp.status, StatusCode::NOT_FOUND, "{name}");
    }
    // The REST door answers what the registry answers.
    let rest = doors::post_op(&state, viewer(), "jc_repository_list", json!({})).await;
    assert_eq!(rest.status, StatusCode::OK, "{}", rest.text());
    // An agent run reaches only the operations its profile names (AG-70).
    let refused = doors::call(
        "jc_repository_file",
        &run(steward(), &["jc_repository_list"]),
        &state,
        input("jc_repository_file"),
    )
    .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text());
}

#[tokio::test]
async fn another_projects_repository_and_another_organizations_project_are_not_there() {
    let (_server, state) = world().await;
    let listed = doors::call("jc_repository_list", &session(viewer()), &state, json!({})).await;
    let names: Vec<&str> = listed.body["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert_eq!(names, vec!["ovzdusie-config", "ovzdusie_dash"]);
    for repository in [
        "doprava_maps",
        "doprava-config",
        "ovzdusie-brno",
        "nonexistent",
    ] {
        let answer = doors::call(
            "jc_repository_get",
            &session(steward()),
            &state,
            json!({ "repository": repository }),
        )
        .await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{repository}");
    }
    // `doprava` is a project the steward of `ovzdusie` holds nothing in: its repositories read
    // like a project that is not there, settings included.
    for name in OPERATIONS {
        let answer = doors::call_in(
            name,
            &session(steward()),
            &state,
            "doprava",
            json!({ "repository": "doprava_maps" }),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{name}: {}",
            answer.text()
        );
    }
}

#[tokio::test]
async fn a_traversal_is_refused_before_the_forge_is_asked() {
    let (server, state) = world().await;
    let before = server.received_requests().await.map_or(0, |r| r.len());
    for (name, bad) in [
        ("jc_repository_file", "../../etc/passwd"),
        ("jc_repository_file", "/etc/passwd"),
        ("jc_repository_tree", "src/../.."),
        ("jc_repository_commits", "a\\b"),
    ] {
        let answer = doors::call(
            name,
            &session(steward()),
            &state,
            json!({ "repository": "ovzdusie-config", "path": bad }),
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{name} {bad}: {}",
            answer.text()
        );
    }
    for bad_ref in ["main..dev", "../x", "a'b"] {
        let answer = doors::call(
            "jc_repository_tree",
            &session(steward()),
            &state,
            json!({ "repository": "ovzdusie-config", "ref": bad_ref }),
        )
        .await;
        assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad_ref}");
    }
    let unknown = doors::call(
        "jc_repository_file",
        &session(steward()),
        &state,
        json!({ "repository": "ovzdusie-config", "path": "a", "raw": true }),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY);
    let after = server.received_requests().await.map_or(0, |r| r.len());
    assert_eq!(before, after, "a refused input reached the forge");
}

#[tokio::test]
async fn a_file_is_text_as_data_binary_by_its_bytes_and_a_large_one_unread() {
    let (_server, state) = world().await;
    let read = |path: &str| {
        let state = state.clone();
        let path = path.to_owned();
        async move {
            doors::call(
                "jc_repository_file",
                &session(viewer()),
                &state,
                json!({ "repository": "ovzdusie-config", "path": path }),
            )
            .await
        }
    };
    // Markup comes back as the characters it is: the page draws them, never renders them.
    let html = read("src/page.html").await;
    assert_eq!(html.body["text"], XSS, "{}", html.text());
    assert_eq!(html.body["binary"], false);
    let png = read("logo.png").await;
    assert_eq!(png.body["binary"], true);
    assert!(png.body.get("text").is_none());
    let big = read("big.csv").await;
    assert_eq!(big.body["tooLarge"], true);
    assert!(big.body.get("text").is_none());
    let folder = read("src").await;
    assert_eq!(folder.status, StatusCode::NOT_FOUND, "a folder is no file");
}

#[tokio::test]
async fn the_reads_answer_in_the_shapes_the_pages_draw() {
    let (_server, state) = world().await;
    let call = |name: &str| {
        let state = state.clone();
        let name = name.to_owned();
        async move {
            doors::call(&name, &session(steward()), &state, input(&name))
                .await
                .body
        }
    };
    let repo = call("jc_repository_get").await;
    assert_eq!(repo["readme"]["text"], XSS);
    assert_eq!(repo["lastCommit"]["message"], "Change b");
    assert_eq!(repo["cloneUrl"], "https://git.example/ovzdusie-config.git");
    let tree = call("jc_repository_tree").await;
    assert_eq!(tree["entries"][0]["path"], "src/page.html");
    let commit = call("jc_repository_commit").await;
    assert_eq!(commit["parents"][0], PARENT);
    assert!(commit["files"][0]["patch"].is_string(), "{commit}");
    let refs = call("jc_repository_refs").await;
    assert_eq!(refs["branches"][0]["name"], "main", "the default first");
    let pull = call("jc_repository_pull").await;
    assert_eq!(pull["change"], "chg-00000007");
    assert_eq!(pull["reviews"][0]["verdict"], "APPROVED");
    let settings = call("jc_repository_settings").await;
    let shown = settings.to_string();
    assert!(
        !shown.contains("hook-secret")
            && !shown.contains("never-shown-secret")
            && !shown.contains("key=k"),
        "{shown}"
    );
    assert_eq!(
        settings["webhooks"][0]["address"],
        "https://portal.example/api/v1/webhooks/shako"
    );
    // Each read leaves an activity row naming the repository.
    let rows = state
        .activity
        .list(
            PROJECT,
            &joinedcontext_portal::activity::ActivityFilter {
                kinds: vec!["repository.read".into()],
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .expect("activity");
    assert!(
        rows.items.iter().any(
            |row| row.details["object"] == "repositories/ovzdusie-config"
                && row.details["actor"] == "jana"
        ),
        "{:?}",
        rows.items
    );
}
