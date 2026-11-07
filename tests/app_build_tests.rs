//! The App page's build: the repository, the newest run and the package, and Rebuild as a
//! dispatch of the application's own workflow (AP-103, ADR-N-028, T-2609).

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{checked_send as send, envelope, person};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ResourceEnvelope, API_VERSION};
use joinedcontext_portal::state::AppState;

/// `common::state_on` names the organization `test-owner`; the App's repository is `{project}_{app}`.
const REPO: &str = "/api/v1/repos/test-owner/helsinki_bikes";
const COMMIT: &str = "3f1c0e2d7a6b5c4f1b9c0e2d7a6b5c4f1b9c0e2d";

fn app(git: bool) -> ResourceEnvelope {
    let source = if git {
        // A manifest naming another repository changes nothing: the path names the repository.
        json!({ "git": { "url": "https://forge.example/test-owner/other_repo.git", "ref": COMMIT } })
    } else {
        json!({ "path": "./src" })
    };
    serde_json::from_value(json!({
        "apiVersion": API_VERSION,
        "kind": "App",
        "metadata": { "name": "bikes", "namespace": "helsinki" },
        "spec": {
            "kind": "static",
            "source": source,
            "build": { "node": "22" },
            "visibility": "project",
            "lifecycle": "published",
            "dataNeeds": [],
        },
        "status": { "build": {
            "digest": format!("sha256:{}", "ab".repeat(32)),
            "commit": COMMIT,
            "sdkVersion": "0.4.1",
            "builtAt": "2026-09-22T06:00:00Z",
        }},
    }))
    .expect("an App")
}

/// `jana` proposes Apps in helsinki, `vera` only reads them, anyone else has no grant here.
fn state_with(gitea: &MockServer, git: bool) -> AppState {
    let state = common::state_on(gitea);
    for (role, verbs) in [
        ("app-editor", json!(["propose"])),
        ("app-reader", json!(["read"])),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["App"], "verbs": verbs }] }),
        ));
    }
    for (who, role) in [("jana", "app-editor"), ("vera", "app-reader")] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{who}-{role}"),
            ORG_NAMESPACE,
            json!({
                "subjects": [{ "user": format!("{who}@hel.fi") }],
                "role": role,
                "scope": { "project": "helsinki" },
            }),
        ));
    }
    state.mirror.upsert(app(git));
    state
}

async fn forge_with_a_run() -> MockServer {
    let gitea = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/actions/runs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_count": 1,
            "workflow_runs": [{
                "id": 91, "run_number": 7, "status": "completed", "conclusion": "success",
                "head_sha": COMMIT, "html_url": "http://gitea-http:3000/test-owner/helsinki_bikes/actions/runs/7",
            }],
        })))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&gitea)
        .await;
    gitea
}

fn body(text: &str) -> Value {
    serde_json::from_str(text).expect("a JSON answer")
}

async fn dispatches(gitea: &MockServer) -> Vec<String> {
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST")
        .map(|r| r.url.path().to_owned())
        .collect()
}

/// AP-103, PF-81: the page links the repository, the newest run and the package of the build,
/// each behind the forge's sign-in, and offers Rebuild only to a person who may propose an App.
#[tokio::test]
async fn the_build_links_the_repository_the_run_and_the_package_behind_the_forge_sign_in() {
    let gitea = forge_with_a_run().await;
    let state = state_with(&gitea, true);
    const URI: &str = "/api/v1/projects/helsinki/apps/bikes/build";

    let answer = send(&state, person("jana"), "GET", URI, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let build = body(&answer.text);
    let login = |target: &str| {
        format!(
            "{}/user/oauth2/keycloak?redirect_to={}",
            gitea.uri(),
            target
        )
    };
    assert_eq!(
        build["repositoryUrl"],
        login("%2Ftest-owner%2Fhelsinki_bikes")
    );
    assert_eq!(
        build["run"],
        json!({
            "status": "completed",
            "conclusion": "success",
            "commit": COMMIT,
            "url": login("%2Ftest-owner%2Fhelsinki_bikes%2Factions%2Fruns%2F7"),
            "number": 7,
        })
    );
    assert_eq!(
        build["packageUrl"],
        login(&format!(
            "%2Ftest-owner%2F-%2Fpackages%2Fgeneric%2Fapp-bikes%2F{COMMIT}-abababababab"
        ))
    );
    assert_eq!(build["rebuild"], json!({ "allowed": true }));

    let reader = body(&send(&state, person("vera"), "GET", URI, None).await.text);
    assert_eq!(reader["rebuild"]["allowed"], false);
    assert!(
        reader["rebuild"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("propose on App"),
        "{reader}"
    );

    let stranger = send(&state, person("otto"), "GET", URI, None).await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND, "{}", stranger.text);
    let missing = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/helsinki/apps/gone/build",
        None,
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(
        stranger.text.replace("bikes", "gone"),
        missing.text,
        "one answer for both"
    );
}

/// AP-103: Rebuild dispatches `build.yml` of the App's own repository on its default branch,
/// whatever repository the manifest names; a reader is refused and nothing is dispatched.
#[tokio::test]
async fn rebuild_dispatches_the_apps_own_workflow_on_its_default_branch() {
    let gitea = forge_with_a_run().await;
    Mock::given(method("POST"))
        .and(path(format!(
            "{REPO}/actions/workflows/build.yml/dispatches"
        )))
        .and(body_json(json!({ "ref": "main" })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&gitea)
        .await;
    let state = state_with(&gitea, true);
    const URI: &str = "/api/v1/projects/helsinki/apps/bikes/rebuild";

    let refused = send(&state, person("vera"), "POST", URI, None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(
        dispatches(&gitea).await.is_empty(),
        "a reader dispatched a build"
    );

    let accepted = send(&state, person("jana"), "POST", URI, None).await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    assert_eq!(
        dispatches(&gitea).await,
        vec![format!("{REPO}/actions/workflows/build.yml/dispatches")]
    );
}

/// AP-100, AP-103: an App that names no repository of its own has no build on the forge; the
/// page says so and Rebuild is refused `409` with the reason, and nothing reaches the forge.
#[tokio::test]
async fn an_app_not_built_on_the_forge_has_no_links_and_no_rebuild() {
    let gitea = MockServer::start().await;
    let state = state_with(&gitea, false);

    let answer = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/helsinki/apps/bikes/build",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let build = body(&answer.text);
    assert_eq!(build["repositoryUrl"], Value::Null);
    assert_eq!(build["run"], Value::Null);
    assert_eq!(build["packageUrl"], Value::Null);
    // jana is bound to the project only, so layout 1's organization repository is not hers.
    assert_eq!(build["configurationUrl"], Value::Null);
    assert_eq!(build["rebuild"]["allowed"], false);
    assert!(build["rebuild"]["reason"]
        .as_str()
        .unwrap_or_default()
        .contains("AP-100"));

    let refused = send(
        &state,
        person("jana"),
        "POST",
        "/api/v1/projects/helsinki/apps/bikes/rebuild",
        None,
    )
    .await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    assert!(gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .is_empty());
}

/// AP-103: a dispatch the forge refuses is a `503` with the forge's own words, and a repository
/// without a run yet is a page with no run rather than an error.
#[tokio::test]
async fn a_refused_dispatch_says_the_forges_reason_and_no_run_is_no_error() {
    let gitea = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/actions/runs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "workflow_runs": [] })))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&gitea)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "{REPO}/actions/workflows/build.yml/dispatches"
        )))
        .respond_with(
            ResponseTemplate::new(422).set_body_string("Actions are disabled for this repository"),
        )
        .mount(&gitea)
        .await;
    let state = state_with(&gitea, true);

    let page = body(
        &send(
            &state,
            person("jana"),
            "GET",
            "/api/v1/projects/helsinki/apps/bikes/build",
            None,
        )
        .await
        .text,
    );
    assert_eq!(page["run"], Value::Null);

    let refused = send(
        &state,
        person("jana"),
        "POST",
        "/api/v1/projects/helsinki/apps/bikes/rebuild",
        None,
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        refused.text
    );
    assert!(
        refused.text.contains("Actions are disabled"),
        "{}",
        refused.text
    );
}

const BUILD: &str = "/api/v1/projects/helsinki/apps/bikes/build";

fn login(gitea: &MockServer, target: &str) -> Value {
    json!(format!(
        "{}/user/oauth2/keycloak?redirect_to={target}",
        gitea.uri()
    ))
}

/// T-3039, PF-87: in layout 1 the configuration repository is the organization's, which the
/// forge reads to a person bound at the organization and to nobody bound only to the project;
/// the App's own repository is read by every signed-in person who may read the App (T-3030).
#[tokio::test]
async fn in_layout_1_the_configuration_link_is_for_a_person_bound_at_the_organization() {
    let gitea = forge_with_a_run().await;
    let state = state_with(&gitea, true);
    state.mirror.upsert(envelope(
        "RoleBinding",
        "olga-app-reader",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "olga@hel.fi" }],
            "role": "app-reader",
            "scope": { "organization": "hel" },
        }),
    ));

    let jana = body(&send(&state, person("jana"), "GET", BUILD, None).await.text);
    assert_eq!(jana["configurationUrl"], Value::Null);
    assert_eq!(
        jana["repositoryUrl"],
        login(&gitea, "%2Ftest-owner%2Fhelsinki_bikes")
    );

    let olga = body(&send(&state, person("olga"), "GET", BUILD, None).await.text);
    assert_eq!(
        olga["configurationUrl"],
        login(&gitea, "%2Ftest-owner%2Ftest-repo")
    );
    assert_eq!(
        olga["repositoryUrl"],
        login(&gitea, "%2Ftest-owner%2Fhelsinki_bikes")
    );
}

/// T-3039, PF-87, CC-87: in layout 2 the configuration link is the project's own repository,
/// for every person the project's bindings place in its readers, a read-only one included; the
/// address follows the platform's record of the repository, so a renamed one moves with it.
#[tokio::test]
async fn in_layout_2_the_configuration_link_is_the_projects_own_repository_for_its_readers() {
    let gitea = forge_with_a_run().await;
    let state = state_with(&gitea, true);
    state.mirror.set_layout(2);
    state
        .mirror
        .set_repositories([("helsinki".to_owned(), "helsinki".to_owned())].into());

    for who in ["jana", "vera"] {
        let build = body(&send(&state, person(who), "GET", BUILD, None).await.text);
        assert_eq!(
            build["configurationUrl"],
            login(&gitea, "%2Ftest-owner%2Fhelsinki"),
            "{who}"
        );
    }

    state
        .mirror
        .set_repositories([("helsinki".to_owned(), "helsinki-city".to_owned())].into());
    let renamed = body(&send(&state, person("vera"), "GET", BUILD, None).await.text);
    assert_eq!(
        renamed["configurationUrl"],
        login(&gitea, "%2Ftest-owner%2Fhelsinki-city")
    );
}

/// T-3245: a running build is shown against the newest finished successful run, and the run
/// carries its number and times, so a client tells the run it started from the one before.
#[tokio::test]
async fn a_build_carries_its_runs_number_and_times_and_the_estimate_of_the_last_success() {
    let gitea = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/actions/runs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "workflow_runs": [
                { "run_number": 9, "status": "in_progress", "head_sha": COMMIT, "started_at": "2026-10-07T10:00:00Z" },
                { "run_number": 8, "status": "completed", "conclusion": "failure", "head_sha": COMMIT,
                  "started_at": "2026-10-07T09:00:00Z", "completed_at": "2026-10-07T09:00:20Z" },
                { "run_number": 7, "status": "completed", "conclusion": "success", "head_sha": COMMIT,
                  "started_at": "2026-10-07T08:00:00Z", "completed_at": "2026-10-07T08:03:05Z" }
            ],
        })))
        .mount(&gitea)
        .await;
    let state = state_with(&gitea, true);
    let answer = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/helsinki/apps/bikes/build",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let build = body(&answer.text);
    assert_eq!(build["run"]["number"], json!(9));
    assert_eq!(build["run"]["status"], json!("in_progress"));
    assert_eq!(build["run"]["startedAt"], json!("2026-10-07T10:00:00Z"));
    assert!(build["run"].get("completedAt").is_none(), "{build}");
    // The failed run is no estimate; the last success took 3 min 5 s.
    assert_eq!(build["typicalSeconds"], json!(185), "{build}");
}

/// No successful run yet is no estimate rather than a guess.
#[tokio::test]
async fn without_a_finished_success_there_is_no_estimate() {
    let gitea = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/actions/runs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "workflow_runs": [{ "run_number": 1, "status": "queued", "head_sha": COMMIT }],
        })))
        .mount(&gitea)
        .await;
    let state = state_with(&gitea, true);
    let answer = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/helsinki/apps/bikes/build",
        None,
    )
    .await;
    let build = body(&answer.text);
    assert!(build.get("typicalSeconds").is_none(), "{build}");
    assert_eq!(build["run"]["number"], json!(1));
}

const OLD: &str = "9a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";
const OTHER: &str = "1111111111111111111111111111111111111111";

/// A forge with the current build, an earlier one built twice, a failed run, the two trees and
/// the forge's write routes.
async fn forge_with_builds() -> MockServer {
    let gitea = MockServer::start().await;
    let run = |number: u64, commit: &str, conclusion: &str| {
        json!({ "id": number, "run_number": number, "status": "completed", "conclusion": conclusion,
                "head_sha": commit, "html_url": "", "completed_at": "2026-10-05T08:12:40Z" })
    };
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/actions/runs")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_count": 4,
            "workflow_runs": [run(9, OTHER, "failure"), run(8, COMMIT, "success"),
                              run(7, OLD, "success"), run(6, OLD, "success")],
        })))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&gitea)
        .await;
    let tree =
        |entries: Value| ResponseTemplate::new(200).set_body_json(json!({ "tree": entries }));
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/git/trees/{OLD}")))
        .respond_with(tree(json!([
            { "path": "index.html", "type": "blob", "sha": "a1" },
            { "path": "src/App.tsx", "type": "blob", "sha": "old" },
            { "path": "src", "type": "tree", "sha": "t1" },
        ])))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/git/trees/main")))
        .respond_with(tree(json!([
            { "path": "index.html", "type": "blob", "sha": "a1" },
            { "path": "src/App.tsx", "type": "blob", "sha": "new" },
            { "path": "src/New.tsx", "type": "blob", "sha": "n1" },
        ])))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/contents/src/App.tsx")))
        .and(wiremock::matchers::query_param("ref", OLD))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "old", "content": "ZXhwb3J0IGNvbnN0IG9sZCA9IDE7", // "export const old = 1;"
        })))
        .mount(&gitea)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/branches")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&gitea)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/contents")))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c0ffee" } })),
        )
        .mount(&gitea)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/pulls")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 3, "html_url": "https://forge.example/test-owner/helsinki_bikes/pulls/3",
            "state": "open", "title": "restore", "body": "", "head": { "ref": "restore" },
            "base": { "ref": "main" }, "created_at": "2026-10-10T21:00:00Z",
            "user": { "login": "jana" }, "mergeable": true, "merged": false,
        })))
        .mount(&gitea)
        .await;
    gitea
}

async fn posted(gitea: &MockServer, route: &str) -> Vec<Value> {
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == format!("{REPO}/{route}"))
        .map(|r| serde_json::from_slice(&r.body).expect("a JSON body"))
        .collect()
}

/// AP-171: the builds are the successful runs, one per commit, newest first, the current one
/// marked, and Restore is offered to a person who may propose an App, never to a reader.
#[tokio::test]
async fn the_builds_are_the_successful_runs_once_per_commit_and_restore_needs_propose() {
    let gitea = forge_with_builds().await;
    let state = state_with(&gitea, true);
    const URI: &str = "/api/v1/projects/helsinki/apps/bikes/builds";

    let answer = send(&state, person("jana"), "GET", URI, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let builds = body(&answer.text);
    let listed: Vec<(&str, bool)> = builds["builds"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|b| {
            (
                b["commit"].as_str().unwrap_or_default(),
                b["current"] == true,
            )
        })
        .collect();
    assert_eq!(listed, vec![(COMMIT, true), (OLD, false)]);
    assert_eq!(builds["builds"][1]["number"], 7);
    assert_eq!(builds["restore"], json!({ "allowed": true }));
    assert!(builds["dataNote"]
        .as_str()
        .unwrap_or_default()
        .contains("data stay"));

    let reader = body(&send(&state, person("vera"), "GET", URI, None).await.text);
    assert_eq!(reader["restore"]["allowed"], false);
    assert!(reader["restore"]["reason"]
        .as_str()
        .unwrap_or_default()
        .starts_with("Restore needs propose on App"));
    let stranger = send(&state, person("otto"), "GET", URI, None).await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND, "{}", stranger.text);
}

/// AP-171, AP-73: Restore opens a merge request into the default branch that writes back the
/// changed file of the build's commit and removes the file it did not hold, and the Portal
/// never writes `status.build`.
#[tokio::test]
async fn restore_opens_a_merge_request_back_to_the_builds_commit() {
    let gitea = forge_with_builds().await;
    let state = state_with(&gitea, true);
    const URI: &str = "/api/v1/projects/helsinki/apps/bikes/restore";

    let answer = send(
        &state,
        person("jana"),
        "POST",
        URI,
        Some(json!({ "commit": OLD })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CREATED, "{}", answer.text);
    let restored = body(&answer.text);
    let branch = restored["branch"].as_str().unwrap_or_default().to_owned();
    assert!(branch.starts_with("restore/9a2b3c4-"), "{restored}");
    assert_eq!(
        restored["pullRequestUrl"],
        "https://forge.example/test-owner/helsinki_bikes/pulls/3"
    );

    let branches = posted(&gitea, "branches").await;
    assert_eq!(
        branches,
        vec![json!({ "new_branch_name": branch, "old_branch_name": "main" })]
    );
    let change = posted(&gitea, "contents").await;
    assert_eq!(change.len(), 1);
    let files: Vec<(&str, &str)> = change[0]["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| {
            (
                f["operation"].as_str().unwrap_or_default(),
                f["path"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        files,
        vec![("upload", "src/App.tsx"), ("delete", "src/New.tsx")]
    );
    assert_eq!(
        change[0]["files"][0]["content"],
        "ZXhwb3J0IGNvbnN0IG9sZCA9IDE7"
    );
    assert_eq!(change[0]["branch"], branch.as_str());
    let pulls = posted(&gitea, "pulls").await;
    assert_eq!(pulls.len(), 1);
    assert_eq!(pulls[0]["head"], branch.as_str());
    assert_eq!(pulls[0]["base"], "main");
    assert!(
        pulls[0]["title"].as_str().unwrap_or_default().contains(OLD),
        "{}",
        pulls[0]
    );
    assert_eq!(
        state
            .mirror
            .get("helsinki", "App", "bikes")
            .and_then(|app| serde_json::to_value(app).ok())
            .and_then(|app| app.pointer("/status/build/commit").cloned()),
        Some(json!(COMMIT)),
        "the Portal wrote status.build"
    );
}

/// AP-171: a reader is refused `403`; a commit that is no successful build of this App (a
/// failed run, another App's commit), the current build and an unknown field are refused, and
/// none of them reaches the forge's write routes.
#[tokio::test]
async fn restore_refuses_a_reader_a_foreign_or_failed_commit_and_the_current_build() {
    let gitea = forge_with_builds().await;
    let state = state_with(&gitea, true);
    const URI: &str = "/api/v1/projects/helsinki/apps/bikes/restore";

    let reader = send(
        &state,
        person("vera"),
        "POST",
        URI,
        Some(json!({ "commit": OLD })),
    )
    .await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN, "{}", reader.text);
    for (commit, why) in [
        (OTHER, "not a successful build"),
        ("deadbeef", "not a successful build"),
        (COMMIT, "serves now"),
    ] {
        let refused = send(
            &state,
            person("jana"),
            "POST",
            URI,
            Some(json!({ "commit": commit })),
        )
        .await;
        assert_eq!(
            refused.status,
            StatusCode::CONFLICT,
            "{commit}: {}",
            refused.text
        );
        assert!(refused.text.contains(why), "{commit}: {}", refused.text);
    }
    let unknown = send(
        &state,
        person("jana"),
        "POST",
        URI,
        Some(json!({ "commit": OLD, "digest": "sha256:00" })),
    )
    .await;
    assert!(unknown.status.is_client_error(), "{}", unknown.text);
    assert!(
        dispatches(&gitea).await.is_empty(),
        "a refused restore wrote to the forge"
    );
}

/// AP-171: an App not built on the forge lists no builds and Restore is refused `409`.
#[tokio::test]
async fn an_app_not_built_on_the_forge_has_no_builds_to_restore() {
    let gitea = MockServer::start().await;
    let state = state_with(&gitea, false);
    let listed = body(
        &send(
            &state,
            person("jana"),
            "GET",
            "/api/v1/projects/helsinki/apps/bikes/builds",
            None,
        )
        .await
        .text,
    );
    assert_eq!(listed["builds"], json!([]));
    assert_eq!(listed["restore"]["allowed"], false);
    let refused = send(
        &state,
        person("jana"),
        "POST",
        "/api/v1/projects/helsinki/apps/bikes/restore",
        Some(json!({ "commit": OLD })),
    )
    .await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    assert!(gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .is_empty());
}

/// AP-171: a changed file that is not UTF-8 text is refused `409` naming it, and a build whose
/// files equal the default branch has nothing to restore; neither opens a branch.
#[tokio::test]
async fn restore_refuses_a_binary_change_and_a_build_equal_to_the_default_branch() {
    const URI: &str = "/api/v1/projects/helsinki/apps/bikes/restore";
    let gitea = forge_with_builds().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/contents/src/App.tsx")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "sha": "old", "content": "/w==" })),
        )
        .with_priority(1)
        .mount(&gitea)
        .await;
    let state = state_with(&gitea, true);
    let binary = send(
        &state,
        person("jana"),
        "POST",
        URI,
        Some(json!({ "commit": OLD })),
    )
    .await;
    assert_eq!(binary.status, StatusCode::CONFLICT, "{}", binary.text);
    assert!(binary.text.contains("src/App.tsx"), "{}", binary.text);

    let gitea = forge_with_builds().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/git/trees/main")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "tree": [
            { "path": "index.html", "type": "blob", "sha": "a1" },
            { "path": "src/App.tsx", "type": "blob", "sha": "old" },
        ]})))
        .with_priority(1)
        .mount(&gitea)
        .await;
    let state = state_with(&gitea, true);
    let same = send(
        &state,
        person("jana"),
        "POST",
        URI,
        Some(json!({ "commit": OLD })),
    )
    .await;
    assert_eq!(same.status, StatusCode::CONFLICT, "{}", same.text);
    assert!(same.text.contains("already holds"), "{}", same.text);
    assert!(dispatches(&gitea).await.is_empty());
}

/// F-POR1-005: Rebuild is judged on the App itself. A grant that proposes only the App `trams`
/// reads `bikes` but does not rebuild or restore it.
#[tokio::test]
async fn a_grant_for_another_app_neither_rebuilds_nor_restores_this_one() {
    let gitea = forge_with_a_run().await;
    let state = state_with(&gitea, true);
    state.mirror.upsert(envelope(
        "Role",
        "trams-editor",
        ORG_NAMESPACE,
        json!({ "rules": [
            { "kinds": ["App"], "verbs": ["read"] },
            { "kinds": ["App"], "verbs": ["propose"],
              "constraints": [{ "field": "metadata.name", "in": ["trams"] }] },
        ] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "olga-trams-editor",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "olga@hel.fi" }],
            "role": "trams-editor",
            "scope": { "project": "helsinki" },
        }),
    ));

    let rebuild = send(
        &state,
        person("olga"),
        "POST",
        "/api/v1/projects/helsinki/apps/bikes/rebuild",
        None,
    )
    .await;
    assert_eq!(rebuild.status, StatusCode::FORBIDDEN, "{}", rebuild.text);
    assert!(
        dispatches(&gitea).await.is_empty(),
        "a build of bikes was dispatched"
    );
    let page = send(
        &state,
        person("olga"),
        "GET",
        "/api/v1/projects/helsinki/apps/bikes/build",
        None,
    )
    .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text);
    assert!(
        page.text.contains("propose on App"),
        "Rebuild is offered: {}",
        page.text
    );
}
