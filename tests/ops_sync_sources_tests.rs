//! The SyncSource operations through every door of the registry (T-1531; AG-64, PF-50, PF-59,
//! MF-30, CC-70).
//!
//! What was there before this file: the REST routes are tested in `sync_source_routes_tests.rs`
//! (no binding drives nothing, `propose` pauses but does not detach), `edge_keys_sync_tests.rs`,
//! `edge_sync_detach_webhook_tests.rs` and `project_read_tests.rs` (status as a read, PF-59); the
//! loop itself in `sync_source_tests.rs` (a paused source runs for nobody). No test called
//! `jc_syncsource_status`, `jc_syncsource_sync`, `jc_syncsource_pause` or `jc_syncsource_detach`
//! through the registry.
//!
//! API/01 §syncsources: a paused source asked to sync "answers with its status and runs nothing",
//! so that case is a `200` whose status says `Paused`, not a refusal.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::doors::{self, mcp, run, session, steward, stranger, viewer, PROJECT};
use common::{encode, envelope, forge, state_on, REPO};
use jcctl::sync::{RemoteError, SyncRemote};
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::sync::driver::{Driver, Remote};
use joinedcontext_portal::sync::state::States;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HEAD: &str = "5e1f0c2a9b8d7e6f5a4b3c2d1e0f9a8b7c6d5e4f";
const SOURCE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: SyncSource\nmetadata:\n  name: regional\n  namespace: ovzdusie\nspec:\n  source:\n    git: { url: https://git.region.sk/udp/models.git, ref: main }\n  schedule: { interval: 1h }\n  mode: mirror\n  conflictPolicy: replace\n";
const SOURCE_FILE: &str = "projects/ovzdusie/sync/regional.yaml";

/// An origin that does not answer: a forced run is judged and says why it could not fetch.
struct Silent;

impl SyncRemote for Silent {
    fn revision(&self, _: &jc_core::kinds::SyncOrigin) -> Result<String, RemoteError> {
        Err(RemoteError::Unavailable(
            "the origin did not answer in time".into(),
        ))
    }

    fn checkout(
        &self,
        _: &jc_core::kinds::SyncOrigin,
        _: &str,
        _: &std::path::Path,
    ) -> Result<(), RemoteError> {
        Err(RemoteError::Unavailable("not reached".into()))
    }
}

async fn world() -> (MockServer, AppState) {
    let server = forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/branches/main")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "name": "main", "commit": { "id": HEAD } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/git/trees/{HEAD}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "tree", "truncated": false,
            "tree": [{ "path": SOURCE_FILE, "type": "blob" }],
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/contents/{SOURCE_FILE}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "sha": "blob", "content": encode(SOURCE) })),
        )
        .mount(&server)
        .await;

    let mut state = state_on(&server);
    let client = state.gitea.clone().expect("forge client");
    let remote: Remote = Arc::new(Silent);
    state.sync = Some(Arc::new(Driver::new(
        client,
        Arc::new(States::new(None)),
        remote,
    )));
    let state = doors::state_with(state);
    state.mirror.upsert(envelope(
        "SyncSource",
        "regional",
        PROJECT,
        json!({
            "source": { "git": { "url": "https://git.region.sk/udp/models.git", "ref": "main" } },
            "schedule": { "interval": "1h" },
            "mode": "mirror",
            "conflictPolicy": "replace"
        }),
    ));
    (server, state)
}

fn named() -> Value {
    json!({ "name": "regional" })
}

fn paused(on: bool) -> Value {
    json!({ "name": "regional", "paused": on })
}

async fn is_paused(state: &AppState) -> bool {
    state
        .sync
        .as_ref()
        .expect("loop")
        .status(PROJECT, "regional")
        .await
        .state
        .paused
}

async fn forge_writes(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.method.as_str() != "GET")
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect()
}

/// MF-30, PF-59: status is a read: the steward and the viewer read it at every door, and a run
/// whose profile names it.
#[tokio::test]
async fn every_door_reads_a_sources_status() {
    let (_server, state) = world().await;
    for (who, caller) in [
        ("steward", session(steward())),
        ("viewer", session(viewer())),
        ("viewer over mcp", mcp(viewer())),
        ("viewer's run", run(viewer(), &["jc_syncsource_status"])),
    ] {
        let status = doors::call("jc_syncsource_status", &caller, &state, named()).await;
        assert_eq!(StatusCode::OK, status.status, "{who}: {}", status.text());
        assert_eq!("regional", status.body["name"], "{who}");
        assert_eq!(json!(false), status.body["paused"], "{who}");
    }
}

/// MF-30: the steward pauses, resumes and forces a run through the session, MCP and a run whose
/// profile names the operations. A forced run of a live source is judged and says why the origin
/// did not answer; the forge is not written.
#[tokio::test]
async fn every_door_pauses_resumes_and_forces_a_run() {
    let (server, state) = world().await;
    for caller in [
        session(steward()),
        mcp(steward()),
        run(steward(), &["jc_syncsource_pause", "jc_syncsource_sync"]),
    ] {
        let off = doors::call("jc_syncsource_pause", &caller, &state, paused(true)).await;
        assert_eq!(StatusCode::OK, off.status, "{}", off.text());
        assert_eq!("Paused", off.body["phase"], "{}", off.text());
        assert!(is_paused(&state).await);

        let on = doors::call("jc_syncsource_pause", &caller, &state, paused(false)).await;
        assert_eq!(StatusCode::OK, on.status, "{}", on.text());
        assert!(!is_paused(&state).await);

        let forced = doors::call("jc_syncsource_sync", &caller, &state, named()).await;
        assert_eq!(StatusCode::OK, forced.status, "{}", forced.text());
        assert_eq!(json!([]), forced.body["proposed"], "{}", forced.text());
        assert!(
            forced.text().contains("the origin did not answer in time"),
            "{}",
            forced.text()
        );
    }
    assert_eq!(Vec::<String>::new(), forge_writes(&server).await);
}

/// API/01, MF-30: a paused source asked to sync runs nothing and answers its status, which says
/// why: `paused` and the phase `Paused`. The switch is not overridden by the button.
#[tokio::test]
async fn a_forced_run_of_a_paused_source_runs_nothing_and_says_it_is_paused() {
    let (server, state) = world().await;
    let caller = mcp(steward());
    doors::call("jc_syncsource_pause", &caller, &state, paused(true)).await;
    let forced = doors::call("jc_syncsource_sync", &caller, &state, named()).await;
    assert_eq!(StatusCode::OK, forced.status, "{}", forced.text());
    assert_eq!(json!([]), forced.body["proposed"], "{}", forced.text());
    assert_eq!(
        json!(true),
        forced.body["status"]["paused"],
        "{}",
        forced.text()
    );
    assert_eq!(
        "Paused",
        forced.body["status"]["phase"],
        "{}",
        forced.text()
    );
    assert!(
        !forced.text().contains("did not answer"),
        "the origin was asked: {}",
        forced.text()
    );
    assert!(is_paused(&state).await, "the forced run resumed the source");
    assert_eq!(Vec::<String>::new(), forge_writes(&server).await);
}

/// CC-70, PF-50: detaching is `delete` on the source and opens the merge request that removes it,
/// with syncing paused at once; `propose` alone pauses and forces a run and never detaches.
#[tokio::test]
async fn detaching_needs_delete_and_pausing_needs_propose() {
    let (_server, state) = world().await;
    for (who, caller) in [
        ("viewer", session(viewer())),
        ("viewer over mcp", mcp(viewer())),
        (
            "viewer's run",
            run(
                viewer(),
                &[
                    "jc_syncsource_pause",
                    "jc_syncsource_sync",
                    "jc_syncsource_detach",
                ],
            ),
        ),
    ] {
        for (name, input) in [
            ("jc_syncsource_pause", paused(true)),
            ("jc_syncsource_sync", named()),
            ("jc_syncsource_detach", named()),
        ] {
            let refused = doors::call(name, &caller, &state, input).await;
            assert_eq!(
                StatusCode::FORBIDDEN,
                refused.status,
                "{who} {name}: {}",
                refused.text()
            );
            // The person's PF-50, or the run's AG-70 where its profile refuses first.
            assert!(
                refused.text().contains("PF-50") || refused.text().contains("AG-70"),
                "{}",
                refused.text()
            );
        }
    }
    let without = run(steward(), &["jc_syncsource_pause"]);
    let refused = doors::call("jc_syncsource_detach", &without, &state, named()).await;
    assert_eq!(StatusCode::FORBIDDEN, refused.status, "{}", refused.text());
    assert!(!is_paused(&state).await, "a refused call switched the loop");

    let detached = doors::call("jc_syncsource_detach", &session(steward()), &state, named()).await;
    assert_eq!(StatusCode::OK, detached.status, "{}", detached.text());
    assert_eq!(
        "https://gitea.example/pulls/9",
        detached.body["url"],
        "{}",
        detached.text()
    );
    assert_eq!(
        "chg-00000009",
        detached.body["changeId"],
        "{}",
        detached.text()
    );
    assert_eq!("red", detached.body["lane"], "{}", detached.text());
    assert!(
        is_paused(&state).await,
        "syncing goes on while its removal waits"
    );
}

/// PF-59: a source that is not there, and a project the caller may not read, answer 404 at every
/// door; the stranger is refused a write with PF-50's 403 before the loop is asked.
#[tokio::test]
async fn an_unknown_source_and_a_stranger_are_answered_before_the_loop() {
    let (_server, state) = world().await;
    for caller in [session(steward()), mcp(steward())] {
        for (name, input) in [
            ("jc_syncsource_status", json!({ "name": "nowhere" })),
            ("jc_syncsource_sync", json!({ "name": "nowhere" })),
            (
                "jc_syncsource_pause",
                json!({ "name": "nowhere", "paused": true }),
            ),
        ] {
            let missing = doors::call(name, &caller, &state, input).await;
            assert_eq!(
                StatusCode::NOT_FOUND,
                missing.status,
                "{name}: {}",
                missing.text()
            );
        }
    }
    let hidden = doors::call("jc_syncsource_status", &mcp(stranger()), &state, named()).await;
    assert_eq!(StatusCode::NOT_FOUND, hidden.status, "{}", hidden.text());
    let refused = doors::call(
        "jc_syncsource_pause",
        &mcp(stranger()),
        &state,
        paused(true),
    )
    .await;
    assert_eq!(StatusCode::FORBIDDEN, refused.status, "{}", refused.text());
    assert!(!is_paused(&state).await);
}

/// AG-64: the REST routes and `POST …/ops/{name}` answer each person alike.
#[tokio::test]
async fn the_rest_routes_and_the_ops_door_answer_alike() {
    let (_server, state) = world().await;
    let base = "/api/v1/projects/ovzdusie/syncsources/regional";
    for identity in [viewer(), stranger()] {
        for (name, rest_method, uri, body, input) in [
            (
                "jc_syncsource_status",
                "GET",
                format!("{base}/status"),
                None,
                named(),
            ),
            (
                "jc_syncsource_sync",
                "POST",
                format!("{base}/sync"),
                None,
                named(),
            ),
            (
                "jc_syncsource_pause",
                "POST",
                format!("{base}/pause"),
                Some(json!({ "paused": true })),
                paused(true),
            ),
            (
                "jc_syncsource_detach",
                "POST",
                format!("{base}/detach"),
                None,
                named(),
            ),
        ] {
            let rest = doors::http(&state, identity.clone(), rest_method, &uri, body).await;
            let door = doors::post_op(&state, identity.clone(), name, input).await;
            if !(identity.username == "peter" && name == "jc_syncsource_status") {
                assert!(rest.status.is_client_error(), "{name}: {}", rest.text());
            }
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
}
