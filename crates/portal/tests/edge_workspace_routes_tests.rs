//! Edge cases of the seven workspace routes (T-2046 … T-2052; PF-51, PF-57, PF-59, AG-59, R20,
//! API/01 §22).
//!
//! **The contract, in one sentence:** a workspace is opened by somebody who may propose in the project,
//! seen by whoever may read what it covers, and changed, brought back or thrown away by nobody but its
//! owner — so a caller who may not see one is told it is not there, and a request that could not be
//! honoured is refused before a branch is cut.
//!
//! The happy paths live in `workspace_api_tests.rs` (open, list, read, compare, update, propose,
//! discard), the visibility rules in `workspace_security_tests.rs` (who sees whose) and the writes
//! inside a workspace in `workspace_propose_tests.rs`. This file is what those leave: the bounds of the
//! opening request, the order the checks run in, and the two places where the name of a workspace in
//! another project comes back to a caller who may not read that project.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

mod common;

use axum::http::StatusCode;
use common::{envelope, forge, person, send, state_on, REPO};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::ops::workspaces::{Opening, Scope, MAX_NAME, MAX_TITLE, MAX_TTL_HOURS};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "ovzdusie";
const ELSEWHERE: &str = "doprava";
const WS: &str = "/api/v1/projects/ovzdusie/workspaces";
const ELSEWHERE_WS: &str = "/api/v1/projects/doprava/workspaces";

/// Reads and proposes everywhere: the bootstrap group, which is how the first binding is written.
fn steward() -> Identity {
    Identity {
        client: None,
        groups: vec!["portal-approver".into()],
        ..person("jana")
    }
}

/// Reads `ovzdusie` and proposes nothing.
fn viewer() -> Identity {
    Identity {
        client: None,
        groups: vec!["readers".into()],
        ..person("vera")
    }
}

/// Proposes in `ovzdusie` and reads nothing of any other project: the caller a narrowed answer is
/// for, because they may open a workspace and may see none of `doprava`'s.
fn proposer() -> Identity {
    Identity {
        client: None,
        groups: vec!["editors".into()],
        ..person("emil")
    }
}

/// Bound to nothing at all.
fn stranger() -> Identity {
    person("nobody")
}

/// A forge that answers main's head, and a mirror with one space and the viewer's read-only binding.
async fn world() -> (MockServer, AppState) {
    let gitea = forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/branches/main")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": "base1" } })),
        )
        .mount(&gitea)
        .await;
    let state = state_on(&gitea);
    state.mirror.upsert(envelope(
        "ContextSpace",
        "air",
        PROJECT,
        json!({ "isSandbox": true, "defaultLocale": "en", "ttlDays": 10 }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "readers",
        ORG_NAMESPACE,
        json!({ "role": "reader", "subjects": [{ "group": "readers" }],
                "scope": { "project": PROJECT } }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "editor",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "editors",
        ORG_NAMESPACE,
        json!({ "role": "editor", "subjects": [{ "group": "editors" }],
                "scope": { "project": PROJECT } }),
    ));
    (gitea, state)
}

/// The workspace `name` of `project`, owned by `owner`, opened at `base1`.
async fn opened(state: &AppState, project: &str, name: &str, owner: &str) {
    state
        .workspaces
        .create(Opening {
            name,
            title: Some("Air"),
            project,
            owner,
            base_revision: "base1",
            scope: Scope::Project {},
            ttl_hours: 24,
        })
        .await
        .expect("opened");
}

/// Everything the forge was asked, as `VERB /path`.
async fn touched(gitea: &MockServer) -> Vec<String> {
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect()
}

fn opening(name: &str) -> Value {
    json!({ "name": name })
}

// -------------------------------------------------------------------------------------------------
// T-2046 open_workspace
// -------------------------------------------------------------------------------------------------

/// PF-59, R20: a project the caller reads nothing of is not there, and a project they read but cannot
/// propose in refuses the opening by name. The two answers are different on purpose — the first hides
/// the project, the second tells a member what they are missing — and neither cuts a branch.
#[tokio::test]
async fn opening_hides_a_project_from_a_stranger_and_names_the_grant_to_a_viewer() {
    let (gitea, state) = world().await;

    for (who, uri, expected) in [
        (stranger(), WS, StatusCode::NOT_FOUND),
        (viewer(), WS, StatusCode::FORBIDDEN),
        // The viewer's binding is `ovzdusie` only, so another project is not there for them either.
        (viewer(), ELSEWHERE_WS, StatusCode::NOT_FOUND),
        (stranger(), ELSEWHERE_WS, StatusCode::NOT_FOUND),
    ] {
        let answer = send(&state, who.clone(), "POST", uri, Some(opening("air-v2"))).await;
        assert_eq!(answer.status, expected, "{uri}: {}", answer.text);
        assert!(
            !answer.text.contains("air-v2"),
            "{uri} answered with the name the caller asked for: {}",
            answer.text,
        );
    }

    let writes: Vec<String> = touched(&gitea)
        .await
        .into_iter()
        .filter(|call| !call.starts_with("GET "))
        .collect();
    assert!(writes.is_empty(), "the forge was asked to {writes:?}");
}

/// The bounds of an opening request, each refused as the caller's mistake: a title longer than the
/// maximum or carrying a control character, a TTL of zero, a negative one and one past the fortnight.
/// The fortnight itself and a single day are accepted, so the bound and the bound plus one are both
/// proved.
#[tokio::test]
async fn a_title_or_a_ttl_outside_its_bounds_is_refused_and_the_bound_itself_is_not() {
    let (_gitea, state) = world().await;
    let max_days = MAX_TTL_HOURS / 24;

    let refused: Vec<(&str, Value)> = vec![
        (
            "a title one character too long",
            json!({ "name": "t1", "title": "x".repeat(MAX_TITLE + 1) }),
        ),
        (
            "a title with a newline",
            json!({ "name": "t2", "title": "two\nlines" }),
        ),
        (
            "a title with a nul",
            json!({ "name": "t3", "title": "nul\u{0}inside" }),
        ),
        (
            "a title with a carriage return, which would reach a log",
            json!({ "name": "t4", "title": "air\r\nSet-Cookie: x=y" }),
        ),
        ("no days at all", json!({ "name": "t5", "ttlDays": 0 })),
        ("a negative TTL", json!({ "name": "t6", "ttlDays": -1 })),
        (
            "a day past the fortnight",
            json!({ "name": "t7", "ttlDays": max_days + 1 }),
        ),
        (
            "a TTL nobody could serve",
            json!({ "name": "t8", "ttlDays": i64::MAX }),
        ),
    ];
    for (what, body) in refused {
        let answer = send(&state, steward(), "POST", WS, Some(body)).await;
        assert_eq!(
            answer.status,
            StatusCode::BAD_REQUEST,
            "{what}: {}",
            answer.text
        );
    }

    // The bound itself, the shortest life, and a title of exactly the maximum: all opened.
    for (what, body) in [
        (
            "the fortnight",
            json!({ "name": "ok-14", "ttlDays": max_days }),
        ),
        ("one day", json!({ "name": "ok-1", "ttlDays": 1 })),
        (
            "a title of exactly the maximum",
            json!({ "name": "ok-title", "title": "x".repeat(MAX_TITLE) }),
        ),
        // Trimmed to nothing is the same as absent, not a title of spaces.
        (
            "a title of spaces",
            json!({ "name": "ok-blank", "title": "   " }),
        ),
    ] {
        let answer = send(&state, steward(), "POST", WS, Some(body)).await;
        assert_eq!(
            answer.status,
            StatusCode::CREATED,
            "{what}: {}",
            answer.text
        );
    }
}

/// A body that is not an opening request is refused by the extractor, before any of it is believed: an
/// unknown field, a name that is not a string, a missing name, a scope of a shape the platform does not
/// have. `deny_unknown_fields` is what makes the first one a refusal rather than a silent ignore
/// (`src/ops/workspaces.rs:535`).
#[tokio::test]
async fn a_body_that_is_not_an_opening_request_is_refused_by_the_extractor() {
    let (gitea, state) = world().await;

    let bad_bodies = [
        json!({ "name": "air-v2", "ttl_days": 3 }),
        json!({ "name": "air-v2", "scope": { "kind": "everything" } }),
        json!({ "name": "air-v2", "scope": { "kind": "space" } }),
        json!({ "name": "air-v2", "scope": "air" }),
        json!({ "name": 7 }),
        json!({ "name": ["air-v2"] }),
        json!({ "title": "no name" }),
        json!({}),
        json!([]),
        json!("air-v2"),
        Value::Null,
    ];
    for body in bad_bodies {
        let answer = send(&state, steward(), "POST", WS, Some(body.clone())).await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {}",
            answer.text
        );
    }

    let writes: Vec<String> = touched(&gitea)
        .await
        .into_iter()
        .filter(|call| !call.starts_with("GET "))
        .collect();
    assert!(writes.is_empty(), "the forge was asked to {writes:?}");
}

/// A name or a scope the store refuses costs the forge two reads first.
///
/// Today's behaviour, not the wanted one: `open` checks the caller, the title and the TTL itself, then
/// reads main's name and head from the forge, and only then calls `create`, which is where the name's
/// length and shape and the scope's contents are checked (`src/ops/workspaces.rs:844`). So a request
/// that could have been refused from its own body alone reaches the forge twice. Written down in
/// `/workspace/chyby.md`; nothing is written, and the answer is still the caller's 400, so what this
/// case pins is the order rather than a wrong answer.
#[tokio::test]
async fn a_name_the_store_refuses_is_read_from_the_forge_before_it_is_refused() {
    let (gitea, state) = world().await;

    let refused = [
        json!({ "name": "x".repeat(MAX_NAME + 1) }),
        json!({ "name": "Air-V2" }),
        json!({ "name": "air_v2" }),
        json!({ "name": "-air" }),
        json!({ "name": "" }),
        json!({ "name": "air v2" }),
        json!({ "name": "air-v2", "scope": { "kind": "resources", "items": [] } }),
        json!({ "name": "air-v2", "scope": { "kind": "resources",
                "items": [{ "kind": "NotAKind", "name": "air" }] }}),
        json!({ "name": "air-v2", "scope": { "kind": "space", "name": "Air" } }),
    ];
    for body in refused {
        let answer = send(&state, steward(), "POST", WS, Some(body.clone())).await;
        assert_eq!(
            answer.status,
            StatusCode::BAD_REQUEST,
            "{body}: {}",
            answer.text
        );
    }

    // The two reads that happened before each of those refusals, and no write.
    let calls = touched(&gitea).await;
    assert!(
        calls.iter().any(|call| call.ends_with("/branches/main")),
        "the forge was not read at all, so the order has changed: {calls:?}",
    );
    let writes: Vec<String> = calls
        .into_iter()
        .filter(|call| !call.starts_with("GET "))
        .collect();
    assert!(writes.is_empty(), "the forge was asked to {writes:?}");
}

/// PF-59, R20: a workspace name is taken across the whole organization, and a caller who may not see
/// what holds it is told the name is not free without being told whose it is.
///
/// The record is keyed by name alone, because the branch and the preview host are
/// (`src/ops/workspaces.rs:222`, CC-76, CC-78). The uniqueness is wanted; naming it to everyone was
/// not — one request per guessed name told anybody who may propose in any project whether a name is
/// in use in every other. The status stays 409 either way: the request cannot be honoured and the
/// person has to pick another name, which is not a leak and is the thing they act on (T-2296).
#[tokio::test]
async fn a_name_held_out_of_sight_is_taken_without_being_named() {
    let (_gitea, state) = world().await;
    opened(&state, ELSEWHERE, "air-v2", "peter@hel.fi").await;

    // The steward reads every project, so for them the conflict is the plain one: they can go and
    // look at what holds the name.
    let answer = send(&state, steward(), "POST", WS, Some(opening("air-v2"))).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(
        answer.text.contains("air-v2"),
        "the conflict does not name what is taken: {}",
        answer.text,
    );

    // The proposer opens workspaces in `ovzdusie` and reads nothing of `doprava`. Same 409, and the
    // body names neither the workspace, nor the project that holds it, nor its owner.
    let answer = send(&state, proposer(), "POST", WS, Some(opening("air-v2"))).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    for secret in ["air-v2", ELSEWHERE, "peter@hel.fi"] {
        assert!(
            !answer.text.contains(secret),
            "the refusal carries '{secret}': {}",
            answer.text,
        );
    }
    let problem: Value = serde_json::from_str(&answer.text).expect("a problem");
    let detail = problem["detail"].as_str().expect("a detail").to_owned();
    assert!(
        detail.contains("not available") && detail.contains("choose another"),
        "the refusal does not tell the person what to do: {detail}",
    );

    // A name held in their own project is still named plainly, because that is the conflict a person
    // acts on — they can open what holds it, or ask its owner.
    opened(&state, PROJECT, "air-v3", "emil@hel.fi").await;
    let answer = send(&state, proposer(), "POST", WS, Some(opening("air-v3"))).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(
        answer.text.contains("air-v3"),
        "the conflict in the caller's own project went vague: {}",
        answer.text,
    );
    assert_ne!(
        serde_json::from_str::<Value>(&answer.text).expect("a problem")["detail"],
        json!(detail),
        "the two conflicts have become one wording",
    );

    // A project the viewer does not read is hidden before the name is looked at at all.
    let answer = send(
        &state,
        viewer(),
        "POST",
        ELSEWHERE_WS,
        Some(opening("air-v2")),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::NOT_FOUND,
        "a project the viewer does not read is hidden before the name is looked at: {}",
        answer.text,
    );
}

// -------------------------------------------------------------------------------------------------
// T-2047 list_workspaces, T-2048 get_workspace, T-2049 compare_workspace
// -------------------------------------------------------------------------------------------------

/// PF-59, R20: the listing of a project the caller reads nothing of is not an empty list but a project
/// that is not there, and the name in the path is matched exactly — another case, a trailing space or a
/// walk upwards is a different project, which is to say none.
#[tokio::test]
async fn a_listing_of_a_project_nobody_bound_the_caller_to_is_not_an_empty_list() {
    let (_gitea, state) = world().await;
    opened(&state, PROJECT, "air-v2", "jana@hel.fi").await;

    for (who, uri) in [
        (stranger(), WS),
        (viewer(), ELSEWHERE_WS),
        (viewer(), "/api/v1/projects/Ovzdusie/workspaces"),
        (viewer(), "/api/v1/projects/ovzdusie%20/workspaces"),
        (viewer(), "/api/v1/projects/%2e%2e/workspaces"),
    ] {
        let answer = send(&state, who.clone(), "GET", uri, None).await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{uri}: {}",
            answer.text
        );
        assert!(
            !answer.text.contains("air-v2"),
            "{uri} answered with a workspace: {}",
            answer.text,
        );
    }

    // The viewer reads the project, so they read its listing; the workspace covers the whole project
    // and their binding does too, so they see it without owning it.
    let answer = send(&state, viewer(), "GET", WS, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let list: Value = serde_json::from_str(&answer.text).expect("a list");
    assert_eq!(list["items"][0]["name"], json!("air-v2"), "{}", answer.text);
}

/// PF-59, R20: reading or comparing a workspace of another project is a 404, and so is one nobody
/// has — in one and the same sentence.
///
/// The two used to be worded apart: `no workspace named 'x'` for a name nobody holds against
/// `no workspace named 'x' in project 'y'` for one held elsewhere. Both are 404, so the status was
/// right and the body was the oracle, readable by any project reader one name at a time. `visible`
/// now answers the second sentence for both misses (`src/ops/workspaces.rs:791`, T-2296).
#[tokio::test]
async fn reading_a_workspace_of_another_project_is_not_there_in_the_same_words_as_one_nobody_has() {
    let (_gitea, state) = world().await;
    opened(&state, ELSEWHERE, "air-v2", "peter@hel.fi").await;

    for suffix in ["", "/compare"] {
        // A name that exists in another project, asked for in this one.
        let answer = send(
            &state,
            viewer(),
            "GET",
            &format!("{WS}/air-v2{suffix}"),
            None,
        )
        .await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text);
        let there: Value = serde_json::from_str(&answer.text).expect("a problem");
        assert_eq!(
            there["detail"],
            json!(format!(
                "no workspace named 'air-v2' in project '{PROJECT}'"
            )),
            "{suffix}: {}",
            answer.text,
        );
        assert!(
            !answer.text.contains(ELSEWHERE) && !answer.text.contains("peter@hel.fi"),
            "{suffix}: the 404 says where the name lives: {}",
            answer.text,
        );

        // A name that exists nowhere: the same sentence, with the name the caller themselves typed.
        let answer = send(
            &state,
            viewer(),
            "GET",
            &format!("{WS}/nothing-at-all{suffix}"),
            None,
        )
        .await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text);
        let nowhere: Value = serde_json::from_str(&answer.text).expect("a problem");
        assert_eq!(
            nowhere["detail"],
            json!(format!(
                "no workspace named 'nothing-at-all' in project '{PROJECT}'"
            )),
            "{suffix}: {}",
            answer.text,
        );
        // And the two differ in nothing but the name that was asked for.
        assert_eq!(
            there["detail"]
                .as_str()
                .expect("a detail")
                .replace("air-v2", "NAME"),
            nowhere["detail"]
                .as_str()
                .expect("a detail")
                .replace("nothing-at-all", "NAME"),
            "{suffix}: the two misses are worded apart again: {} against {}",
            there["detail"],
            nowhere["detail"],
        );
    }

    // The workspace-scoped read of the resource API is the third way in: it resolves the name
    // through `mirror_of`, which carried the same two wordings (`src/ops/workspaces.rs:454`,
    // measured by worker-10 on T-1682). One sentence there too.
    let mut details = Vec::new();
    for name in ["air-v2", "nothing-at-all"] {
        let answer = send(
            &state,
            viewer(),
            "GET",
            &format!("/api/v1/projects/{PROJECT}/spaces?workspace={name}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{name}: {}",
            answer.text
        );
        let problem: Value = serde_json::from_str(&answer.text).expect("a problem");
        details.push(
            problem["detail"]
                .as_str()
                .expect("a detail")
                .replace(name, "NAME"),
        );
    }
    assert_eq!(
        details[0], details[1],
        "the workspace-scoped read still says which name lives in another project",
    );

    // And a stranger reads neither wording: the project is refused before the name is looked at.
    for uri in [
        format!("{WS}/air-v2"),
        format!("{WS}/nothing-at-all"),
        format!("{ELSEWHERE_WS}/air-v2"),
    ] {
        let answer = send(&state, stranger(), "GET", &uri, None).await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{uri}: {}",
            answer.text
        );
        assert!(answer.text.contains("not found"), "{uri}: {}", answer.text,);
        assert!(
            !answer.text.contains("no workspace named"),
            "{uri} told a stranger which workspaces there are: {}",
            answer.text,
        );
    }
}

/// A workspace whose branch is not in the forge reads as a workspace that changed nothing.
///
/// The listing answers from the record alone and asks the forge nothing, which is what makes it cheap
/// per workspace. Reading one counts the files it changes, so it asks for the branch's tree — and when
/// there is no such branch, `trees` answers `None` and the comparison is the empty one
/// (`src/ops/workspaces.rs:992`), so `changes` is `0` and the answer is a plain 200. A branch deleted
/// behind the Portal's back therefore reads as a workspace with nothing in it rather than as one that
/// cannot be brought back: today's behaviour, written down in `/workspace/chyby.md`.
#[tokio::test]
async fn a_workspace_whose_branch_is_gone_reads_as_one_that_changed_nothing() {
    // A forge with nothing mounted: every call answers 404, which is how a missing branch looks.
    let gitea = MockServer::start().await;
    let state = state_on(&gitea);
    opened(&state, PROJECT, "air-v2", "jana@hel.fi").await;

    let answer = send(&state, steward(), "GET", WS, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let list: Value = serde_json::from_str(&answer.text).expect("a list");
    assert_eq!(list["items"][0]["name"], json!("air-v2"), "{}", answer.text);
    assert!(
        list["items"][0]["changes"].is_null(),
        "the listing counted changes, which costs the forge a read per workspace: {}",
        answer.text,
    );
    assert!(
        touched(&gitea).await.is_empty(),
        "the listing asked the forge something",
    );

    // The same workspace, read and compared: 200 both times, and nothing changed.
    for (uri, at) in [
        (format!("{WS}/air-v2"), "changes"),
        (format!("{WS}/air-v2/compare"), "files"),
    ] {
        let answer = send(&state, steward(), "GET", &uri, None).await;
        assert_eq!(answer.status, StatusCode::OK, "{uri}: {}", answer.text);
        let view: Value = serde_json::from_str(&answer.text).expect("a view");
        let count = match at {
            "changes" => view["changes"].as_u64().unwrap_or_default(),
            _ => view["files"]
                .as_array()
                .map_or(0, |files| files.len() as u64),
        };
        assert_eq!(
            count, 0,
            "{uri} found changes without a branch: {}",
            answer.text
        );
        assert!(
            !answer.text.contains("token-xyz"),
            "the forge token is in the answer: {}",
            answer.text,
        );
    }

    // And the read did ask the forge for the branch, so the emptiness is the forge's answer and not
    // a shortcut taken before it.
    let calls = touched(&gitea).await;
    assert!(
        calls.iter().any(|call| call.contains("workspace/air-v2")),
        "the branch was never asked for: {calls:?}",
    );
}

// -------------------------------------------------------------------------------------------------
// T-2050 update_workspace, T-2051 propose_workspace, T-2052 discard_workspace
// -------------------------------------------------------------------------------------------------

/// API/01 §22: updating, bringing back and throwing away are the owner's, and nobody else's. A project
/// reader who can see the workspace is refused all three by name — they may see it, so the owner in the
/// answer is nothing they could not read in the listing — and a stranger is told it is not there.
/// Neither of them costs the forge a write, so a refusal never leaves a branch merged or deleted.
#[tokio::test]
async fn only_the_owner_updates_brings_back_or_throws_away_and_a_refusal_writes_nothing() {
    let (gitea, state) = world().await;
    opened(&state, PROJECT, "air-v2", "jana@hel.fi").await;

    let routes: Vec<(&str, String, Option<Value>)> = vec![
        ("POST", format!("{WS}/air-v2/update"), Some(json!({}))),
        ("POST", format!("{WS}/air-v2/propose"), None),
        ("DELETE", format!("{WS}/air-v2"), None),
    ];

    for (verb, uri, body) in &routes {
        let answer = send(&state, viewer(), verb, uri, body.clone()).await;
        assert_eq!(
            answer.status,
            StatusCode::FORBIDDEN,
            "the viewer on {verb} {uri}: {}",
            answer.text,
        );
        assert!(
            answer.text.contains("jana@hel.fi"),
            "the refusal does not say whose it is: {}",
            answer.text,
        );

        let answer = send(&state, stranger(), verb, uri, body.clone()).await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "a stranger on {verb} {uri}: {}",
            answer.text,
        );
        assert!(
            !answer.text.contains("jana@hel.fi"),
            "a stranger was told whose workspace it is: {}",
            answer.text,
        );
    }

    let writes: Vec<String> = touched(&gitea)
        .await
        .into_iter()
        .filter(|call| !call.starts_with("GET "))
        .collect();
    assert!(writes.is_empty(), "the forge was asked to {writes:?}");
}

/// The resolutions an update carries are answers to conflicts, and a body that is not a list of them is
/// refused before main is read: an unknown field, a side nobody offers, a resolution that is not an
/// object. An empty list is a valid request — it means there was nothing to answer — and `{}` is the
/// same request, because `resolutions` defaults.
#[tokio::test]
async fn an_update_body_that_is_not_a_list_of_resolutions_is_refused_before_main_is_read() {
    let (gitea, state) = world().await;
    opened(&state, PROJECT, "air-v2", "jana@hel.fi").await;
    let uri = format!("{WS}/air-v2/update");

    let refused = [
        json!({ "resolutions": [], "force": true }),
        json!({ "resolution": [] }),
        json!({ "resolutions": {} }),
        json!({ "resolutions": [{ "path": "a.yaml", "keep": "middle" }] }),
        json!({ "resolutions": [{ "path": "a.yaml" }] }),
        json!({ "resolutions": [{ "keep": "ours" }] }),
        json!({ "resolutions": [{ "path": "a.yaml", "keep": "ours", "why": "because" }] }),
        json!({ "resolutions": ["a.yaml"] }),
        json!("ours"),
    ];
    for body in refused {
        let answer = send(&state, steward(), "POST", &uri, Some(body.clone())).await;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {}",
            answer.text
        );
    }

    // Nothing of that reached the forge: the extractor answered every one of them.
    let calls = touched(&gitea).await;
    assert!(
        calls.is_empty(),
        "a body the extractor refuses reached the forge: {calls:?}",
    );

    // A resolution for a file no comparison lists is not a parse error; it is a request the owner
    // may make, and the forge is read to find out there is nothing to merge.
    let answer = send(
        &state,
        steward(),
        "POST",
        &uri,
        Some(json!({ "resolutions": [
            { "path": "projects/ovzdusie/spaces/air/space.yaml", "field": "spec.ttlDays",
              "keep": "ours" },
            { "path": "projects/ovzdusie/spaces/air/space.yaml", "field": "spec.ttlDays",
              "keep": "theirs" }
        ] })),
    )
    .await;
    assert_ne!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a duplicate resolution is the owner's business, not the extractor's: {}",
        answer.text,
    );
}

/// PF-51: bringing back or throwing away a workspace that is not there writes nothing, whatever the
/// name looks like — one nobody has, one that could not be a name, and one that was thrown away a
/// moment ago. The second discard is the same 404 as the first name, so a discard is not a way to ask
/// whether something was there.
#[tokio::test]
async fn a_second_discard_and_a_name_nobody_has_are_the_same_answer() {
    let (gitea, state) = world().await;
    opened(&state, PROJECT, "air-v2", "jana@hel.fi").await;

    // The first discard is the owner's and takes the branch with it.
    let answer = send(&state, steward(), "DELETE", &format!("{WS}/air-v2"), None).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.text);
    let deleted: Vec<String> = touched(&gitea)
        .await
        .into_iter()
        .filter(|call| call.starts_with("DELETE "))
        .collect();
    assert!(
        deleted.iter().any(|call| call.contains("workspace/air-v2")),
        "the branch was not deleted with the record: {deleted:?}",
    );

    // The second, and three names that were never workspaces: one answer for all of them.
    for name in ["air-v2", "nothing-at-all", "Air-V2", "%2e%2e"] {
        for (verb, uri) in [
            ("DELETE", format!("{WS}/{name}")),
            ("POST", format!("{WS}/{name}/propose")),
        ] {
            let answer = send(&state, steward(), verb, &uri, None).await;
            assert_eq!(
                answer.status,
                StatusCode::NOT_FOUND,
                "{verb} {uri}: {}",
                answer.text
            );
        }
    }

    // And nothing was deleted a second time.
    let deleted: Vec<String> = touched(&gitea)
        .await
        .into_iter()
        .filter(|call| call.starts_with("DELETE ") && call.contains("workspace/"))
        .collect();
    assert_eq!(
        deleted.len(),
        1,
        "a branch was deleted more than once: {deleted:?}",
    );
}
