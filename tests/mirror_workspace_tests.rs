//! Inside a workspace the Portal reads its branch laid over `main` (CC-76, CC-77; T-1234).

use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::ops::workspaces::{mirror_of, Opening, Scope};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::state::AppState;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPO: &str = "/api/v1/repos/test-owner/test-repo";

fn pipeline(name: &str, period: &str) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Pipeline\nmetadata:\n  name: {name}\n  namespace: helsinki\nspec:\n  period: {period}\n"
    )
}

fn envelope(name: &str, period: &str) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Pipeline".into(),
        metadata: ObjectMeta::new(name, "helsinki"),
        spec: json!({ "period": period }),
        status: None,
    }
}

async fn tree(server: &MockServer, git_ref: &str, entries: &[(&str, &str)]) {
    let tree: Vec<_> = entries
        .iter()
        .map(|(p, sha)| json!({ "path": p, "type": "blob", "sha": sha }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/git/trees/{git_ref}")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "tree": tree, "truncated": false })),
        )
        .mount(server)
        .await;
}

/// The head commit of a branch, as `GET /branches/{branch}` answers it.
async fn branch(server: &MockServer, name: &str, head: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/branches/{name}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": head } })))
        .mount(server)
        .await;
}

async fn file(server: &MockServer, git_ref: &str, file_path: &str, content: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/contents/{file_path}")))
        .and(query_param("ref", git_ref))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "path": file_path, "sha": "x", "encoding": "base64", "content": STANDARD.encode(content)
        })))
        .mount(server)
        .await;
}

/// `main` holds `a` (60s), `b` and `gone`; the workspace changed `a` to 30s, added `c`, and
/// removed `gone`.
async fn world(open: bool, branch_exists: bool) -> (MockServer, AppState) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    let a = "projects/helsinki/pipelines/a/pipeline.yaml";
    let b = "projects/helsinki/pipelines/b/pipeline.yaml";
    let c = "projects/helsinki/pipelines/c/pipeline.yaml";
    let gone = "projects/helsinki/pipelines/gone/pipeline.yaml";
    // Main moved after the workspace opened at `abc`: `b` is b9 now, and the branch still
    // holds the b1 it was cut with.
    tree(
        &server,
        "main",
        &[(a, "a1"), (b, "b9"), (gone, "g1"), ("README.md", "r1")],
    )
    .await;
    tree(
        &server,
        "abc",
        &[(a, "a1"), (b, "b1"), (gone, "g1"), ("README.md", "r1")],
    )
    .await;
    // A branch whose name has a slash is read the way real Gitea serves it: the head commit from
    // `branches/{branch}`, then the tree by that sha. Gitea answers 404 for
    // `git/trees/workspace/bikes-v2`, so a stub on that path proves nothing (T-2266).
    if branch_exists {
        branch(&server, "workspace/bikes-v2", "ws1").await;
        tree(
            &server,
            "ws1",
            &[(a, "a2"), (b, "b1"), (c, "c1"), ("README.md", "r2")],
        )
        .await;
    } else {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/branches/workspace/bikes-v2")))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })),
            )
            .mount(&server)
            .await;
    }
    file(&server, "workspace/bikes-v2", a, &pipeline("a", "30s")).await;
    file(&server, "workspace/bikes-v2", c, &pipeline("c", "5m")).await;
    file(&server, "abc", gone, &pipeline("gone", "1h")).await;

    let client = GiteaClient::new(
        server.uri().parse().unwrap(),
        "test-owner",
        "test-repo",
        "t",
    )
    .unwrap();
    let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
    for (name, period) in [("a", "60s"), ("b", "60s"), ("gone", "1h")] {
        state.mirror.upsert(envelope(name, period));
    }
    if open {
        state
            .workspaces
            .create(Opening {
                name: "bikes-v2",
                title: None,
                project: "helsinki",
                owner: "jana@hel.fi",
                base_revision: "abc",
                scope: Scope::Project {},
                ttl_hours: 2,
            })
            .await
            .unwrap();
    }
    (server, state)
}

/// The owner of the copy the world opens, with the bootstrap role so the project reads (T-2562:
/// `mirror_of` resolves the copy as this caller sees it).
fn jana(state: &AppState) -> Identity {
    Identity {
        client: None,
        subject: "jana".into(),
        username: "jana".into(),
        email: Some("jana@hel.fi".into()),
        name: None,
        roles: vec![state.config.bootstrap_admins.clone()],
        groups: vec![],
        second_factor: false,
    }
}

fn period(view: &joinedcontext_portal::store::Mirror, name: &str) -> Option<String> {
    view.get("helsinki", "Pipeline", name)
        .map(|e| e.spec["period"].as_str().unwrap().to_owned())
}

#[tokio::test]
async fn workspace_mirror_shows_workspace_manifest_and_wins_on_conflict() {
    let (_server, state) = world(true, true).await;
    let view = mirror_of(&state, &jana(&state), "bikes-v2", "helsinki")
        .await
        .expect("the view");
    assert_eq!(
        period(&view, "a").as_deref(),
        Some("30s"),
        "the workspace wins"
    );
    assert_eq!(
        period(&view, "c").as_deref(),
        Some("5m"),
        "what the workspace added"
    );
    assert_eq!(
        period(&state.mirror, "a").as_deref(),
        Some("60s"),
        "main is untouched"
    );
}

#[tokio::test]
async fn workspace_mirror_falls_back_to_main_and_drops_what_it_removed() {
    let (_server, state) = world(true, true).await;
    let view = mirror_of(&state, &jana(&state), "bikes-v2", "helsinki")
        .await
        .unwrap();
    assert_eq!(
        period(&view, "b").as_deref(),
        Some("60s"),
        "untouched reads as main"
    );
    assert_eq!(period(&view, "gone"), None, "removed in the workspace");
    assert!(period(&state.mirror, "gone").is_some());
}

#[tokio::test]
async fn a_workspace_with_no_commit_yet_reads_as_main() {
    let (_server, state) = world(true, false).await;
    let view = mirror_of(&state, &jana(&state), "bikes-v2", "helsinki")
        .await
        .unwrap();
    assert_eq!(period(&view, "a").as_deref(), Some("60s"));
    assert_eq!(period(&view, "c"), None);
}

#[tokio::test]
async fn an_unknown_workspace_or_another_projects_is_not_found() {
    let (_server, state) = world(false, true).await;
    let Err(err) = mirror_of(&state, &jana(&state), "bikes-v2", "helsinki").await else {
        panic!("no workspace was opened")
    };
    assert!(err.to_string().contains("bikes-v2"), "{err}");
    let (_server, state) = world(true, true).await;
    let Err(err) = mirror_of(&state, &jana(&state), "bikes-v2", "espoo").await else {
        panic!("the workspace is helsinki's")
    };
    assert!(err.to_string().contains("espoo"), "{err}");
}

#[tokio::test]
async fn what_main_changed_after_the_workspace_opened_reads_as_main() {
    let (_server, state) = world(true, true).await;
    state.mirror.upsert(envelope("b", "90s"));
    let view = mirror_of(&state, &jana(&state), "bikes-v2", "helsinki")
        .await
        .unwrap();
    assert_eq!(
        period(&view, "b").as_deref(),
        Some("90s"),
        "the branch's old copy of b is not the workspace's change"
    );
}
