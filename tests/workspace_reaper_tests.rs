//! What an expired workspace leaves behind, and who takes it away (T-1260; CC-81, PF-83, CC-67).
//!
//! Expiry already hides a workspace: `live` answers `NotFound`, the listings skip it and the
//! gateway is no longer told about its preview. The branch is the part that does not go by
//! itself — and a branch nobody can reach through the Portal is a copy of the configuration
//! outliving the TTL that was set for it, which is the whole of what a TTL is for.

mod common;

use chrono::{Duration, Utc};
use common::{forge, state_on, REPO};
use joinedcontext_portal::ops::workspaces::{reap_expired_at, Opening, Scope};
use joinedcontext_portal::state::AppState;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "ovzdusie";

/// A forge that answers the branch deletion with `status`, and the state on it.
async fn world(name: &str, status: u16) -> (MockServer, AppState) {
    let server = forge().await;
    Mock::given(method("DELETE"))
        .and(path(format!("{REPO}/branches/workspace/{name}")))
        .respond_with(ResponseTemplate::new(status))
        .mount(&server)
        .await;
    let state = state_on(&server);
    state
        .workspaces
        .create(Opening {
            name,
            title: None,
            project: PROJECT,
            owner: "jana@hel.fi",
            base_revision: "base1",
            scope: Scope::Project {},
            ttl_hours: 1,
        })
        .await
        .expect("the workspace opens");
    (server, state)
}

/// Every branch deletion the forge saw.
async fn deletions(server: &MockServer, name: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| {
            request.method == wiremock::http::Method::DELETE
                && request.url.path() == format!("{REPO}/branches/workspace/{name}")
        })
        .count()
}

/// CC-81: an hour after its hour, the workspace is a deleted branch and a deleted record.
#[tokio::test]
async fn an_expired_workspace_loses_its_branch_and_its_record() {
    let (server, state) = world("old", 204).await;
    let reaped = reap_expired_at(&state, Utc::now() + Duration::hours(2)).await;
    assert_eq!(reaped, 1, "nothing was reaped");
    assert_eq!(
        deletions(&server, "old").await,
        1,
        "the branch is still there"
    );
    assert!(
        state.workspaces.get("old").await.unwrap().is_none(),
        "the record outlived its branch"
    );
}

/// The other half of the rule: a workspace inside its TTL is somebody's open work.
#[tokio::test]
async fn a_workspace_that_has_not_expired_is_not_touched() {
    let (server, state) = world("fresh", 204).await;
    let reaped = reap_expired_at(&state, Utc::now()).await;
    assert_eq!(reaped, 0, "a live workspace was reaped");
    assert_eq!(
        deletions(&server, "fresh").await,
        0,
        "the branch of a live workspace was deleted"
    );
    assert!(state.workspaces.get("fresh").await.unwrap().is_some());
}

/// PF-83: the record goes last, so a forge that cannot be reached leaves something to try again.
/// Deleting the record first would leave a branch nothing in the Portal knows about.
#[tokio::test]
async fn a_forge_that_cannot_be_reached_keeps_the_record_for_the_next_pass() {
    let (server, state) = world("stuck", 500).await;
    let reaped = reap_expired_at(&state, Utc::now() + Duration::hours(2)).await;
    assert_eq!(
        reaped, 0,
        "the record was deleted although the branch stands"
    );
    assert!(
        state.workspaces.get("stuck").await.unwrap().is_some(),
        "the record is gone and the branch is not"
    );

    // The next pass finds it again, and a forge that answers takes both away.
    server.reset().await;
    let server = forge_deleting("stuck", server).await;
    let reaped = reap_expired_at(&state, Utc::now() + Duration::hours(2)).await;
    assert_eq!(reaped, 1, "the retry did not finish the expiry");
    assert!(state.workspaces.get("stuck").await.unwrap().is_none());
    assert_eq!(deletions(&server, "stuck").await, 1);
}

/// The same forge, now answering the branch deletion.
async fn forge_deleting(name: &str, server: MockServer) -> MockServer {
    Mock::given(method("DELETE"))
        .and(path(format!("{REPO}/branches/workspace/{name}")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    server
}

/// CC-81: a branch the forge no longer has is not an error — a discard that raced the reaper, or a
/// hand deleting it in the forge, leaves a record whose expiry still has to finish.
#[tokio::test]
async fn a_branch_that_is_already_gone_still_finishes_the_expiry() {
    // No DELETE mock at all: the forge answers 404, which is what a missing branch is.
    let server = forge().await;
    let state = state_on(&server);
    state
        .workspaces
        .create(Opening {
            name: "ghost",
            title: None,
            project: PROJECT,
            owner: "jana@hel.fi",
            base_revision: "base1",
            scope: Scope::Project {},
            ttl_hours: 1,
        })
        .await
        .expect("the workspace opens");
    let reaped = reap_expired_at(&state, Utc::now() + Duration::hours(2)).await;
    assert_eq!(reaped, 1, "a record waits for a branch that does not exist");
    assert!(state.workspaces.get("ghost").await.unwrap().is_none());
}

/// One pass takes every expired workspace, not the first: a Portal that was down for a day comes
/// back to a dozen of them.
#[tokio::test]
async fn one_pass_reaps_every_expired_workspace() {
    let server = forge().await;
    let state = state_on(&server);
    for name in ["one", "two", "three"] {
        Mock::given(method("DELETE"))
            .and(path(format!("{REPO}/branches/workspace/{name}")))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        state
            .workspaces
            .create(Opening {
                name,
                title: None,
                project: PROJECT,
                owner: "jana@hel.fi",
                base_revision: "base1",
                scope: Scope::Project {},
                ttl_hours: 1,
            })
            .await
            .expect("the workspace opens");
    }
    let reaped = reap_expired_at(&state, Utc::now() + Duration::hours(2)).await;
    assert_eq!(reaped, 3, "the pass stopped early");
    assert!(state.workspaces.list(PROJECT).await.unwrap().is_empty());
}

/// CC-76: a copy's name is taken while the copy is, and free once it is not — the name is the
/// branch and the preview prefix, so a name that stays taken after the TTL is the residue the
/// reaper exists to remove (T-2232).
#[tokio::test]
async fn a_reaped_copys_name_is_free_again() {
    let (_server, state) = world("again", 204).await;
    let taken = state
        .workspaces
        .create(Opening {
            name: "again",
            title: None,
            project: PROJECT,
            owner: "jana@hel.fi",
            base_revision: "base1",
            scope: Scope::Project {},
            ttl_hours: 1,
        })
        .await;
    assert!(taken.is_err(), "the name was free while the copy lived");

    reap_expired_at(&state, Utc::now() + Duration::hours(2)).await;
    state
        .workspaces
        .create(Opening {
            name: "again",
            title: None,
            project: PROJECT,
            owner: "petra@hel.fi",
            base_revision: "base2",
            scope: Scope::Project {},
            ttl_hours: 1,
        })
        .await
        .expect("the name is free after the reap");
}

/// PF-83, T-1593: a copy whose preview runs is reaped like any other, and the node's slot goes with
/// it — the slot is counted from the copies whose preview runs, so a record left behind holds one.
#[tokio::test]
async fn a_copy_whose_preview_is_running_is_reaped_too_and_its_slot_freed() {
    let (_server, state) = world("shown", 204).await;
    state
        .workspaces
        .set_preview_state(
            "shown",
            joinedcontext_portal::ops::workspaces::PreviewState::Running,
        )
        .await
        .expect("the preview runs");
    assert_eq!(
        state.workspaces.previewing().await.unwrap().len(),
        1,
        "the slot is held while the copy lives"
    );

    let reaped = reap_expired_at(&state, Utc::now() + Duration::hours(2)).await;
    assert_eq!(reaped, 1, "a copy with a running preview was skipped");
    assert!(
        state.workspaces.previewing().await.unwrap().is_empty(),
        "the node's slot is still held by a copy that no longer exists"
    );
}
