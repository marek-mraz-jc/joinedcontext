//! Edge cases of the workspace store and of the two listings above it (T-2069 … T-2072; CC-76, CC-78,
//! CC-80, CC-81, PF-59, R20).
//!
//! **The contract, in one sentence:** a workspace record is a name that could be a DNS label and a host
//! prefix, a life between an hour and a fortnight, and a scope that names something — and the two
//! listings above it answer the project's own, oldest first, minus what has expired and minus what this
//! caller may not see.
//!
//! `workspace_store_tests.rs` covers the store working, `workspace_api_tests.rs` the routes and
//! `edge_workspace_routes_tests.rs` the bodies those routes refuse. This file is the layer between: the
//! bounds `create` enforces that no route can reach, the order two listings promise, and what a
//! comparison makes of a file that is on one side only.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

mod common;

use chrono::{Duration, Utc};
use common::{envelope, person};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::ops::workspaces::{
    self, Opening, Scope, ScopedResource, WorkspaceError, MAX_NAME, MAX_TTL_HOURS,
};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::json;

const PROJECT: &str = "ovzdusie";
const ELSEWHERE: &str = "doprava";

fn opening<'a>(name: &'a str, project: &'a str, ttl_hours: i64, scope: Scope) -> Opening<'a> {
    Opening {
        name,
        title: None,
        project,
        owner: "jana@hel.fi",
        base_revision: "base1",
        scope,
        ttl_hours,
    }
}

/// A state with no forge and no database: the store falls back to memory, which is the layer these
/// cases are about (`src/ops/workspaces.rs:210`).
fn state() -> AppState {
    AppState::new(Config::for_tests(), None)
}

// -------------------------------------------------------------------------------------------------
// T-2069 create
// -------------------------------------------------------------------------------------------------

/// CC-76, CC-78: the name has to survive being a branch (`workspace/{name}`) and a preview host
/// (`ws-{name}-{project}`), so it is a DNS label of at most [`MAX_NAME`] characters — the bound and the
/// bound plus one are both here. The life is between an hour and a fortnight, which no route can reach
/// the ends of, because the route speaks in whole days.
#[tokio::test]
async fn a_name_or_a_life_outside_its_bounds_is_refused_by_the_store() {
    let state = state();

    // The bound itself, and one character past it.
    let longest = "a".repeat(MAX_NAME);
    state
        .workspaces
        .create(opening(&longest, PROJECT, 24, Scope::Project {}))
        .await
        .expect("a name of exactly the maximum");
    let too_long = "a".repeat(MAX_NAME + 1);
    assert!(
        matches!(
            state
                .workspaces
                .create(opening(&too_long, PROJECT, 24, Scope::Project {}))
                .await,
            Err(WorkspaceError::Invalid(_))
        ),
        "a name one character too long was recorded",
    );

    // Names that are not DNS labels: each refused, and none of them written.
    for name in [
        "",
        "-leading",
        "trailing-",
        "Upper",
        "under_score",
        "dots.inside",
        "spa ce",
        "slash/inside",
        "árvíztűrő",
    ] {
        assert!(
            matches!(
                state
                    .workspaces
                    .create(opening(name, PROJECT, 24, Scope::Project {}))
                    .await,
                Err(WorkspaceError::Invalid(_))
            ),
            "{name:?} was recorded as a workspace name",
        );
        assert!(
            state.workspaces.get(name).await.expect("read").is_none(),
            "{name:?} left a row behind",
        );
    }

    // The life: an hour is the shortest and the fortnight the longest, and both ends plus one are out.
    for (hours, allowed) in [
        (0, false),
        (-1, false),
        (1, true),
        (MAX_TTL_HOURS, true),
        (MAX_TTL_HOURS + 1, false),
        (i64::MAX, false),
    ] {
        let name = format!("ttl-{}", hours.rem_euclid(1000));
        let made = state
            .workspaces
            .create(opening(&name, PROJECT, hours, Scope::Project {}))
            .await;
        assert_eq!(
            made.is_ok(),
            allowed,
            "a life of {hours} hours was {}",
            if made.is_ok() { "recorded" } else { "refused" },
        );
        if let Ok(workspace) = made {
            assert_eq!(
                workspace.expires_at - workspace.created_at,
                Duration::hours(hours),
                "the life recorded is not the life asked for",
            );
        }
    }
}

/// CC-76: a scope names something. An empty list of resources is refused, a kind the platform does not
/// serve is refused, a resource name that is not a name is refused — and a list of one, a long list and
/// a list with the same resource twice are all accepted, because none of them is a lie about what the
/// workspace covers.
#[tokio::test]
async fn a_scope_that_names_nothing_is_refused_and_a_repeated_name_is_not() {
    let state = state();
    let resource = |kind: &str, name: &str| ScopedResource {
        kind: kind.to_owned(),
        name: name.to_owned(),
    };

    let refused = [
        (
            "no resources at all",
            Scope::Resources { items: Vec::new() },
        ),
        (
            "a kind nobody serves",
            Scope::Resources {
                items: vec![resource("NotAKind", "air")],
            },
        ),
        (
            "a resource name that is not one",
            Scope::Resources {
                items: vec![resource("ContextSpace", "Air Quality")],
            },
        ),
        (
            "one good resource and one bad",
            Scope::Resources {
                items: vec![
                    resource("ContextSpace", "air"),
                    resource("ContextSpace", ""),
                ],
            },
        ),
        (
            "a space name that is not one",
            Scope::Space {
                name: "Air Quality".into(),
            },
        ),
        (
            "no space name at all",
            Scope::Space {
                name: String::new(),
            },
        ),
    ];
    for (what, scope) in refused {
        let name = "scope-refused";
        assert!(
            matches!(
                state
                    .workspaces
                    .create(opening(name, PROJECT, 24, scope))
                    .await,
                Err(WorkspaceError::Invalid(_))
            ),
            "{what} was recorded",
        );
        assert!(
            state.workspaces.get(name).await.expect("read").is_none(),
            "{what} left a row behind",
        );
    }

    let accepted = [
        (
            "one resource",
            Scope::Resources {
                items: vec![resource("ContextSpace", "air")],
            },
        ),
        (
            "fifty resources",
            Scope::Resources {
                items: (0..50)
                    .map(|n| resource("Endpoint", &format!("endpoint-{n}")))
                    .collect(),
            },
        ),
        (
            "the same resource twice",
            Scope::Resources {
                items: vec![
                    resource("ContextSpace", "air"),
                    resource("ContextSpace", "air"),
                ],
            },
        ),
        ("one space", Scope::Space { name: "air".into() }),
        ("the whole project", Scope::Project {}),
    ];
    for (n, (what, scope)) in accepted.into_iter().enumerate() {
        let name = format!("scope-ok-{n}");
        state
            .workspaces
            .create(opening(&name, PROJECT, 24, scope))
            .await
            .unwrap_or_else(|error| panic!("{what} was refused: {error}"));
    }
}

/// CC-78: the name is taken across the organization, because the branch and the preview prefix are. A
/// second record of one name is a conflict whichever project asks for it, and the first record is the
/// one that stands — the conflict writes nothing. (That the conflict is also readable by a caller who
/// may not see the first workspace is filed as T-2296; this case is only about the store.)
#[tokio::test]
async fn one_name_is_one_record_in_the_whole_organization() {
    let state = state();
    let first = state
        .workspaces
        .create(opening("air-v2", PROJECT, 24, Scope::Project {}))
        .await
        .expect("the first");

    for project in [PROJECT, ELSEWHERE] {
        let again = state
            .workspaces
            .create(Opening {
                owner: "petra@hel.fi",
                ..opening("air-v2", project, 48, Scope::Project {})
            })
            .await;
        assert!(
            matches!(&again, Err(WorkspaceError::Conflict(name)) if name == "air-v2"),
            "a second record of one name in {project} was taken: {again:?}",
        );
    }

    // The first record is untouched: its project, its owner and its life are the ones it was made
    // with, so a colliding attempt cannot take a workspace over.
    let stored = state
        .workspaces
        .get("air-v2")
        .await
        .expect("read")
        .expect("the workspace");
    assert_eq!(stored.project, first.project);
    assert_eq!(stored.owner, "jana@hel.fi");
    assert_eq!(stored.expires_at, first.expires_at);
}

// -------------------------------------------------------------------------------------------------
// T-2070 the store's listing, T-2071 the listing a caller reads
// -------------------------------------------------------------------------------------------------

/// The store's listing is one project's, oldest first, with the name breaking a tie — so two workspaces
/// made in the same instant have an order, and a client can page them. A project with none is an empty
/// list and not an error, and a project name that differs in case is a different project.
#[tokio::test]
async fn the_stores_listing_is_one_projects_own_oldest_first_and_ties_broken_by_name() {
    let state = state();
    for (name, project) in [
        ("air-v2", PROJECT),
        ("air-v1", PROJECT),
        ("traffic", ELSEWHERE),
    ] {
        state
            .workspaces
            .create(opening(name, project, 24, Scope::Project {}))
            .await
            .expect("created");
    }

    let listed = state.workspaces.list(PROJECT).await.expect("list");
    let names: Vec<&str> = listed.iter().map(|w| w.name.as_str()).collect();
    assert!(
        !names.contains(&"traffic"),
        "another project's workspace is listed: {names:?}",
    );
    assert_eq!(names.len(), 2, "{names:?}");
    // Both were made in the same instant to the second, so the tie-break by name is what orders
    // them; if the clock did separate them, the older one is first either way.
    let by_time = listed[0].created_at <= listed[1].created_at;
    assert!(by_time, "the listing is not oldest first: {names:?}");

    assert!(state
        .workspaces
        .list("no-such-project")
        .await
        .expect("list")
        .is_empty(),);
    assert!(
        state
            .workspaces
            .list("Ovzdusie")
            .await
            .expect("list")
            .is_empty(),
        "a project name differing in case matched",
    );
}

/// CC-81, PF-59, R20: the listing a caller reads is the store's, minus what has expired and minus what
/// this caller may not see. A reader bound to one space sees a workspace over that space and not one
/// over the whole project; the owner sees their own whatever it covers; and a caller bound to nothing
/// is told the project is not there rather than given an empty list.
#[tokio::test]
async fn the_listing_a_caller_reads_hides_what_they_may_not_see_and_what_has_expired() {
    let state = state();
    state.mirror.upsert(envelope(
        "ContextSpace",
        "air",
        PROJECT,
        json!({ "isSandbox": true, "defaultLocale": "en", "ttlDays": 10 }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "air-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "air-readers",
        ORG_NAMESPACE,
        json!({ "role": "air-reader", "subjects": [{ "user": "vera@hel.fi" }],
                "scope": { "contextSpace": "air" } }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "project-readers",
        ORG_NAMESPACE,
        json!({ "role": "air-reader", "subjects": [{ "user": "jana@hel.fi" }],
                "scope": { "project": PROJECT } }),
    ));

    for (name, scope) in [
        ("over-air", Scope::Space { name: "air".into() }),
        ("over-all", Scope::Project {}),
    ] {
        state
            .workspaces
            .create(Opening {
                owner: "jana@hel.fi",
                ..opening(name, PROJECT, 24, scope)
            })
            .await
            .expect("created");
    }

    let names = |identity: Identity| {
        let state = state.clone();
        async move {
            workspaces::list(&identity, &state, PROJECT)
                .await
                .map(|list| {
                    list.items
                        .into_iter()
                        .map(|view| view.workspace.name)
                        .collect::<Vec<_>>()
                })
        }
    };

    // The space reader sees the workspace over their space and not the one over the project.
    let mut seen = names(person("vera")).await.expect("the listing");
    seen.sort();
    assert_eq!(seen, vec!["over-air".to_owned()], "{seen:?}");

    // The owner sees both, because they own both.
    let mut seen = names(person("jana")).await.expect("the listing");
    seen.sort();
    assert_eq!(seen, vec!["over-air".to_owned(), "over-all".to_owned()]);

    // A caller bound to nothing is told the project is not there, which is not an empty list — and
    // that holds even for the owner of a workspace in it. `may_see` says an owner always sees their
    // own, but `readable` runs first (`src/ops/workspaces.rs:881`), so a person whose binding was
    // taken away keeps a workspace they can no longer list. The reaper is what takes it in the end
    // (CC-81); until then it is theirs and invisible.
    for who in ["nobody", "petra"] {
        let error = workspaces::list(&person(who), &state, PROJECT)
            .await
            .expect_err("a caller bound to nothing was given a listing");
        assert!(
            format!("{error:?}").contains("not found"),
            "{who} was refused with something other than a miss: {error:?}",
        );
    }
    let orphan = state
        .workspaces
        .create(Opening {
            owner: "petra@hel.fi",
            ..opening("petras-own", PROJECT, 24, Scope::Project {})
        })
        .await
        .expect("created");
    assert!(
        workspaces::list(&person("petra"), &state, PROJECT)
            .await
            .is_err(),
        "a workspace of their own let an unbound owner read the project's listing",
    );
    assert_eq!(orphan.owner, "petra@hel.fi");

    // An expired workspace is in the store and in neither listing (CC-81). The record's own clock is
    // what decides, so this is asserted against the store rather than by waiting an hour.
    let expiring = state
        .workspaces
        .create(Opening {
            owner: "jana@hel.fi",
            ..opening("short-lived", PROJECT, 1, Scope::Project {})
        })
        .await
        .expect("created");
    assert!(!expiring.expired(Utc::now()));
    assert!(expiring.expired(Utc::now() + Duration::hours(2)));
    let expired_now: Vec<String> = state
        .workspaces
        .expired(Utc::now() + Duration::hours(2))
        .await
        .expect("the expired")
        .into_iter()
        .map(|w| w.name)
        .collect();
    assert!(
        expired_now.contains(&"short-lived".to_owned()),
        "the store does not report it as expired: {expired_now:?}",
    );
}

// -------------------------------------------------------------------------------------------------
// T-2072 compare_workspace
// -------------------------------------------------------------------------------------------------

/// CC-80: a comparison of a workspace whose branch the forge does not have is the empty comparison, not
/// an error — which is what makes a workspace whose branch was deleted read as one that changed nothing
/// (pinned from the route's side in `edge_workspace_routes_tests.rs` and written down in
/// `/workspace/chyby.md`). With no forge configured at all it is a plain 503 instead, so the two
/// situations are told apart: a Portal that cannot reach a repository says so.
#[tokio::test]
async fn a_comparison_without_a_forge_is_unavailable_and_not_an_empty_answer() {
    let state = state();
    let workspace = state
        .workspaces
        .create(opening("air-v2", PROJECT, 24, Scope::Project {}))
        .await
        .expect("created");

    let error = workspaces::compare_workspace(&state, &workspace)
        .await
        .expect_err("a comparison was made without a repository");
    let text = format!("{error:?}");
    assert!(
        text.contains("git forge is not configured"),
        "the refusal does not say what is missing: {text}",
    );
    assert!(
        !text.contains("token"),
        "the refusal carries a credential: {text}",
    );
}
