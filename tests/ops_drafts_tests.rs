//! The draft reads and the drop through every door of the registry (T-1526; AG-64, PF-50, AG-61).
//!
//! What was there before this file: the REST routes `GET|DELETE /api/v1/projects/{project}/drafts
//! [/{kind}/{name}]` are thin wrappers that call `jc_draft_get`, `jc_draft_list` and
//! `jc_draft_drop` (src/api/drafts.rs), and the store itself is tested in src/ops/drafts.rs. No test
//! called the operations by name as a steward, a viewer, an agent run or an MCP client, so nothing
//! held that the doors refuse alike.
//!
//! Dropping a draft that is not there answers `{ "dropped": false }` and not `404`: the operation
//! is idempotent by contract (API/01, "discard it → { "dropped": true }"), which is what a second
//! window closing the same form relies on.

mod common;

use axum::http::StatusCode;
use common::doors::{self, mcp, run, session, steward, stranger, viewer, PROJECT};
use common::envelope;
use joinedcontext_portal::ops::workspaces::{Opening, Scope};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use serde_json::{json, Value};

fn pipeline(name: &str) -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Pipeline",
        "metadata": { "name": name, "namespace": PROJECT },
        "spec": { "class": "resident" },
    })
}

fn policy(name: &str) -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Policy",
        "metadata": { "name": name, "namespace": PROJECT },
        "spec": { "operations": ["retrieveOps"] },
    })
}

/// A state holding the steward's draft of one pipeline and one policy.
async fn with_drafts() -> joinedcontext_portal::state::AppState {
    let state = doors::state();
    for (kind, name, manifest) in [
        ("Pipeline", "aq-ingest", pipeline("aq-ingest")),
        ("Policy", "public-read", policy("public-read")),
    ] {
        let put = doors::call(
            "jc_draft_put",
            &session(steward()),
            &state,
            json!({ "kind": kind, "name": name, "manifest": manifest }),
        )
        .await;
        assert_eq!(StatusCode::OK, put.status, "{kind}/{name}: {}", put.text());
    }
    state
}

fn names(answer: &doors::Answer) -> Vec<String> {
    answer.body["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    format!(
                        "{}/{}",
                        item["kind"].as_str().unwrap_or(""),
                        item["name"].as_str().unwrap_or("")
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// AG-61, PF-59: a steward reads and lists the drafts of the project, a viewer reads them too, and
/// a stranger is answered as if the project were not there, through the session and MCP alike.
#[tokio::test]
async fn the_project_reads_its_drafts_and_a_stranger_finds_no_project() {
    let state = with_drafts().await;
    for caller in [
        session(steward()),
        mcp(steward()),
        session(viewer()),
        mcp(viewer()),
    ] {
        let listed = doors::call("jc_draft_list", &caller, &state, json!({})).await;
        assert_eq!(StatusCode::OK, listed.status, "{}", listed.text());
        let mut seen = names(&listed);
        seen.sort();
        assert_eq!(vec!["Pipeline/aq-ingest", "Policy/public-read"], seen);
        let one = doors::call(
            "jc_draft_get",
            &caller,
            &state,
            json!({ "kind": "Pipeline", "name": "aq-ingest" }),
        )
        .await;
        assert_eq!(StatusCode::OK, one.status, "{}", one.text());
        assert_eq!("resident", one.body["manifest"]["spec"]["class"]);
    }
    for caller in [session(stranger()), mcp(stranger())] {
        for (name, input) in [
            ("jc_draft_list", json!({})),
            (
                "jc_draft_get",
                json!({ "kind": "Pipeline", "name": "aq-ingest" }),
            ),
        ] {
            let refused = doors::call(name, &caller, &state, input).await;
            assert_eq!(
                StatusCode::NOT_FOUND,
                refused.status,
                "{name}: {}",
                refused.text()
            );
            assert!(
                !refused.text().contains("resident"),
                "{name} leaked the manifest"
            );
        }
    }
}

/// PF-59, T-1455: a draft is readable exactly where its manifest would be. A person who may read
/// pipelines and nothing else is neither listed nor given the policy's draft, and a draft they may
/// not read is answered as one that is not there.
#[tokio::test]
async fn a_draft_whose_manifest_the_person_may_not_read_is_neither_listed_nor_read() {
    let state = with_drafts().await;
    state.mirror.upsert(envelope(
        "Role",
        "pipeline-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "pipeline-reader-binding",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "group": "pipeline-readers" }],
            "role": "pipeline-reader",
            "scope": { "project": PROJECT },
        }),
    ));
    let mut reader = stranger();
    reader.username = "milan".into();
    reader.groups = vec!["pipeline-readers".into()];

    let listed = doors::call("jc_draft_list", &session(reader.clone()), &state, json!({})).await;
    assert_eq!(StatusCode::OK, listed.status, "{}", listed.text());
    assert_eq!(vec!["Pipeline/aq-ingest"], names(&listed));
    let hidden = doors::call(
        "jc_draft_get",
        &mcp(reader),
        &state,
        json!({ "kind": "Policy", "name": "public-read" }),
    )
    .await;
    assert_eq!(StatusCode::NOT_FOUND, hidden.status, "{}", hidden.text());
    assert!(!hidden.text().contains("retrieveOps"));
}

/// PF-50: dropping a draft needs `propose` on its kind, the same as writing it. A viewer is refused
/// with the verb and the kind named, and the draft is still there afterwards.
#[tokio::test]
async fn a_viewer_may_not_drop_a_draft_and_the_draft_stays() {
    let state = with_drafts().await;
    for caller in [session(viewer()), mcp(viewer())] {
        let refused = doors::call(
            "jc_draft_drop",
            &caller,
            &state,
            json!({ "kind": "Pipeline", "name": "aq-ingest" }),
        )
        .await;
        assert_eq!(StatusCode::FORBIDDEN, refused.status, "{}", refused.text());
        assert!(refused.text().contains("propose"), "{}", refused.text());
    }
    let still = doors::call(
        "jc_draft_get",
        &session(steward()),
        &state,
        json!({ "kind": "Pipeline", "name": "aq-ingest" }),
    )
    .await;
    assert_eq!(
        StatusCode::OK,
        still.status,
        "a refused drop removed the draft"
    );
}

/// AG-61: the steward drops a draft once, and dropping it again, or one that never existed, answers
/// `dropped: false` without touching the other draft.
#[tokio::test]
async fn a_drop_is_idempotent_and_takes_only_its_own_draft() {
    let state = with_drafts().await;
    let caller = session(steward());
    let input = json!({ "kind": "Pipeline", "name": "aq-ingest" });
    let first = doors::call("jc_draft_drop", &caller, &state, input.clone()).await;
    assert_eq!(json!({ "dropped": true }), first.body, "{}", first.text());
    let again = doors::call("jc_draft_drop", &caller, &state, input).await;
    assert_eq!(json!({ "dropped": false }), again.body, "{}", again.text());
    let unknown = doors::call(
        "jc_draft_drop",
        &caller,
        &state,
        json!({ "kind": "Pipeline", "name": "never-drafted" }),
    )
    .await;
    assert_eq!(
        json!({ "dropped": false }),
        unknown.body,
        "{}",
        unknown.text()
    );

    let listed = doors::call("jc_draft_list", &caller, &state, json!({})).await;
    assert_eq!(vec!["Policy/public-read"], names(&listed));
}

/// CC-76, T-2267: a draft inside a workspace is reached only with the workspace named. Without it
/// the project's own drafts answer, and the copy's draft is not among them.
#[tokio::test]
async fn a_draft_inside_a_workspace_is_reached_only_with_the_workspace_named() {
    let state = doors::state();
    state
        .workspaces
        .create(Opening {
            name: "ws-air",
            title: None,
            project: PROJECT,
            owner: "jana",
            base_revision: "base1",
            scope: Scope::Project {},
            ttl_hours: 24,
        })
        .await
        .expect("opened");
    let caller = session(steward());
    let put = doors::call(
        "jc_draft_put",
        &caller,
        &state,
        json!({ "kind": "Pipeline", "name": "aq-copy", "manifest": pipeline("aq-copy"), "workspace": "ws-air" }),
    )
    .await;
    assert_eq!(StatusCode::OK, put.status, "{}", put.text());

    let outside = doors::call("jc_draft_list", &caller, &state, json!({})).await;
    assert!(
        names(&outside).is_empty(),
        "the copy's draft leaked into the project's: {}",
        outside.text()
    );
    let unnamed = doors::call(
        "jc_draft_get",
        &caller,
        &state,
        json!({ "kind": "Pipeline", "name": "aq-copy" }),
    )
    .await;
    assert_eq!(StatusCode::NOT_FOUND, unnamed.status, "{}", unnamed.text());

    let inside = doors::call(
        "jc_draft_list",
        &caller,
        &state,
        json!({ "workspace": "ws-air" }),
    )
    .await;
    assert_eq!(vec!["Pipeline/aq-copy"], names(&inside));
    let named = doors::call(
        "jc_draft_get",
        &caller,
        &state,
        json!({ "kind": "Pipeline", "name": "aq-copy", "workspace": "ws-air" }),
    )
    .await;
    assert_eq!(StatusCode::OK, named.status, "{}", named.text());
    assert_eq!("ws-air", named.body["workspace"]);
}

/// AG-70: an agent run reads and drops drafts only when its profile names the operation, and the
/// refusal says so; a profile that names them lets the run through like its person.
#[tokio::test]
async fn an_agent_run_reaches_the_drafts_only_through_its_profile() {
    let state = with_drafts().await;
    let bare = run(steward(), &["jc_resource_list"]);
    for (name, input) in [
        ("jc_draft_list", json!({})),
        (
            "jc_draft_get",
            json!({ "kind": "Pipeline", "name": "aq-ingest" }),
        ),
        (
            "jc_draft_drop",
            json!({ "kind": "Pipeline", "name": "aq-ingest" }),
        ),
    ] {
        let refused = doors::call(name, &bare, &state, input).await;
        assert_eq!(
            StatusCode::FORBIDDEN,
            refused.status,
            "{name}: {}",
            refused.text()
        );
        assert!(
            !doors::offered(&bare, &state).contains(&name.to_owned()),
            "{name} is offered to a run that is refused it"
        );
    }
    let named = run(
        steward(),
        &["jc_draft_list", "jc_draft_get", "jc_draft_drop"],
    );
    let listed = doors::call("jc_draft_list", &named, &state, json!({})).await;
    assert_eq!(StatusCode::OK, listed.status, "{}", listed.text());
    let dropped = doors::call(
        "jc_draft_drop",
        &named,
        &state,
        json!({ "kind": "Pipeline", "name": "aq-ingest" }),
    )
    .await;
    assert_eq!(
        json!({ "dropped": true }),
        dropped.body,
        "{}",
        dropped.text()
    );
    // The run acts as its person and no wider: its viewer is still refused the drop.
    let viewer_run = run(viewer(), &["jc_draft_drop"]);
    let refused = doors::call(
        "jc_draft_drop",
        &viewer_run,
        &state,
        json!({ "kind": "Policy", "name": "public-read" }),
    )
    .await;
    assert_eq!(StatusCode::FORBIDDEN, refused.status, "{}", refused.text());
}

/// AG-64: the REST route and `POST …/ops/{name}` answer the same status to the same person.
#[tokio::test]
async fn the_rest_route_and_the_ops_door_answer_alike() {
    let state = with_drafts().await;
    let get = "/api/v1/projects/ovzdusie/drafts/Pipeline/aq-ingest";
    for identity in [steward(), viewer(), stranger()] {
        let rest = doors::http(&state, identity.clone(), "GET", get, None).await;
        let door = doors::post_op(
            &state,
            identity.clone(),
            "jc_draft_get",
            json!({ "kind": "Pipeline", "name": "aq-ingest" }),
        )
        .await;
        assert_eq!(
            rest.status,
            door.status,
            "{}: {} vs {}",
            identity.username,
            rest.text(),
            door.text()
        );
        let rest_list = doors::http(
            &state,
            identity.clone(),
            "GET",
            "/api/v1/projects/ovzdusie/drafts",
            None,
        )
        .await;
        let door_list = doors::post_op(&state, identity.clone(), "jc_draft_list", json!({})).await;
        assert_eq!(rest_list.status, door_list.status, "{}", identity.username);
    }
    let rest_drop = doors::http(&state, viewer(), "DELETE", get, None).await;
    let door_drop = doors::post_op(
        &state,
        viewer(),
        "jc_draft_drop",
        json!({ "kind": "Pipeline", "name": "aq-ingest" }),
    )
    .await;
    assert_eq!(
        StatusCode::FORBIDDEN,
        rest_drop.status,
        "{}",
        rest_drop.text()
    );
    assert_eq!(rest_drop.status, door_drop.status);
}
