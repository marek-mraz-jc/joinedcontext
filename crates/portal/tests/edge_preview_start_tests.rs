//! Edge cases of starting a workspace preview (T-2129, T-2294; CC-78, CC-81, PF-59, PF-83).
//!
//! **The contract, in one sentence:** a preview is started by the workspace's owner, once, while the
//! node has a slot — and nothing the refusals say may be about a workspace the caller cannot see.
//!
//! A preview renders somebody's branch and serves its Endpoints, and the node takes two at a time
//! (`MAX_ON_NODE`). That makes `start` a place where one person's request has to reason about other
//! people's work, which is where an answer starts saying too much: the full-node refusal used to list
//! every running workspace by name, across projects (T-2294, fixed in this commit).
//!
//! Tests only for `start` itself (the family's rule); the one red case became T-2294 and is the first
//! case here.

mod common;

use common::{envelope, person, state_on};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::ops::previews;
use joinedcontext_portal::ops::workspaces::{Opening, PreviewState, Scope};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use wiremock::MockServer;

const PROJECT: &str = "ovzdusie";
const ELSEWHERE: &str = "doprava";

/// A person who may read and propose in both projects, so nothing below is refused for want of a
/// binding: what the cases are about is ownership and what an answer says.
fn everywhere() -> Identity {
    Identity {
        client: None,
        groups: vec!["devs".into()],
        ..person("jana")
    }
}

fn with_roles(state: &AppState) {
    state.mirror.upsert(envelope(
        "Role",
        "developer",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace", "Endpoint", "Pipeline"],
                            "verbs": ["read", "propose"] }] }),
    ));
    for (name, project) in [("dev-here", PROJECT), ("dev-there", ELSEWHERE)] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            name,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "group": "devs" }], "role": "developer",
                    "scope": { "project": project } }),
        ));
    }
}

async fn opened(state: &AppState, name: &str, project: &str, owner: &str) {
    state
        .workspaces
        .create(Opening {
            name,
            title: Some(name),
            project,
            owner,
            base_revision: "base1",
            scope: Scope::Project {},
            ttl_hours: 24,
        })
        .await
        .expect("the workspace is opened");
}

async fn previewing(state: &AppState, name: &str) {
    state
        .workspaces
        .set_preview_state(name, PreviewState::Running)
        .await
        .expect("the preview state is written");
}

fn message_of(err: ApiError) -> String {
    match err {
        ApiError::Conflict(message) => message,
        ApiError::Denied(message) => message,
        ApiError::NotFound(message) => message,
        ApiError::Unavailable(message) => message,
        other => panic!("the refusal was {other:?}"),
    }
}

// -------------------------------------------------------------------------------------------------
// T-2294: what a full node may say
// -------------------------------------------------------------------------------------------------

/// PF-59, R20: the node's two slots may be held by work the caller knows nothing about. The refusal
/// says the node is full and, of the running previews, names only the ones this caller may see.
#[tokio::test]
async fn a_full_node_names_only_the_previews_this_caller_may_see() {
    let gitea = MockServer::start().await;
    let state = state_on(&gitea);
    with_roles(&state);

    // Two previews of another project, owned by somebody else. The caller has a binding there, but
    // `visible` is about the project of the request: a preview of `doprava` is not this call's.
    opened(&state, "tender-2027", ELSEWHERE, "stefan").await;
    opened(&state, "air-secret", ELSEWHERE, "stefan").await;
    previewing(&state, "tender-2027").await;
    previewing(&state, "air-secret").await;
    opened(&state, "mine", PROJECT, "jana").await;

    let message = message_of(
        previews::start(&everywhere(), &state, PROJECT, "mine")
            .await
            .expect_err("a full node started a third preview"),
    );
    for hidden in ["tender-2027", "air-secret", "stefan"] {
        assert!(
            !message.contains(hidden),
            "the refusal named {hidden:?}, which this call may not see: {message}",
        );
    }
    assert!(
        message.contains("2") && message.to_lowercase().contains("preview"),
        "the refusal no longer says the node is full: {message}",
    );
}

/// CC-78: when the previews holding the node are the caller's own, the refusal names them — that
/// sentence is the one thing they can act on.
#[tokio::test]
async fn a_full_node_names_the_callers_own_previews_so_they_know_what_to_stop() {
    let gitea = MockServer::start().await;
    let state = state_on(&gitea);
    with_roles(&state);
    opened(&state, "air-v2", PROJECT, "jana").await;
    opened(&state, "bikes-v2", PROJECT, "jana").await;
    previewing(&state, "air-v2").await;
    previewing(&state, "bikes-v2").await;
    opened(&state, "third", PROJECT, "jana").await;

    let message = message_of(
        previews::start(&everywhere(), &state, PROJECT, "third")
            .await
            .expect_err("a full node started a third preview"),
    );
    assert!(message.contains("air-v2"), "{message}");
    assert!(message.contains("bikes-v2"), "{message}");
    assert!(
        message.contains("stop"),
        "the refusal says nothing to do: {message}"
    );
}

/// PF-59: one slot of this project and one elsewhere — the caller reads their own and a count of the
/// rest, never a name they have no business with.
#[tokio::test]
async fn a_full_node_counts_what_it_may_not_name() {
    let gitea = MockServer::start().await;
    let state = state_on(&gitea);
    with_roles(&state);
    opened(&state, "air-v2", PROJECT, "jana").await;
    opened(&state, "tender-2027", ELSEWHERE, "stefan").await;
    previewing(&state, "air-v2").await;
    previewing(&state, "tender-2027").await;
    opened(&state, "third", PROJECT, "jana").await;

    let message = message_of(
        previews::start(&everywhere(), &state, PROJECT, "third")
            .await
            .expect_err("a full node started a third preview"),
    );
    assert!(message.contains("air-v2"), "{message}");
    assert!(
        !message.contains("tender-2027"),
        "the refusal named another project's workspace: {message}",
    );
}

// -------------------------------------------------------------------------------------------------
// T-2129: who may start one, and when
// -------------------------------------------------------------------------------------------------

/// API/01 §22: only the owner starts a preview. Somebody else's workspace of this project is refused
/// with the owner's name — they can see it, so they can be told whose it is — and a workspace of
/// another project is answered as a workspace that is not there.
#[tokio::test]
async fn only_the_owner_starts_a_preview_and_another_projects_is_not_there() {
    let gitea = MockServer::start().await;
    let state = state_on(&gitea);
    with_roles(&state);
    opened(&state, "stefans", PROJECT, "stefan").await;
    opened(&state, "elsewhere", ELSEWHERE, "jana").await;

    let refused = previews::start(&everywhere(), &state, PROJECT, "stefans")
        .await
        .expect_err("somebody else's preview started");
    assert!(matches!(refused, ApiError::Denied(_)), "{refused:?}");
    assert!(message_of(refused).contains("stefan"));

    // The workspace exists, in another project, and this project's door says only that it has none.
    let missing = previews::start(&everywhere(), &state, PROJECT, "elsewhere")
        .await
        .expect_err("a workspace of another project started");
    assert!(matches!(missing, ApiError::NotFound(_)), "{missing:?}");
    let message = message_of(missing);
    assert!(
        !message.contains(ELSEWHERE),
        "the answer said which project it is in: {message}",
    );

    // A name nobody opened reads the same way.
    let never = previews::start(&everywhere(), &state, PROJECT, "never-opened")
        .await
        .expect_err("a preview of nothing started");
    assert!(matches!(never, ApiError::NotFound(_)), "{never:?}");
}

/// CC-78: one preview per workspace. A second start while it is starting or running is a conflict
/// that names the workspace, and the state is not touched by the refusal.
#[tokio::test]
async fn a_preview_that_already_runs_is_not_started_again() {
    let gitea = MockServer::start().await;
    let state = state_on(&gitea);
    with_roles(&state);
    opened(&state, "air-v2", PROJECT, "jana").await;

    for state_now in [PreviewState::Starting, PreviewState::Running] {
        state
            .workspaces
            .set_preview_state("air-v2", state_now)
            .await
            .expect("the preview state is written");
        let refused = previews::start(&everywhere(), &state, PROJECT, "air-v2")
            .await
            .expect_err("a running preview started again");
        assert!(
            matches!(refused, ApiError::Conflict(_)),
            "{state_now:?} {refused:?}"
        );
        assert!(message_of(refused).contains("air-v2"));
        assert_eq!(
            state
                .workspaces
                .get("air-v2")
                .await
                .expect("the store")
                .expect("the workspace")
                .preview_state,
            state_now,
            "the refusal changed the preview's state",
        );
    }
}

// Struck as impossible to drive from here: the slot count skips a preview whose workspace has
// expired (`other.expired(now)`, `src/ops/previews.rs:266`), but a workspace lives at least one hour
// (`create` refuses `ttl_hours` below 1, `src/ops/workspaces.rs:228`) and `start` reads the clock
// itself, so a test cannot age one without sleeping for an hour. The reaper's own suite covers the
// slot being freed (`tests/workspace_reaper_tests.rs`,
// `a_copy_whose_preview_is_running_is_reaped_too_and_its_slot_freed`).

/// CC-78: a render that fails leaves the preview in `Error` rather than in `Starting`, so the UI shows
/// what happened instead of a spinner nothing will finish, and the answer says what is wrong without
/// a stack or an internal address.
#[tokio::test]
async fn a_render_that_fails_leaves_the_preview_in_error_and_says_why() {
    let state = AppState::new(joinedcontext_portal::config::Config::for_tests(), None);
    with_roles(&state);
    opened(&state, "air-v2", PROJECT, "jana").await;

    // No forge is configured at all: the render cannot start, which is the shape of every failure.
    let refused = previews::start(&everywhere(), &state, PROJECT, "air-v2")
        .await
        .expect_err("a preview rendered without a forge");
    let message = message_of(refused);
    assert!(
        message.contains("forge"),
        "the answer does not say what is missing: {message}",
    );
    for internal in ["panicked", "/tmp/", "src/", "sqlx", "Bearer"] {
        assert!(
            !message.contains(internal),
            "the answer carried {internal:?}: {message}",
        );
    }
    assert_eq!(
        state
            .workspaces
            .get("air-v2")
            .await
            .expect("the store")
            .expect("the workspace")
            .preview_state,
        PreviewState::Error,
        "a failed render left the preview where a spinner never ends",
    );
}
