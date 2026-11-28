//! The workspace preview operations through every door of the registry (T-1532; AG-64, PF-50,
//! PF-59, CC-78, CC-81, API/01 §22).
//!
//! What was there before this file: the REST routes are tested in `workspace_preview_tests.rs`
//! (the render, one per workspace and two on the node, the owner only, the gateway's list) and the
//! functions under them in `edge_preview_start_tests.rs` and `edge_preview_apps_tests.rs`. No test
//! called `jc_workspace_preview_start`, `jc_workspace_preview_get` or `jc_workspace_preview_stop`
//! through the registry, as an MCP client or as the owner's agent run.

mod common;

use axum::http::StatusCode;
use common::doors::{self, mcp, run, session, steward, stranger, viewer, PROJECT};
use common::{encode, envelope, forge, state_on, REPO};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::ops::workspaces::{Opening, PreviewState, Scope};
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SLUG: &str = "zt4qm7ge2xdv6ksb3ncf5arw2y";
const HEAD: &str = "head1";
const PREVIEWS: [&str; 3] = [
    "jc_workspace_preview_start",
    "jc_workspace_preview_get",
    "jc_workspace_preview_stop",
];

fn files() -> Vec<(&'static str, String)> {
    let manifest = |kind: &str, name: &str, namespace: &str, spec: &str| {
        format!("apiVersion: joinedcontext.com/v1alpha1\nkind: {kind}\nmetadata:\n  name: {name}\n  namespace: {namespace}\nspec:\n{spec}")
    };
    vec![
        ("org.yaml", manifest("Organization", "bb", "org", "  domain: banskabystrica.sk\n  locales: [\"en\"]\n  defaultLocale: en\n")),
        ("projects/ovzdusie/project.yaml", manifest("Project", "ovzdusie", "org", "  organizationRef: bb\n")),
        ("projects/ovzdusie/spaces/air/space.yaml", manifest("ContextSpace", "air", "ovzdusie", "  isSandbox: false\n")),
        (
            "projects/ovzdusie/spaces/air/endpoints/public-air.yaml",
            manifest("Endpoint", "public-air", "ovzdusie", &format!("  slug: {SLUG}\n  contextSpaceRef: air\n  audience: public\n  enabledRepresentations: [ngsi-ld]\n")),
        ),
    ]
}

/// Another steward of the project, who owns no workspace.
fn petra() -> Identity {
    Identity {
        client: None,
        subject: "f:1:petra".into(),
        username: "petra".into(),
        email: Some("petra@banskabystrica.sk".into()),
        ..steward()
    }
}

async fn open(state: &AppState, name: &str, owner: &str) {
    state
        .workspaces
        .create(Opening {
            name,
            title: None,
            project: PROJECT,
            owner,
            base_revision: "base1",
            scope: Scope::Project {},
            ttl_hours: 24,
        })
        .await
        .expect("opened");
}

/// Jana's workspace `air-v2`, its branch on the forge.
async fn world() -> (MockServer, AppState) {
    let server = forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Branches('workspace%2Fair-v2')")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "HeadSha": HEAD })))
        .mount(&server)
        .await;
    let mut files = files();
    // Another project of the organization: the preview renders the workspace's project alone.
    files.push((
        "projects/elsewhere/project.yaml",
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata:\n  name: elsewhere\n  namespace: org\nspec:\n  organizationRef: bb\n".to_owned(),
    ));
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"))).and(query_param("@r", format!("'{HEAD}'")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Truncated": false,
            "Tree": files.iter().map(|(p, _)| json!({ "Path": p, "Type": "blob", "Sha": format!("s-{p}") })).collect::<Vec<_>>()
        })))
        .mount(&server)
        .await;
    for (file, content) in &files {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
            .and(query_param("@p", format!("'{file}'")))
            .and(query_param("@r", format!("'{}'", HEAD)))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Sha": "x",
                "Content": encode(content)
            })))
            .mount(&server)
            .await;
    }
    let state = doors::state_with(state_on(&server));
    state.mirror.upsert(envelope(
        "Endpoint",
        "public-air",
        PROJECT,
        json!({ "slug": SLUG, "contextSpaceRef": "air", "audience": "public" }),
    ));
    open(&state, "air-v2", "jana").await;
    (server, state)
}

fn named(name: &str) -> Value {
    json!({ "name": name })
}

async fn preview_state(state: &AppState, name: &str) -> PreviewState {
    state
        .workspaces
        .get(name)
        .await
        .expect("store")
        .expect("workspace")
        .preview_state
}

/// CC-78: the owner starts, reads and stops her workspace's preview through the session, her MCP
/// client and her agent run, whose profile names the three operations; the preview runs under the
/// workspace's prefix.
#[tokio::test]
async fn every_door_of_the_owner_starts_reads_and_stops_the_preview() {
    let (_server, state) = world().await;
    for (who, caller) in [
        ("session", session(steward())),
        ("mcp", mcp(steward())),
        ("her run", run(steward(), &PREVIEWS)),
    ] {
        let started = doors::call(
            "jc_workspace_preview_start",
            &caller,
            &state,
            named("air-v2"),
        )
        .await;
        assert_eq!(StatusCode::OK, started.status, "{who}: {}", started.text());
        assert_eq!(
            "running",
            started.body["state"],
            "{who}: {}",
            started.text()
        );
        assert_eq!("ws-air-v2-", started.body["prefix"], "{who}");

        let read = doors::call("jc_workspace_preview_get", &caller, &state, named("air-v2")).await;
        assert_eq!(StatusCode::OK, read.status, "{who}: {}", read.text());
        assert_eq!(started.body["endpoints"], read.body["endpoints"], "{who}");

        let stopped = doors::call(
            "jc_workspace_preview_stop",
            &caller,
            &state,
            named("air-v2"),
        )
        .await;
        assert_eq!(StatusCode::OK, stopped.status, "{who}: {}", stopped.text());
        assert_eq!(
            PreviewState::Stopped,
            preview_state(&state, "air-v2").await,
            "{who}"
        );
    }
}

/// API/01 §22, CC-78: one preview per workspace, and a second start is refused naming it; a stop of
/// a preview that does not run changes nothing.
#[tokio::test]
async fn one_preview_per_workspace_and_a_second_start_says_so() {
    let (_server, state) = world().await;
    let caller = mcp(steward());
    let first = doors::call(
        "jc_workspace_preview_start",
        &caller,
        &state,
        named("air-v2"),
    )
    .await;
    assert_eq!(StatusCode::OK, first.status, "{}", first.text());
    let again = doors::call(
        "jc_workspace_preview_start",
        &caller,
        &state,
        named("air-v2"),
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, again.status, "{}", again.text());
    assert!(again.text().contains("runs already"), "{}", again.text());

    for _ in 0..2 {
        let stopped = doors::call(
            "jc_workspace_preview_stop",
            &caller,
            &state,
            named("air-v2"),
        )
        .await;
        assert_eq!(StatusCode::OK, stopped.status, "{}", stopped.text());
    }
    assert_eq!(PreviewState::Stopped, preview_state(&state, "air-v2").await);
}

/// CC-81, PF-83: two previews run on the node; a third is refused with the node full, and the
/// refusal names the running previews this caller may see.
#[tokio::test]
async fn two_previews_on_the_node_and_a_third_is_refused() {
    let (_server, state) = world().await;
    for name in ["noise-v1", "water-v1"] {
        open(&state, name, "jana").await;
        state
            .workspaces
            .set_preview_state(name, PreviewState::Running)
            .await
            .expect("running");
    }
    let full = doors::call(
        "jc_workspace_preview_start",
        &run(steward(), &PREVIEWS),
        &state,
        named("air-v2"),
    )
    .await;
    assert_eq!(StatusCode::CONFLICT, full.status, "{}", full.text());
    assert!(full.text().contains("noise-v1"), "{}", full.text());
    assert_eq!(PreviewState::None, preview_state(&state, "air-v2").await);
}

/// API/01 §22, PF-59: only the owner starts or stops a preview: another steward is refused naming
/// the owner, a viewer is refused, and a person with no binding finds no workspace; each may read
/// the preview only where the workspace itself may be read.
#[tokio::test]
async fn only_the_owner_starts_or_stops_and_a_stranger_finds_nothing() {
    let (_server, state) = world().await;
    for (who, caller) in [
        ("colleague", session(petra())),
        ("colleague over mcp", mcp(petra())),
        ("colleague's run", run(petra(), &PREVIEWS)),
        ("viewer", session(viewer())),
    ] {
        for name in ["jc_workspace_preview_start", "jc_workspace_preview_stop"] {
            let refused = doors::call(name, &caller, &state, named("air-v2")).await;
            assert_eq!(
                StatusCode::FORBIDDEN,
                refused.status,
                "{who} {name}: {}",
                refused.text()
            );
            assert!(
                refused.text().contains("only its owner"),
                "{who} {name}: {}",
                refused.text()
            );
        }
        let read = doors::call("jc_workspace_preview_get", &caller, &state, named("air-v2")).await;
        assert_eq!(StatusCode::OK, read.status, "{who}: {}", read.text());
    }
    for caller in [session(stranger()), mcp(stranger())] {
        let hidden =
            doors::call("jc_workspace_preview_get", &caller, &state, named("air-v2")).await;
        assert_eq!(StatusCode::NOT_FOUND, hidden.status, "{}", hidden.text());
        let refused = doors::call(
            "jc_workspace_preview_start",
            &caller,
            &state,
            named("air-v2"),
        )
        .await;
        assert!(refused.status.is_client_error(), "{}", refused.text());
        assert!(
            !refused.text().contains("jana"),
            "the owner leaked: {}",
            refused.text()
        );
    }
    let unknown = doors::call(
        "jc_workspace_preview_start",
        &session(steward()),
        &state,
        named("nope"),
    )
    .await;
    assert_eq!(StatusCode::NOT_FOUND, unknown.status, "{}", unknown.text());
    assert_eq!(PreviewState::None, preview_state(&state, "air-v2").await);
}

/// AG-70: a run whose profile does not name the preview operations is refused them and not offered
/// them.
#[tokio::test]
async fn a_run_whose_profile_does_not_name_them_is_refused_them() {
    let (_server, state) = world().await;
    let bare = run(steward(), &["jc_workspace_list"]);
    let offered = doors::offered(&bare, &state);
    for name in PREVIEWS {
        assert!(!offered.contains(&name.to_owned()), "{name} offered");
        let refused = doors::call(name, &bare, &state, named("air-v2")).await;
        assert_eq!(
            StatusCode::FORBIDDEN,
            refused.status,
            "{name}: {}",
            refused.text()
        );
    }
    assert_eq!(PreviewState::None, preview_state(&state, "air-v2").await);
}

/// AG-64: the REST route and `POST …/ops/{name}` refuse each person alike.
#[tokio::test]
async fn the_rest_route_and_the_ops_door_refuse_alike() {
    let (_server, state) = world().await;
    let uri = "/api/v1/projects/ovzdusie/workspaces/air-v2/preview";
    for identity in [petra(), viewer()] {
        for (name, rest_method) in [
            ("jc_workspace_preview_start", "POST"),
            ("jc_workspace_preview_stop", "DELETE"),
        ] {
            let rest = doors::http(&state, identity.clone(), rest_method, uri, None).await;
            let door = doors::post_op(&state, identity.clone(), name, named("air-v2")).await;
            assert_eq!(
                StatusCode::FORBIDDEN,
                rest.status,
                "{name}: {}",
                rest.text()
            );
            assert_eq!(
                rest.status,
                door.status,
                "{} {name}: {} vs {}",
                identity.username,
                rest.text(),
                door.text()
            );
        }
    }
    let rest = doors::http(&state, stranger(), "GET", uri, None).await;
    let door = doors::post_op(
        &state,
        stranger(),
        "jc_workspace_preview_get",
        named("air-v2"),
    )
    .await;
    assert_eq!(StatusCode::NOT_FOUND, rest.status, "{}", rest.text());
    assert_eq!(
        rest.status,
        door.status,
        "{} vs {}",
        rest.text(),
        door.text()
    );
}

/// The project repository of layout 2 the mirror names for the project (CC-85).
const PROJECT_REPO: &str = "/odata/Repositories(Owner='test-owner',Name='ovzdusie-data')";

/// `repo`'s tree at `head` holding `files`, each file readable at that ref.
async fn mount_tree(server: &MockServer, repo: &str, head: &str, files: &[(&str, String)]) {
    Mock::given(method("GET"))
        .and(path(format!("{repo}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)")))
        .and(query_param("@r", format!("'{head}'")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Truncated": false,
            "Tree": files.iter().map(|(p, _)| json!({ "Path": p, "Type": "blob", "Sha": format!("s-{head}-{p}") })).collect::<Vec<_>>()
        })))
        .mount(server)
        .await;
    for (file, content) in files {
        Mock::given(method("GET"))
            .and(path(format!("{repo}/Shako.File(Path=@p,Ref=@r)")))
            .and(query_param("@p", format!("'{file}'")))
            .and(query_param("@r", format!("'{head}'")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Sha": "x",
                "Content": encode(content)
            })))
            .mount(server)
            .await;
    }
}

/// Jana's workspace `air-v2` of a project with a repository of its own: the organization at
/// its main head, the project's branch in the project repository.
async fn layout_two(project_tree: bool) -> (MockServer, AppState) {
    layout_two_with(project_tree, true).await
}

/// [`layout_two`] whose project branch holds `project.yaml` only when `project_file`.
async fn layout_two_with(project_tree: bool, project_file: bool) -> (MockServer, AppState) {
    let server = forge().await;
    common::mount_repository(&server, PROJECT_REPO).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{PROJECT_REPO}/Branches('workspace%2Fair-v2')"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "HeadSha": HEAD })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Branches('main')")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "HeadSha": "main1" })))
        .mount(&server)
        .await;
    let all = files();
    let layout = ("2\n").to_owned();
    let entry = all[1].1.replace(
        "  organizationRef: bb\n",
        "  organizationRef: bb\n  repository:\n    name: ovzdusie-data\n  ref: main\n",
    );
    let organization = vec![
        (".jc/layout", layout.clone()),
        (all[0].0, all[0].1.clone()),
        ("projects/ovzdusie.yaml", entry),
    ];
    mount_tree(&server, REPO, "main1", &organization).await;
    let mut inside: Vec<(&str, String)> = vec![(".jc/layout", layout)];
    if project_file {
        inside.push(("project.yaml", all[1].1.clone()));
    }
    inside.extend(
        all[2..]
            .iter()
            .map(|(p, c)| (p.trim_start_matches("projects/ovzdusie/"), c.clone())),
    );
    if project_tree {
        mount_tree(&server, PROJECT_REPO, HEAD, &inside).await;
    } else {
        Mock::given(method("GET"))
            .and(path(format!(
                "{PROJECT_REPO}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
            )))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
    }
    let state = doors::state_with(state_on(&server));
    state
        .mirror
        .set_repositories(std::collections::BTreeMap::from([(
            PROJECT.to_owned(),
            "ovzdusie-data".to_owned(),
        )]));
    open(&state, "air-v2", "jana").await;
    (server, state)
}

/// CC-85: a preview of a project with its own repository assembles the organization at main
/// with the project's branch; the answer is the render or the assembly's refusal, never a
/// stage of the wrong repository.
#[tokio::test]
async fn a_preview_of_a_project_repository_assembles_it_with_the_organization() {
    let (server, state) = layout_two(true).await;
    let started = doors::call(
        "jc_workspace_preview_start",
        &session(steward()),
        &state,
        named("air-v2"),
    )
    .await;
    assert_eq!(StatusCode::OK, started.status, "{}", started.text());
    assert_eq!("running", started.body["state"], "{}", started.text());
    let asked: Vec<String> = server
        .received_requests()
        .await
        .expect("requests")
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    assert!(
        asked
            .iter()
            .any(|p| p.starts_with(PROJECT_REPO) && p.contains("Shako.Tree")),
        "the project repository is staged: {asked:?}"
    );
}

/// A project repository the forge cannot list is a 503 saying the branch did not stage.
#[tokio::test]
async fn a_project_repository_that_does_not_stage_is_unavailable() {
    let (_server, state) = layout_two(false).await;
    let started = doors::call(
        "jc_workspace_preview_start",
        &session(steward()),
        &state,
        named("air-v2"),
    )
    .await;
    assert_eq!(
        StatusCode::SERVICE_UNAVAILABLE,
        started.status,
        "{}",
        started.text()
    );
    assert!(
        started.text().contains("did not stage"),
        "{}",
        started.text()
    );
}

/// A project repository whose branch holds no project file stages but does not assemble with
/// the organization: a 503 saying so.
#[tokio::test]
async fn a_project_branch_without_its_project_file_does_not_assemble() {
    let (_server, state) = layout_two_with(true, false).await;
    let started = doors::call(
        "jc_workspace_preview_start",
        &session(steward()),
        &state,
        named("air-v2"),
    )
    .await;
    assert_eq!(
        StatusCode::SERVICE_UNAVAILABLE,
        started.status,
        "{}",
        started.text()
    );
    assert!(
        started.text().contains("does not assemble"),
        "{}",
        started.text()
    );
}

/// A forge that fails reading the workspace's branch is the forge's error, not a missing branch.
#[tokio::test]
async fn a_forge_failing_the_branch_read_is_not_a_missing_branch() {
    let server = forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Branches('workspace%2Fair-v2')")))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let state = doors::state_with(state_on(&server));
    open(&state, "air-v2", "jana").await;
    let started = doors::call(
        "jc_workspace_preview_start",
        &session(steward()),
        &state,
        named("air-v2"),
    )
    .await;
    assert_ne!(StatusCode::OK, started.status, "{}", started.text());
    assert_ne!(StatusCode::NOT_FOUND, started.status, "{}", started.text());
}

/// CC-81: the gateway's list holds a running preview that renders, and leaves out one whose
/// branch went away since it started, without failing the list.
#[tokio::test]
async fn the_served_list_leaves_out_a_preview_that_no_longer_renders() {
    let (server, state) = world().await;
    let started = doors::call(
        "jc_workspace_preview_start",
        &session(steward()),
        &state,
        named("air-v2"),
    )
    .await;
    assert_eq!(StatusCode::OK, started.status, "{}", started.text());
    let listed = joinedcontext_portal::ops::previews::served(&state)
        .await
        .expect("listed");
    assert_eq!(listed.items.len(), 1);
    server.reset().await;
    let listed = joinedcontext_portal::ops::previews::served(&state)
        .await
        .expect("listed");
    assert!(
        listed.items.is_empty(),
        "a branch that is gone serves nothing"
    );
}

/// A branch of the organization's repository that the forge cannot list is a 503 saying the
/// branch did not stage (T-7921).
#[tokio::test]
async fn a_branch_that_does_not_stage_is_unavailable() {
    let server = forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Branches('workspace%2Fair-v2')")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "HeadSha": HEAD })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{REPO}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
        )))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let state = doors::state_with(state_on(&server));
    open(&state, "air-v2", "jana").await;
    let started = doors::call(
        "jc_workspace_preview_start",
        &session(steward()),
        &state,
        named("air-v2"),
    )
    .await;
    assert_eq!(
        StatusCode::SERVICE_UNAVAILABLE,
        started.status,
        "{}",
        started.text()
    );
    assert!(
        started.text().contains("did not stage"),
        "{}",
        started.text()
    );
}

/// CC-81: a running preview whose branch moved to files that no longer load is left out of the
/// gateway's list, and the list still answers (T-7921).
#[tokio::test]
async fn the_served_list_leaves_out_a_preview_whose_files_no_longer_load() {
    let (server, state) = world().await;
    let started = doors::call(
        "jc_workspace_preview_start",
        &session(steward()),
        &state,
        named("air-v2"),
    )
    .await;
    assert_eq!(StatusCode::OK, started.status, "{}", started.text());
    server.reset().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Branches('workspace%2Fair-v2')")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "HeadSha": "head2" })))
        .mount(&server)
        .await;
    let mut broken = files();
    // The same ContextSpace twice: the repository does not load.
    let twice = broken[2].1.clone();
    broken.push(("projects/ovzdusie/spaces/air/again.yaml", twice));
    mount_tree(&server, REPO, "head2", &broken).await;
    let listed = joinedcontext_portal::ops::previews::served(&state)
        .await
        .expect("listed");
    assert!(
        listed.items.is_empty(),
        "files that do not load serve nothing: {:?}",
        listed
            .items
            .iter()
            .map(|item| item.files.keys().cloned().collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
}

/// A branch that holds no `projects/` directory renders as it is, with nothing to leave out;
/// a blank `JC_ENVIRONMENT` renders with no overlay (T-7931).
#[tokio::test]
async fn a_branch_without_projects_renders_with_nothing_left_out() {
    // Blank is what an unset overlay reads as, so no other test sees a change.
    std::env::set_var("JC_ENVIRONMENT", " ");
    let server = forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Branches('workspace%2Fair-v2')")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "HeadSha": HEAD })))
        .mount(&server)
        .await;
    let all = files();
    mount_tree(&server, REPO, HEAD, &all[..1]).await;
    let state = doors::state_with(state_on(&server));
    open(&state, "air-v2", "jana").await;
    let started = doors::call(
        "jc_workspace_preview_start",
        &session(steward()),
        &state,
        named("air-v2"),
    )
    .await;
    assert_ne!(
        StatusCode::SERVICE_UNAVAILABLE,
        started.status,
        "{}",
        started.text()
    );
    assert!(
        started.body["endpoints"]
            .as_array()
            .is_none_or(|endpoints| endpoints.is_empty()),
        "{}",
        started.text()
    );
}
