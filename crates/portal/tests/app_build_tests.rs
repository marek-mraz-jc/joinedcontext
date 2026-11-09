//! The App page's build: the repository, the newest build and the package, and Rebuild as a
//! build the Portal queues itself (AP-103, ADR-N-028, T-2609, jc build lane). The builds come
//! from the Portal's own record; the tests that read it skip without
//! `JC_PORTAL_TEST_DATABASE_URL`.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::build_lane::Work;
use common::{checked_send as send, envelope, person};
use joinedcontext_portal::apps::build_pods::Class;
use joinedcontext_portal::apps::build_records::{self, Trigger};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ResourceEnvelope, API_VERSION};
use joinedcontext_portal::state::AppState;

/// `common::state_on` names the organization `test-owner`; the App's repository is `{project}_{app}`.
const REPO: &str = "/odata/Repositories(Owner='test-owner',Name='helsinki_bikes')";
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
fn state_with(shako: &MockServer, git: bool) -> AppState {
    let state = common::state_on(shako);
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

/// [`state_with`] on a Portal that builds Apps, its builds in `db`.
fn lane_state(shako: &MockServer, git: bool, db: &sqlx::PgPool, work: &Work) -> AppState {
    let state = state_with(shako, git);
    let mut config = (*state.config).clone();
    config.build_pods = Some(common::build_lane::settings(work.path()));
    let mut lane = state.with_db(db.clone());
    lane.config = std::sync::Arc::new(config);
    lane
}

/// A build of bikes at `commit` that ended `Ok(digest)` or `Err(reason)`, started at `started`
/// and finished `seconds` later; its id.
async fn recorded(
    db: &sqlx::PgPool,
    commit: &str,
    outcome: Result<&str, &str>,
    started: &str,
    seconds: i64,
) -> i64 {
    let id = build_records::enqueue(
        db,
        "helsinki",
        "bikes",
        commit,
        Class::Node,
        Trigger::Push,
        None,
    )
    .await
    .expect("queued");
    build_records::start(db, id, "t", chrono::Duration::minutes(25))
        .await
        .expect("started");
    build_records::finish(db, id, outcome).await.expect("ended");
    sqlx::query(
        "UPDATE app_builds SET started_at = $2::timestamptz, \
         finished_at = $2::timestamptz + make_interval(secs => $3) WHERE id = $1",
    )
    .bind(id)
    .bind(started)
    .bind(seconds as f64)
    .execute(db)
    .await
    .expect("times");
    id
}

async fn forge_with_a_run() -> MockServer {
    let shako = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/WorkflowRuns")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                {
                    "Number": 7,
                    "Status": "completed",
                    "Conclusion": "success",
                    "HeadSha": COMMIT
                }
            ]
        })))
        .mount(&shako)
        .await;
    Mock::given(method("GET"))
        .and(path(REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "DefaultBranch": "main" })))
        .mount(&shako)
        .await;
    shako
}

fn body(text: &str) -> Value {
    serde_json::from_str(text).expect("a JSON answer")
}

async fn dispatches(shako: &MockServer) -> Vec<String> {
    shako
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST")
        .map(|r| r.url.path().to_owned())
        .collect()
}

/// AP-103, PF-81: the page links the repository behind the forge's sign-in, the newest build
/// with its log, and the package of the build, and offers Rebuild only to a person who may
/// propose an App.
#[tokio::test]
async fn the_build_links_the_repository_the_newest_build_and_the_package() {
    let db = db_or_skip!();
    let work = Work::new();
    let shako = forge_with_a_run().await;
    let state = lane_state(&shako, true, &db, &work);
    let id = recorded(&db, COMMIT, Ok("sha256:ab"), "2026-10-07T08:00:00Z", 185).await;
    const URI: &str = "/api/v1/projects/helsinki/apps/bikes/build";

    let answer = send(&state, person("jana"), "GET", URI, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let build = body(&answer.text);
    let login = |target: &str| {
        format!(
            "{}/user/oauth2/keycloak?redirect_to={}",
            shako.uri(),
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
            "url": format!(
                "{}/api/v1/projects/helsinki/apps/bikes/builds/{id}/log",
                state.config.public_base_url.as_str().trim_end_matches('/')
            ),
            "number": id,
            "startedAt": "2026-10-07T08:00:00Z",
            "completedAt": "2026-10-07T08:03:05Z",
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

/// AP-103: a build's log is the App's readers' to read; a stranger and a build of another App
/// get the same 404.
#[tokio::test]
async fn a_builds_log_is_read_by_the_apps_readers_only() {
    let db = db_or_skip!();
    let work = Work::new();
    let shako = forge_with_a_run().await;
    let state = lane_state(&shako, true, &db, &work);
    let id = recorded(
        &db,
        COMMIT,
        Err("vitest: 2 failed"),
        "2026-10-07T08:00:00Z",
        20,
    )
    .await;
    let other = build_records::enqueue(
        &db,
        "helsinki",
        "trams",
        COMMIT,
        Class::Node,
        Trigger::Push,
        None,
    )
    .await
    .expect("queued");
    let uri = |id: i64| format!("/api/v1/projects/helsinki/apps/bikes/builds/{id}/log");

    let read = send(&state, person("vera"), "GET", &uri(id), None).await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    assert!(
        read.text.contains("failed: vitest: 2 failed"),
        "{}",
        read.text
    );
    let stranger = send(&state, person("otto"), "GET", &uri(id), None).await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND);
    let foreign = send(&state, person("vera"), "GET", &uri(other), None).await;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND, "{}", foreign.text);
}

/// The repository's head and the template's build workflow at it.
async fn head_with_workflow(shako: &MockServer, workflow: Option<&str>) {
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Branches('main')")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "HeadSha": COMMIT })))
        .mount(shako)
        .await;
    use base64::Engine;
    let file = match workflow {
        Some(text) => ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "f1",
            "Content": base64::engine::general_purpose::STANDARD.encode(text),
        })),
        None => ResponseTemplate::new(404),
    };
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param("@p", "'.gitea/workflows/build.yml'"))
        .and(query_param("@r", format!("'{COMMIT}'")))
        .respond_with(file)
        .mount(shako)
        .await;
}

/// AP-103: Rebuild queues a build of the head of the App's own repository, of the class its
/// workflow names, whatever repository the manifest names; a reader is refused and nothing is
/// queued; a second Rebuild keeps one build waiting.
#[tokio::test]
async fn rebuild_queues_a_build_of_the_head_of_the_apps_own_repository() {
    let db = db_or_skip!();
    let work = Work::new();
    let shako = forge_with_a_run().await;
    head_with_workflow(
        &shako,
        Some("jobs:\n  build:\n    runs-on: app-build-rust\n"),
    )
    .await;
    let state = lane_state(&shako, true, &db, &work);
    const URI: &str = "/api/v1/projects/helsinki/apps/bikes/rebuild";

    let refused = send(&state, person("vera"), "POST", URI, None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(
        build_records::next_pending(&db)
            .await
            .expect("read")
            .is_none(),
        "a reader queued a build"
    );

    for _ in 0..2 {
        let accepted = send(&state, person("jana"), "POST", URI, None).await;
        assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    }
    let builds = build_records::latest(&db, "helsinki", "bikes", 10)
        .await
        .expect("read");
    assert_eq!(builds.len(), 1, "one build waits per App");
    let build = &builds[0];
    assert_eq!(
        (
            build.commit.as_str(),
            build.class(),
            build.trigger.as_str(),
            build.requested_by.as_deref(),
            build.state.as_str()
        ),
        (COMMIT, Class::Rust, "rebuild", Some("jana"), "pending")
    );
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
    assert_eq!(page["run"]["status"], "queued");
}

/// AP-100: a repository whose head carries no App build workflow has nothing to build: 409 with
/// the reason, and nothing queued.
#[tokio::test]
async fn a_head_without_the_build_workflow_is_not_rebuilt() {
    let db = db_or_skip!();
    let work = Work::new();
    let shako = forge_with_a_run().await;
    head_with_workflow(&shako, None).await;
    let state = lane_state(&shako, true, &db, &work);
    let refused = send(
        &state,
        person("jana"),
        "POST",
        "/api/v1/projects/helsinki/apps/bikes/rebuild",
        None,
    )
    .await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    assert!(
        refused.text.contains(".gitea/workflows/build.yml"),
        "{}",
        refused.text
    );
    assert!(build_records::latest(&db, "helsinki", "bikes", 10)
        .await
        .expect("read")
        .is_empty());
}

/// AP-100, AP-103: an App that names no repository of its own has no build on the forge; the
/// page says so and Rebuild is refused `409` with the reason, and nothing reaches the forge.
#[tokio::test]
async fn an_app_not_built_on_the_forge_has_no_links_and_no_rebuild() {
    let shako = MockServer::start().await;
    let state = state_with(&shako, false);

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
    assert!(shako
        .received_requests()
        .await
        .unwrap_or_default()
        .is_empty());
}

/// AP-103: on a Portal that builds nothing, Rebuild is a `503` that says so, nothing reaches the
/// forge, and the page shows no build rather than an error.
#[tokio::test]
async fn without_builds_configured_rebuild_says_so_and_the_page_has_no_build() {
    let shako = MockServer::start().await;
    let state = state_with(&shako, true);

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
        refused.text.contains("JC_PORTAL_BUILD_NAMESPACE"),
        "{}",
        refused.text
    );
    assert!(shako
        .received_requests()
        .await
        .unwrap_or_default()
        .is_empty());
}

const BUILD: &str = "/api/v1/projects/helsinki/apps/bikes/build";

fn login(shako: &MockServer, target: &str) -> Value {
    json!(format!(
        "{}/user/oauth2/keycloak?redirect_to={target}",
        shako.uri()
    ))
}

/// T-3039, PF-87: in layout 1 the configuration repository is the organization's, which the
/// forge reads to a person bound at the organization and to nobody bound only to the project;
/// the App's own repository is read by every signed-in person who may read the App (T-3030).
#[tokio::test]
async fn in_layout_1_the_configuration_link_is_for_a_person_bound_at_the_organization() {
    let shako = forge_with_a_run().await;
    let state = state_with(&shako, true);
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
        login(&shako, "%2Ftest-owner%2Fhelsinki_bikes")
    );

    let olga = body(&send(&state, person("olga"), "GET", BUILD, None).await.text);
    assert_eq!(
        olga["configurationUrl"],
        login(&shako, "%2Ftest-owner%2Ftest-repo")
    );
    assert_eq!(
        olga["repositoryUrl"],
        login(&shako, "%2Ftest-owner%2Fhelsinki_bikes")
    );
}

/// T-3039, PF-87, CC-87: in layout 2 the configuration link is the project's own repository,
/// for every person the project's bindings place in its readers, a read-only one included; the
/// address follows the platform's record of the repository, so a renamed one moves with it.
#[tokio::test]
async fn in_layout_2_the_configuration_link_is_the_projects_own_repository_for_its_readers() {
    let shako = forge_with_a_run().await;
    let state = state_with(&shako, true);
    state.mirror.set_layout(2);
    state
        .mirror
        .set_repositories([("helsinki".to_owned(), "helsinki".to_owned())].into());

    for who in ["jana", "vera"] {
        let build = body(&send(&state, person(who), "GET", BUILD, None).await.text);
        assert_eq!(
            build["configurationUrl"],
            login(&shako, "%2Ftest-owner%2Fhelsinki"),
            "{who}"
        );
    }

    state
        .mirror
        .set_repositories([("helsinki".to_owned(), "helsinki-city".to_owned())].into());
    let renamed = body(&send(&state, person("vera"), "GET", BUILD, None).await.text);
    assert_eq!(
        renamed["configurationUrl"],
        login(&shako, "%2Ftest-owner%2Fhelsinki-city")
    );
}

/// T-3245: a running build is shown against the newest finished successful build, and the run
/// carries its number and times, so a client tells the build it started from the one before.
#[tokio::test]
async fn a_build_carries_its_number_and_times_and_the_estimate_of_the_last_success() {
    let db = db_or_skip!();
    let work = Work::new();
    let shako = MockServer::start().await;
    let state = lane_state(&shako, true, &db, &work);
    recorded(&db, COMMIT, Ok("sha256:ab"), "2026-10-07T08:00:00Z", 185).await;
    recorded(&db, COMMIT, Err("tests failed"), "2026-10-07T09:00:00Z", 20).await;
    let running = build_records::enqueue(
        &db,
        "helsinki",
        "bikes",
        COMMIT,
        Class::Node,
        Trigger::Rebuild,
        Some("jana"),
    )
    .await
    .expect("queued");
    build_records::start(&db, running, "t", chrono::Duration::minutes(25))
        .await
        .expect("started");
    sqlx::query("UPDATE app_builds SET started_at = '2026-10-07T10:00:00Z' WHERE id = $1")
        .bind(running)
        .execute(&db)
        .await
        .expect("time");
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
    assert_eq!(build["run"]["number"], json!(running));
    assert_eq!(build["run"]["status"], json!("in_progress"));
    assert_eq!(build["run"]["startedAt"], json!("2026-10-07T10:00:00Z"));
    assert!(build["run"].get("completedAt").is_none(), "{build}");
    // The failed build is no estimate; the last success took 3 min 5 s.
    assert_eq!(build["typicalSeconds"], json!(185), "{build}");
}

/// No successful build yet is no estimate rather than a guess.
#[tokio::test]
async fn without_a_finished_success_there_is_no_estimate() {
    let db = db_or_skip!();
    let work = Work::new();
    let shako = MockServer::start().await;
    let state = lane_state(&shako, true, &db, &work);
    let queued = build_records::enqueue(
        &db,
        "helsinki",
        "bikes",
        COMMIT,
        Class::Node,
        Trigger::Push,
        None,
    )
    .await
    .expect("queued");
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
    assert_eq!(build["run"]["number"], json!(queued));
}

const OLD: &str = "9a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";
const OTHER: &str = "1111111111111111111111111111111111111111";

/// The current build, an earlier one built twice and a failed build of another commit, in the
/// Portal's record; their ids.
async fn builds_recorded(db: &sqlx::PgPool) -> (i64, i64) {
    recorded(db, OLD, Ok("sha256:01"), "2026-10-05T08:00:00Z", 60).await;
    let old = recorded(db, OLD, Ok("sha256:02"), "2026-10-05T08:10:00Z", 60).await;
    let current = recorded(db, COMMIT, Ok("sha256:ab"), "2026-10-05T08:20:00Z", 60).await;
    recorded(db, OTHER, Err("tests failed"), "2026-10-05T08:30:00Z", 60).await;
    (current, old)
}

/// A forge with the two trees and the forge's write routes.
async fn forge_with_builds() -> MockServer {
    let shako = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "DefaultBranch": "main" })))
        .mount(&shako)
        .await;
    let tree =
        |entries: Value| ResponseTemplate::new(200).set_body_json(json!({ "Tree": entries }));
    Mock::given(method("GET"))
        .and(path(format!(
            "{REPO}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
        )))
        .and(query_param("@r", format!("'{OLD}'")))
        .respond_with(tree(json!([
            { "Path": "index.html", "Type": "blob", "Sha": "a1" },
            { "Path": "src/App.tsx", "Type": "blob", "Sha": "old" },
            { "Path": "src", "Type": "tree", "Sha": "t1" },
        ])))
        .mount(&shako)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{REPO}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
        )))
        .and(query_param("@r", "'main'"))
        .respond_with(tree(json!([
            { "Path": "index.html", "Type": "blob", "Sha": "a1" },
            { "Path": "src/App.tsx", "Type": "blob", "Sha": "new" },
            { "Path": "src/New.tsx", "Type": "blob", "Sha": "n1" },
        ])))
        .mount(&shako)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param("@p", "'src/App.tsx'"))
        .and(query_param("@r", format!("'{OLD}'")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "old", "Content": "ZXhwb3J0IGNvbnN0IG9sZCA9IDE7", // "export const old = 1;"
        })))
        .mount(&shako)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/Shako.CreateBranch")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&shako)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/Shako.CommitFiles")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "Sha": "c0ffee" })))
        .mount(&shako)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/Shako.OpenPullRequest")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "Number": 3,
            "HtmlUrl": "https://forge.example/test-owner/helsinki_bikes/pulls/3",
            "State": "open",
            "Title": "restore",
            "Body": "",
            "Head": "restore",
            "Base": "main",
            "CreatedAt": "2026-10-10T21:00:00Z",
            "Author": "jana",
            "Mergeable": true,
            "Merged": false
        })))
        .mount(&shako)
        .await;
    shako
}

async fn posted(shako: &MockServer, route: &str) -> Vec<Value> {
    shako
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == format!("{REPO}/{route}"))
        .map(|r| serde_json::from_slice(&r.body).expect("a JSON body"))
        .collect()
}

/// AP-171: the builds are the successful builds, one per commit, newest first, the current one
/// marked, and Restore is offered to a person who may propose an App, never to a reader.
#[tokio::test]
async fn the_builds_are_the_successful_builds_once_per_commit_and_restore_needs_propose() {
    let db = db_or_skip!();
    let work = Work::new();
    let (_, old_id) = builds_recorded(&db).await;
    let shako = forge_with_builds().await;
    let state = lane_state(&shako, true, &db, &work);
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
    assert_eq!(builds["builds"][1]["number"], old_id);
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
    let db = db_or_skip!();
    let work = Work::new();
    builds_recorded(&db).await;
    let shako = forge_with_builds().await;
    let state = lane_state(&shako, true, &db, &work);
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

    let branches = posted(&shako, "Shako.CreateBranch").await;
    assert_eq!(
        branches,
        vec![json!({ "NewBranchName": branch, "OldBranchName": "main" })]
    );
    let change = posted(&shako, "Shako.CommitFiles").await;
    assert_eq!(change.len(), 1);
    let files: Vec<(&str, &str)> = change[0]["Files"]
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
        change[0]["Files"][0]["content"],
        "ZXhwb3J0IGNvbnN0IG9sZCA9IDE7"
    );
    assert_eq!(change[0]["Branch"], branch.as_str());
    let pulls = posted(&shako, "Shako.OpenPullRequest").await;
    assert_eq!(pulls.len(), 1);
    assert_eq!(pulls[0]["Head"], branch.as_str());
    assert_eq!(pulls[0]["Base"], "main");
    assert!(
        pulls[0]["Title"].as_str().unwrap_or_default().contains(OLD),
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
    let db = db_or_skip!();
    let work = Work::new();
    builds_recorded(&db).await;
    let shako = forge_with_builds().await;
    let state = lane_state(&shako, true, &db, &work);
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
        dispatches(&shako).await.is_empty(),
        "a refused restore wrote to the forge"
    );
}

/// AP-171: an App not built on the forge lists no builds and Restore is refused `409`.
#[tokio::test]
async fn an_app_not_built_on_the_forge_has_no_builds_to_restore() {
    let shako = MockServer::start().await;
    let state = state_with(&shako, false);
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
    assert!(shako
        .received_requests()
        .await
        .unwrap_or_default()
        .is_empty());
}

/// AP-171: a changed file that is not UTF-8 text is refused `409` naming it, and a build whose
/// files equal the default branch has nothing to restore; neither opens a branch.
#[tokio::test]
async fn restore_refuses_a_binary_change_and_a_build_equal_to_the_default_branch() {
    let db = db_or_skip!();
    let work = Work::new();
    builds_recorded(&db).await;
    const URI: &str = "/api/v1/projects/helsinki/apps/bikes/restore";
    let shako = forge_with_builds().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param("@p", "'src/App.tsx'"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "Sha": "old", "Content": "/w==" })),
        )
        .with_priority(1)
        .mount(&shako)
        .await;
    let state = lane_state(&shako, true, &db, &work);
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

    let shako = forge_with_builds().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{REPO}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
        )))
        .and(query_param("@r", "'main'"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Tree": [
                { "Path": "index.html", "Type": "blob", "Sha": "a1" },
                { "Path": "src/App.tsx", "Type": "blob", "Sha": "old" }
            ]
        })))
        .with_priority(1)
        .mount(&shako)
        .await;
    let state = lane_state(&shako, true, &db, &work);
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
    assert!(dispatches(&shako).await.is_empty());
}

/// F-POR1-005: Rebuild is judged on the App itself. A grant that proposes only the App `trams`
/// reads `bikes` but does not rebuild or restore it.
#[tokio::test]
async fn a_grant_for_another_app_neither_rebuilds_nor_restores_this_one() {
    let shako = forge_with_a_run().await;
    let state = state_with(&shako, true);
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
        dispatches(&shako).await.is_empty(),
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

/// AP-171: a build carried over from the forge's runs (status.build names it, the Portal kept no
/// record of it) is still listed as the current one, in its place by time.
#[tokio::test]
async fn the_current_build_is_listed_even_when_built_before_the_portal_kept_records() {
    let db = db_or_skip!();
    let work = Work::new();
    let shako = MockServer::start().await;
    let state = lane_state(&shako, true, &db, &work);
    recorded(&db, OLD, Ok("sha256:01"), "2026-09-01T08:00:00Z", 60).await;
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
    assert_eq!(
        listed["builds"],
        json!([
            { "commit": COMMIT, "completedAt": "2026-09-22T06:00:00Z", "current": true },
            { "commit": OLD, "number": listed["builds"][1]["number"], "completedAt": "2026-09-01T08:01:00Z", "current": false },
        ])
    );
}
