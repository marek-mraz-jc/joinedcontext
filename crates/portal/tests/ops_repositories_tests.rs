//! The repository reads through every door of the registry (`src/ops/repositories.rs`): who reads
//! what, and what is never there.
//!
//! The forge is a stub answering shako's OData shapes; `crates/shako/server/tests/
//! portal_repositories.rs` runs the same operations against real shako with git on disk.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::http::StatusCode;
use common::doors::{self, mcp, run, session, steward, stranger, viewer, PROJECT};
use common::state_on;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::forge::ShakoClient;
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
        "Sha": "b", "Size": content.len(),
        "Content": base64::engine::general_purpose::STANDARD.encode(content),
    })
}

/// A forge holding the project's configuration repository, its application's repository, and
/// another project's application.
async fn world() -> (MockServer, AppState) {
    world_on(None).await
}

/// [`world`] with the forge's public address (`JC_SHAKO_PUBLIC_URL`) set to `public`.
async fn world_on(public: Option<&str>) -> (MockServer, AppState) {
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
        json!({ "Sha": ROOT, "Tree": [
        { "Path": "README.md", "Mode": "100644", "Type": "blob", "Size": XSS.len(), "Sha": "r" },
        { "Path": "logo.png", "Mode": "100644", "Type": "blob", "Size": 6, "Sha": "p" },
        { "Path": "big.csv", "Mode": "100644", "Type": "blob", "Size": 5_000_000, "Sha": "c" },
        { "Path": "src", "Mode": "040000", "Type": "tree", "Size": 0, "Sha": SRC },
    ] }),
    )
    .await;
    get_with(
        &server,
        &tree,
        "Ref",
        SRC,
        json!({ "Sha": SRC, "Tree": [
        { "Path": "page.html", "Mode": "100644", "Type": "blob", "Size": XSS.len(), "Sha": "h" },
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
        "Sha": HEAD, "Message": "Change b\n\nWhy it changed", "Author": "Alice",
        "Email": "alice@city.example", "Date": "2026-11-08T10:00:00Z",
        "Parents": [PARENT],
    }] }),
    )
    .await;
    get(
        &server,
        &format!("/odata/Repositories('{CONFIG_ID}')/Compare"),
        json!({ "files": [{ "filename": "src/page.html", "status": "modified" }] }),
    )
    .await;
    // Only a read in the walk's order is answered: by key, commits come back in sha order.
    get_with(
        &server,
        "/odata/Commits",
        "$orderby",
        "Position",
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
        "Files": [{ "Filename": "c.yaml", "Status": "added" }],
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

    let state = match public {
        None => state_on(&server),
        Some(public) => {
            let mut client = ShakoClient::new(
                server.uri().parse().expect("mock url"),
                "test-owner",
                "test-repo",
                "token-xyz",
            )
            .expect("client");
            client.public_base = public.parse().expect("public url");
            AppState::new(Config::for_tests(), None).with_shako(Arc::new(client))
        }
    };
    let state = doors::state_with(state);
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
async fn the_clone_url_is_the_public_address() {
    let (_server, state) = world_on(Some("https://jc.example/git/")).await;
    let name = "jc_repository_get";
    let repo = doors::call(name, &session(steward()), &state, input(name))
        .await
        .body;
    assert_eq!(
        repo["cloneUrl"],
        "https://jc.example/git/test-owner/ovzdusie-config.git"
    );
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
    assert_eq!(
        repo["cloneUrl"], "https://git.example/ovzdusie-config.git",
        "no public address: the forge's own CloneUrl, never the dial address"
    );
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
                && row.details["actor"] == "f:1:jana"
        ),
        "{:?}",
        rows.items
    );
}

/// The reads' other shapes: one path's history, a page with more behind it, a root commit's
/// whole tree as added files, a commit or a pull request the forge does not hold, closed and
/// every pull request, packages in the settings, and a folder or a file the tree does not hold.
#[tokio::test]
async fn the_reads_page_walk_a_root_commit_and_name_what_is_not_there() {
    let (server, state) = world().await;
    let first = |at: String, name: &'static str, value: &'static str, answer: Value| {
        let server = &server;
        async move {
            Mock::given(method("GET"))
                .and(path(at))
                .and(query_param(name, value))
                .respond_with(ResponseTemplate::new(200).set_body_json(answer))
                .with_priority(1)
                .mount(server)
                .await;
        }
    };
    first(
        format!("/odata/Repositories('{CONFIG_ID}')/History"),
        "Ref",
        ROOT,
        json!({ "value": [{ "Sha": ROOT, "Message": "Initial", "Author": "Alice",
                            "Date": "2026-11-01T10:00:00Z", "Parents": [] }] }),
    )
    .await;
    first(
        format!("/odata/Repositories('{CONFIG_ID}')/History"),
        "Ref",
        PARENT,
        json!({ "value": [] }),
    )
    .await;
    first(
        format!("/odata/Repositories('{CONFIG_ID}')/Tree"),
        "Ref",
        ROOT,
        json!({ "Sha": ROOT, "Tree": [
            { "Path": "README.md", "Mode": "100644", "Type": "blob", "Size": XSS.len(), "Sha": "r" },
            { "Path": "src", "Mode": "040000", "Type": "tree", "Size": 0, "Sha": SRC },
        ] }),
    )
    .await;
    first(
        "/odata/Commits".into(),
        "$orderby",
        "Position",
        json!({ "value": [{ "Sha": HEAD, "Message": "Change b", "Author": "Alice",
                            "Date": "2026-11-08T10:00:00Z" }], "@odata.count": 5 }),
    )
    .await;
    first(
        "/odata/PullRequests".into(),
        "$orderby",
        "Number desc",
        json!({ "value": [], "@odata.count": 5 }),
    )
    .await;
    first(
        "/odata/PackageVersions".into(),
        "$top",
        "100",
        json!({ "value": [{ "Type": "container", "Name": "ovzdusie-config", "Version": "1.0.0",
                            "Digest": "sha256:ab", "Size": 10, "CreatedAt": "2026-11-08T10:00:00Z" }] }),
    )
    .await;
    let r = "ovzdusie-config";
    let call = |name: &'static str, input: Value| {
        let state = state.clone();
        async move { doors::call(name, &session(steward()), &state, input).await }
    };

    let history = call(
        "jc_repository_commits",
        json!({ "repository": r, "path": "src" }),
    )
    .await;
    assert_eq!(history.status, StatusCode::OK, "{}", history.text());
    assert_eq!(history.body["ref"], "main");
    let paged = call(
        "jc_repository_commits",
        json!({ "repository": r, "limit": 1 }),
    )
    .await;
    assert_eq!(paged.body["nextCursor"], 1, "{}", paged.text());
    assert_eq!(paged.body["total"], 5);

    let root = call(
        "jc_repository_commit",
        json!({ "repository": r, "sha": ROOT }),
    )
    .await;
    assert_eq!(root.status, StatusCode::OK, "{}", root.text());
    let files = root.body["files"].as_array().expect("files");
    assert!(
        files.iter().all(|f| f["status"] == "added") && files.len() == 1,
        "{}",
        root.text()
    );
    let gone = call(
        "jc_repository_commit",
        json!({ "repository": r, "sha": PARENT }),
    )
    .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND, "{}", gone.text());
    assert!(gone
        .text()
        .contains(&format!("commit '{PARENT}' not found")));

    for state in ["closed", "all"] {
        let pulls = call(
            "jc_repository_pulls",
            json!({ "repository": r, "state": state, "limit": 1 }),
        )
        .await;
        assert_eq!(pulls.body["nextCursor"], 1, "{state}: {}", pulls.text());
    }
    let no_pull = call(
        "jc_repository_pull",
        json!({ "repository": r, "number": 9 }),
    )
    .await;
    assert_eq!(no_pull.status, StatusCode::NOT_FOUND, "{}", no_pull.text());
    assert!(no_pull
        .text()
        .contains("pull request 9 not found in 'ovzdusie-config'"));

    let settings = call("jc_repository_settings", json!({ "repository": r })).await;
    assert_eq!(
        settings.body["packages"][0]["version"],
        "1.0.0",
        "{}",
        settings.text()
    );
    assert_eq!(settings.body["packages"][0]["signed"], false);

    let folder = call(
        "jc_repository_tree",
        json!({ "repository": r, "path": "nope" }),
    )
    .await;
    assert_eq!(folder.status, StatusCode::NOT_FOUND, "{}", folder.text());
    assert!(folder.text().contains("folder 'nope' not found"));
    let file = call(
        "jc_repository_file",
        json!({ "repository": r, "path": "src/missing.txt" }),
    )
    .await;
    assert_eq!(file.status, StatusCode::NOT_FOUND, "{}", file.text());
    let nameless = call("jc_repository_file", json!({ "repository": r, "path": "" })).await;
    assert_eq!(
        nameless.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        nameless.text()
    );
}

/// The list filters by name or description, case-blind, and pages with a cursor while more
/// remain; a filter matching nothing is an empty page.
#[tokio::test]
async fn the_list_filters_by_name_or_description_and_pages() {
    let (_server, state) = world().await;
    let caller = session(steward());
    let call = |input: Value| doors::call("jc_repository_list", &caller, &state, input);
    let all = call(json!({})).await;
    assert_eq!(all.status, StatusCode::OK, "{}", all.text());
    let total = all.body["total"].as_u64().expect("total");
    let named = call(json!({ "filter": "DASH" })).await;
    assert_eq!(named.body["total"], 1, "{}", named.text());
    assert_eq!(named.body["items"][0]["name"], "ovzdusie_dash");
    let described = call(json!({ "filter": "config repository" })).await;
    assert_eq!(described.body["total"], 1, "{}", described.text());
    let none = call(json!({ "filter": "no such words" })).await;
    assert_eq!(none.body["total"], 0);
    assert_eq!(none.body["items"], json!([]));
    if total > 1 {
        let first = call(json!({ "limit": 1 })).await;
        assert_eq!(first.body["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(first.body["nextCursor"], 1, "{}", first.text());
        let last = call(json!({ "cursor": total - 1, "limit": 1 })).await;
        assert!(last.body.get("nextCursor").is_none(), "{}", last.text());
    }
}

/// A folder lists folders first, then files and submodules by name; a path deeper than 32
/// folders is refused before the forge is asked; a file that is not at the ref is a 404.
#[tokio::test]
async fn a_folder_sorts_folders_first_and_a_deep_or_missing_path_is_refused() {
    let (server, state) = world().await;
    get_with(
        &server,
        &format!("/odata/Repositories('{CONFIG_ID}')/Tree"),
        "Ref",
        "dev",
        json!({ "Sha": ROOT, "Tree": [
            { "Path": "zeta.txt", "Type": "blob", "Size": 1, "Sha": "z" },
            { "Path": "vendor", "Type": "commit", "Size": 0, "Sha": "v" },
            { "Path": "lib", "Type": "tree", "Size": 0, "Sha": "l" },
            { "Path": "app", "Type": "tree", "Size": 0, "Sha": "a" },
        ] }),
    )
    .await;
    let tree = doors::call(
        "jc_repository_tree",
        &session(steward()),
        &state,
        json!({ "repository": "ovzdusie-config", "ref": "dev" }),
    )
    .await;
    assert_eq!(tree.status, StatusCode::OK, "{}", tree.text());
    let rows: Vec<(String, String)> = tree.body["entries"]
        .as_array()
        .or_else(|| tree.body["items"].as_array())
        .expect("entries")
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap_or_default().to_owned(),
                e["type"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            ("app".into(), "folder".into()),
            ("lib".into(), "folder".into()),
            ("vendor".into(), "submodule".into()),
            ("zeta.txt".into(), "file".into()),
        ]
    );
    let deep = vec!["d"; 33].join("/");
    let refused = doors::call(
        "jc_repository_tree",
        &session(steward()),
        &state,
        json!({ "repository": "ovzdusie-config", "path": deep }),
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.text()
    );
    assert!(
        refused.text().contains("deeper than 32 folders"),
        "{}",
        refused.text()
    );
    let missing = doors::call(
        "jc_repository_file",
        &session(steward()),
        &state,
        json!({ "repository": "ovzdusie-config", "path": "nothere.txt" }),
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "{}", missing.text());
}

/// A commit's diff: a removal has no after side, a binary file is named binary without a patch,
/// files past the thirtieth are listed without one; a file the forge no longer holds is not found.
#[tokio::test]
async fn a_commit_diff_names_removals_binaries_and_the_files_past_its_cap() {
    let (server, state) = world().await;
    let mut changed = vec![
        json!({ "filename": "README.md", "status": "deleted" }),
        json!({ "filename": "logo.png", "status": "modified" }),
    ];
    changed.extend((0..30).map(|n| json!({ "filename": format!("x{n}.txt"), "status": "added" })));
    Mock::given(method("GET"))
        .and(path(format!("/odata/Repositories('{CONFIG_ID}')/Compare")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "files": changed })))
        .with_priority(1)
        .mount(&server)
        .await;
    let commit = doors::call(
        "jc_repository_commit",
        &session(steward()),
        &state,
        input("jc_repository_commit"),
    )
    .await;
    assert_eq!(commit.status, StatusCode::OK, "{}", commit.text());
    let files = commit.body["files"].as_array().expect("files");
    assert_eq!(files.len(), 32, "{}", commit.text());
    assert_eq!(files[0]["status"], "deleted");
    assert_eq!(files[1]["binary"], true, "{}", files[1]);
    assert!(files[31].get("patch").is_none(), "{}", files[31]);
    assert_eq!(commit.body["truncated"], true, "{}", commit.text());

    get_with(
        &server,
        &format!("/odata/Repositories('{CONFIG_ID}')/Tree"),
        "Ref",
        "dev",
        json!({ "Sha": ROOT, "Tree": [{ "Path": "gone.txt", "Type": "blob", "Size": 1, "Sha": "g" }] }),
    )
    .await;
    let gone = doors::call(
        "jc_repository_file",
        &session(steward()),
        &state,
        json!({ "repository": "ovzdusie-config", "path": "gone.txt", "ref": "dev" }),
    )
    .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND, "{}", gone.text());
}

/// A repository with no branch yet is empty, no error; a forge failing the root read is said.
#[tokio::test]
async fn an_empty_repository_is_empty_and_a_failing_root_read_is_an_error() {
    let (server, state) = world().await;
    let empty = doors::call(
        "jc_repository_get",
        &session(steward()),
        &state,
        json!({ "repository": "ovzdusie_dash" }),
    )
    .await;
    assert_eq!(empty.status, StatusCode::OK, "{}", empty.text());
    assert_eq!(empty.body["empty"], true, "{}", empty.text());
    assert!(empty.body.get("readme").is_none(), "{}", empty.text());
    Mock::given(method("GET"))
        .and(path(format!("/odata/Repositories('{APP_ID}')/Tree")))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({ "message": "down" })))
        .mount(&server)
        .await;
    let failed = doors::call(
        "jc_repository_get",
        &session(steward()),
        &state,
        json!({ "repository": "ovzdusie_dash" }),
    )
    .await;
    assert!(
        failed.status.is_server_error(),
        "{}: {}",
        failed.status,
        failed.text()
    );
}

/// A forge answer short of a field reads its default: an empty default branch is `main` and an
/// empty owner the client's. A broken one is an error: an id that is no UUID, a file that does
/// not decode, a failing file read. A binary README is not shown.
#[tokio::test]
async fn a_forge_answer_short_of_a_field_reads_its_default_and_a_broken_one_is_an_error() {
    let (server, state) = world_on(Some("https://git.public.example")).await;
    let mut bare = repo(CONFIG_ID, "ovzdusie-config", "");
    bare["Owner"] = json!("");
    bare["DefaultBranch"] = json!("");
    Mock::given(method("GET"))
        .and(path("/odata/Repositories"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [
            bare,
            repo(APP_ID, "ovzdusie_dash", ""),
            repo("not-a-uuid", "ovzdusie_odd", ""),
        ] })))
        .with_priority(1)
        .mount(&server)
        .await;
    let file = format!("/odata/Repositories('{CONFIG_ID}')/File");
    for (at, answer) in [
        (
            "README.md",
            ResponseTemplate::new(200).set_body_json(blob(b"\0\x01\x02 binary")),
        ),
        (
            "src/page.html",
            ResponseTemplate::new(200)
                .set_body_json(json!({ "Sha": "b", "Size": 3, "Content": "!!!" })),
        ),
        (
            "logo.png",
            ResponseTemplate::new(500).set_body_json(json!({ "message": "down" })),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(file.clone()))
            .and(query_param("Path", at))
            .respond_with(answer)
            .with_priority(1)
            .mount(&server)
            .await;
    }
    let got = doors::call(
        "jc_repository_get",
        &session(steward()),
        &state,
        json!({ "repository": "ovzdusie-config" }),
    )
    .await;
    assert_eq!(got.status, StatusCode::OK, "{}", got.text());
    assert_eq!(
        got.body["cloneUrl"],
        "https://git.public.example/test-owner/ovzdusie-config.git",
        "{}",
        got.text()
    );
    assert!(got.body.get("readme").is_none(), "{}", got.text());
    for (repository, path) in [
        ("ovzdusie-config", "src/page.html"),
        ("ovzdusie-config", "logo.png"),
        ("ovzdusie_odd", "README.md"),
    ] {
        let failed = doors::call(
            "jc_repository_file",
            &session(steward()),
            &state,
            json!({ "repository": repository, "path": path }),
        )
        .await;
        assert!(
            failed.status.is_server_error(),
            "{repository} {path}: {} {}",
            failed.status,
            failed.text()
        );
    }
}
