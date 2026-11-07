//! `jc_draft_put` through every door of the registry (T-1535, T-2482; AG-64, PF-50, AG-61, CC-76).
//!
//! What was there before this file: `mcp_portal_tests.rs` and `attack_assistant_tests.rs` name
//! `jc_draft_put` on their happy paths, and the store's own unit test in src/ops/drafts.rs refuses
//! a literal secret and a stale version. No test asserted a refusal through the operation, and none
//! wrote into a workspace that was not the caller's. That one was open (T-2482): the draft
//! operations used the workspace as a key and never asked whose it was.

mod common;

use axum::http::StatusCode;
use common::doors::{self, mcp, run, session, steward, stranger, viewer, PROJECT};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::ops::workspaces::{Opening, Scope};
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};

fn pipeline(class: &str) -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Pipeline",
        "metadata": { "name": "aq-ingest", "namespace": PROJECT },
        "spec": { "class": class },
    })
}

fn put(manifest: Value) -> Value {
    json!({ "kind": "Pipeline", "name": "aq-ingest", "manifest": manifest })
}

/// A second steward of the same project, who owns no workspace.
fn petra() -> Identity {
    Identity {
        client: None,
        subject: "f:1:petra".into(),
        username: "petra".into(),
        email: Some("petra@banskabystrica.sk".into()),
        ..steward()
    }
}

/// A state with Jana's workspace `jana-copy` open over the whole project.
async fn with_janas_copy() -> AppState {
    let state = doors::state();
    state
        .workspaces
        .create(Opening {
            name: "jana-copy",
            title: Some("Air"),
            project: PROJECT,
            owner: "jana",
            base_revision: "base1",
            scope: Scope::Project {},
            ttl_hours: 24,
        })
        .await
        .expect("opened");
    state
}

async fn get(state: &AppState, workspace: Option<&str>) -> doors::Answer {
    let mut input = json!({ "kind": "Pipeline", "name": "aq-ingest" });
    if let Some(name) = workspace {
        input["workspace"] = json!(name);
    }
    doors::call("jc_draft_get", &session(steward()), state, input).await
}

/// AG-61: the steward writes the one shared draft through the session, MCP and a run whose profile
/// names the operation, each write a version on top of the last and signed with its door.
#[tokio::test]
async fn every_door_writes_the_same_shared_draft_and_signs_it() {
    let state = doors::state();
    let doors_in_order = [
        (session(steward()), "person"),
        (mcp(steward()), "mcp"),
        (run(steward(), &["jc_draft_put"]), "run"),
    ];
    for (version, (caller, signed)) in doors_in_order.iter().enumerate() {
        let written = doors::call("jc_draft_put", caller, &state, put(pipeline("resident"))).await;
        assert_eq!(
            StatusCode::OK,
            written.status,
            "{signed}: {}",
            written.text()
        );
        assert_eq!(
            json!(version as i64 + 1),
            written.body["version"],
            "{signed}"
        );
        assert_eq!(*signed, written.body["touchedKind"], "{}", written.text());
    }
}

/// MF-24: a literal secret in the manifest is refused at every door, with the field named and the
/// value nowhere in the answer, and nothing is stored.
#[tokio::test]
async fn a_secret_typed_into_the_manifest_is_refused_at_every_door_and_nothing_is_stored() {
    let state = doors::state();
    let mut manifest = pipeline("resident");
    manifest["spec"]["password"] = json!("hunter2-never-stored");
    for caller in [
        session(steward()),
        mcp(steward()),
        run(steward(), &["jc_draft_put"]),
    ] {
        let refused = doors::call("jc_draft_put", &caller, &state, put(manifest.clone())).await;
        assert_eq!(
            StatusCode::UNPROCESSABLE_ENTITY,
            refused.status,
            "{}",
            refused.text()
        );
        assert!(refused.text().contains("password"), "{}", refused.text());
        assert!(refused.text().contains("secretRef"), "{}", refused.text());
        assert!(
            !refused.text().contains("hunter2"),
            "the answer echoed the secret"
        );
    }
    assert_eq!(
        StatusCode::NOT_FOUND,
        get(&state, None).await.status,
        "a refused draft was stored"
    );
}

/// AG-61: a write on top of a version the caller did not read is a conflict that names the
/// current version, and the draft keeps what the newer write put there.
#[tokio::test]
async fn a_stale_version_is_a_conflict_naming_the_current_one() {
    let state = doors::state();
    let caller = session(steward());
    let first = doors::call("jc_draft_put", &caller, &state, put(pipeline("resident"))).await;
    assert_eq!(StatusCode::OK, first.status, "{}", first.text());
    let mut second = put(pipeline("scheduled"));
    second["expectedVersion"] = json!(1);
    assert_eq!(
        StatusCode::OK,
        doors::call("jc_draft_put", &caller, &state, second)
            .await
            .status
    );

    let mut stale = put(pipeline("stale"));
    stale["expectedVersion"] = json!(1);
    let conflict = doors::call("jc_draft_put", &mcp(steward()), &state, stale).await;
    assert_eq!(StatusCode::CONFLICT, conflict.status, "{}", conflict.text());
    assert_eq!(json!(2), conflict.body["current"], "{}", conflict.text());
    assert_eq!(
        "scheduled",
        get(&state, None).await.body["manifest"]["spec"]["class"]
    );
}

/// PF-50: a viewer may not write a draft, a stranger finds no project, and an agent run whose
/// profile does not name the operation is refused it, at every door.
#[tokio::test]
async fn a_viewer_a_stranger_and_an_unnamed_run_write_nothing() {
    let state = doors::state();
    for (who, caller, expected) in [
        ("viewer", session(viewer()), StatusCode::FORBIDDEN),
        ("viewer over mcp", mcp(viewer()), StatusCode::FORBIDDEN),
        ("stranger", session(stranger()), StatusCode::FORBIDDEN),
        (
            "run without the operation",
            run(steward(), &["jc_draft_get"]),
            StatusCode::FORBIDDEN,
        ),
        (
            "a viewer's run",
            run(viewer(), &["jc_draft_put"]),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let refused = doors::call("jc_draft_put", &caller, &state, put(pipeline("resident"))).await;
        assert_eq!(expected, refused.status, "{who}: {}", refused.text());
    }
    assert_eq!(
        StatusCode::NOT_FOUND,
        get(&state, None).await.status,
        "a refused write was stored"
    );
}

/// CC-76, API/01 §22, T-2482: only a workspace's owner writes into it. Another steward of the same
/// project is refused the put and the drop with the owner named, and the owner's draft is still
/// exactly what the owner wrote.
#[tokio::test]
async fn a_draft_is_never_written_into_or_dropped_from_someone_elses_workspace() {
    let state = with_janas_copy().await;
    let mut own = put(pipeline("resident"));
    own["workspace"] = json!("jana-copy");
    let written = doors::call("jc_draft_put", &session(steward()), &state, own).await;
    assert_eq!(
        StatusCode::OK,
        written.status,
        "the owner may write: {}",
        written.text()
    );

    for caller in [
        session(petra()),
        mcp(petra()),
        run(petra(), &["jc_draft_put", "jc_draft_drop"]),
    ] {
        let mut theirs = put(pipeline("overwritten"));
        theirs["workspace"] = json!("jana-copy");
        let refused = doors::call("jc_draft_put", &caller, &state, theirs).await;
        assert_eq!(StatusCode::FORBIDDEN, refused.status, "{}", refused.text());
        assert!(
            refused.text().contains("only its owner"),
            "{}",
            refused.text()
        );

        let dropped = doors::call(
            "jc_draft_drop",
            &caller,
            &state,
            json!({ "kind": "Pipeline", "name": "aq-ingest", "workspace": "jana-copy" }),
        )
        .await;
        assert_eq!(StatusCode::FORBIDDEN, dropped.status, "{}", dropped.text());
    }
    let kept = get(&state, Some("jana-copy")).await;
    assert_eq!(StatusCode::OK, kept.status, "{}", kept.text());
    assert_eq!("resident", kept.body["manifest"]["spec"]["class"]);
    assert_eq!(
        json!(1),
        kept.body["version"],
        "someone else's write landed"
    );
}

/// CC-76, T-2482: a draft is never written into a name that is no live workspace, so no draft
/// waits in a copy that does not exist for whoever opens one by that name later.
#[tokio::test]
async fn a_draft_is_never_written_into_a_workspace_that_is_not_there() {
    let state = with_janas_copy().await;
    let mut nowhere = put(pipeline("resident"));
    nowhere["workspace"] = json!("never-opened");
    let refused = doors::call("jc_draft_put", &session(steward()), &state, nowhere).await;
    assert_eq!(StatusCode::NOT_FOUND, refused.status, "{}", refused.text());
    assert!(
        refused.text().contains("no workspace named"),
        "{}",
        refused.text()
    );
    assert_eq!(
        StatusCode::NOT_FOUND,
        get(&state, Some("never-opened")).await.status
    );
}

/// PF-59, T-2482: the drafts of a workspace are read where the workspace itself may be read. A
/// person with no grant in the project gets the workspace's own 404 and nothing of the draft.
#[tokio::test]
async fn the_drafts_of_a_workspace_are_read_only_where_the_workspace_is() {
    let state = with_janas_copy().await;
    let mut own = put(pipeline("resident"));
    own["workspace"] = json!("jana-copy");
    assert_eq!(
        StatusCode::OK,
        doors::call("jc_draft_put", &session(steward()), &state, own)
            .await
            .status
    );

    let colleague = doors::call(
        "jc_draft_list",
        &session(viewer()),
        &state,
        json!({ "workspace": "jana-copy" }),
    )
    .await;
    assert_eq!(
        StatusCode::OK,
        colleague.status,
        "a reader of the project reads its copies: {}",
        colleague.text()
    );

    for (name, input) in [
        ("jc_draft_list", json!({ "workspace": "jana-copy" })),
        (
            "jc_draft_get",
            json!({ "kind": "Pipeline", "name": "aq-ingest", "workspace": "jana-copy" }),
        ),
    ] {
        let refused = doors::call(name, &mcp(stranger()), &state, input).await;
        assert_eq!(
            StatusCode::NOT_FOUND,
            refused.status,
            "{name}: {}",
            refused.text()
        );
        assert!(
            !refused.text().contains("resident"),
            "{name} leaked the draft"
        );
    }
    let missing = doors::call(
        "jc_draft_list",
        &session(steward()),
        &state,
        json!({ "workspace": "never-opened" }),
    )
    .await;
    assert_eq!(StatusCode::NOT_FOUND, missing.status, "{}", missing.text());
}

/// AG-64: the REST route and `POST …/ops/jc_draft_put` refuse alike.
#[tokio::test]
async fn the_rest_route_and_the_ops_door_refuse_alike() {
    let state = with_janas_copy().await;
    let uri = "/api/v1/projects/ovzdusie/drafts/Pipeline/aq-ingest";
    for (identity, workspace) in [
        (viewer(), None),
        (petra(), Some("jana-copy")),
        (stranger(), None),
    ] {
        let rest_uri =
            workspace.map_or_else(|| uri.to_owned(), |ws| format!("{uri}?workspace={ws}"));
        let rest = doors::http(
            &state,
            identity.clone(),
            "PUT",
            &rest_uri,
            Some(json!({ "manifest": pipeline("resident") })),
        )
        .await;
        let mut input = put(pipeline("resident"));
        if let Some(ws) = workspace {
            input["workspace"] = json!(ws);
        }
        let door = doors::post_op(&state, identity.clone(), "jc_draft_put", input).await;
        assert_eq!(
            StatusCode::FORBIDDEN,
            rest.status,
            "{}: {}",
            identity.username,
            rest.text()
        );
        assert_eq!(
            rest.status,
            door.status,
            "{}: {} vs {}",
            identity.username,
            rest.text(),
            door.text()
        );
    }
}
