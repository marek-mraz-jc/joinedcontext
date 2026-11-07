//! The SyncSource control routes are bound to the caller's roles (T-0800, PF-50): forcing a run
//! or pausing is `propose` on the source, detaching is `delete`, and none of them touches the
//! loop or the forge for a person without the binding.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::json;
use wiremock::MockServer;

use common::{envelope, person, send, state_on, REPO};
use jcctl::sync::{RemoteError, SyncRemote};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::sync::driver::{Driver, Remote};
use joinedcontext_portal::sync::state::States;

/// An origin nobody reaches: these routes decide before any fetch, and the loop's own runs
/// are `sync_source_tests`' business.
struct Unreachable;

impl SyncRemote for Unreachable {
    fn revision(&self, _: &jc_core::kinds::SyncOrigin) -> Result<String, RemoteError> {
        Err(RemoteError::Unavailable("not in this test".into()))
    }

    fn checkout(
        &self,
        _: &jc_core::kinds::SyncOrigin,
        _: &str,
        _: &std::path::Path,
    ) -> Result<(), RemoteError> {
        Err(RemoteError::Unavailable("not in this test".into()))
    }
}

const PAUSE: &str = "/api/v1/projects/bb/syncsources/regional/pause";
const SYNC: &str = "/api/v1/projects/bb/syncsources/regional/sync";
const DETACH: &str = "/api/v1/projects/bb/syncsources/regional/detach";

/// The Portal with the sync loop running over the mock forge, one source `bb/regional` in the
/// mirror, and a role `driver` with `verbs` on SyncSource bound to `jana` on project bb.
fn state_with(gitea: &MockServer, verbs: &[&str]) -> AppState {
    let mut state = state_on(gitea);
    let client = state.gitea.clone().expect("forge client");
    let remote: Remote = Arc::new(Unreachable);
    state.sync = Some(Arc::new(Driver::new(
        client,
        Arc::new(States::new(None)),
        remote,
    )));
    state.mirror.upsert(envelope(
        "SyncSource",
        "regional",
        "bb",
        json!({
            "source": { "git": { "url": "https://git.region.sk/udp/models.git", "ref": "main" } },
            "schedule": { "every": "1h" },
            "mode": "mirror",
            "conflictPolicy": "replace"
        }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "driver",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["SyncSource"], "verbs": verbs }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-driver",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "driver", "scope": { "project": "bb" } }),
    ));
    state
}

async fn forge_writes(gitea: &MockServer) -> Vec<String> {
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method != "GET")
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect()
}

#[tokio::test]
async fn a_person_without_a_binding_drives_nothing_and_the_source_stays_as_it_was() {
    let gitea = common::forge().await;
    let state = state_with(&gitea, &["propose", "delete"]);
    for (uri, body) in [
        (PAUSE, Some(json!({ "paused": true }))),
        (SYNC, None),
        (DETACH, None),
    ] {
        let answer = send(&state, person("nobody"), "POST", uri, body).await;
        assert_eq!(
            answer.status,
            StatusCode::FORBIDDEN,
            "{uri}: {}",
            answer.text
        );
        assert!(answer.text.contains("PF-50"), "{}", answer.text);
    }
    let driver = state.sync.as_ref().expect("loop");
    assert!(!driver.status("bb", "regional").await.state.paused);
    assert!(forge_writes(&gitea).await.is_empty(), "{REPO} was written");
}

#[tokio::test]
async fn propose_on_the_source_pauses_it_but_does_not_detach_it() {
    let gitea = common::forge().await;
    let state = state_with(&gitea, &["propose"]);

    let answer = send(
        &state,
        person("jana"),
        "POST",
        PAUSE,
        Some(json!({ "paused": true })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert!(
        state
            .sync
            .as_ref()
            .expect("loop")
            .status("bb", "regional")
            .await
            .state
            .paused
    );

    let answer = send(&state, person("jana"), "POST", DETACH, None).await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert!(answer.text.contains("delete"), "{}", answer.text);
    assert!(forge_writes(&gitea).await.is_empty());
}

#[tokio::test]
async fn delete_on_the_source_opens_the_detach_merge_request() {
    let gitea = common::forge().await;
    let state = state_with(&gitea, &["delete"]);

    let answer = send(&state, person("jana"), "POST", DETACH, None).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    assert!(
        forge_writes(&gitea)
            .await
            .iter()
            .any(|w| w == &format!("POST {REPO}/pulls")),
        "no merge request opened"
    );
}

#[tokio::test]
async fn a_binding_answers_404_for_a_source_that_is_not_there_and_403_for_another_project() {
    let gitea = common::forge().await;
    let state = state_with(&gitea, &["propose", "delete"]);

    let answer = send(
        &state,
        person("jana"),
        "POST",
        "/api/v1/projects/bb/syncsources/missing/pause",
        Some(json!({ "paused": true })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text);

    // The binding is on bb; espoo is not hers, whatever it holds.
    let answer = send(
        &state,
        person("jana"),
        "POST",
        "/api/v1/projects/espoo/syncsources/regional/sync",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert!(forge_writes(&gitea).await.is_empty());
}
