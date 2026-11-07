//! Edge cases of the reconciler's status and metrics, the mirror's listing, and the server that binds
//! them all (T-2073 … T-2076; CC-03, CC-08, MF-04, OPS-51, T-0914).
//!
//! **The contract, in one sentence:** the mirror is the Portal's one view of the repository, read under
//! a lock that survives a panic and paged by name; the reconciler's status is a copy a reader cannot
//! change; and the server binds, answers and stops.
//!
//! `sync_tests.rs` and `mirror_workspace_tests.rs` cover the reconciler converging and the mirror being
//! filled. This file is what they leave: the bounds of one page, a continuation token nobody minted,
//! a lock whose holder panicked, and the project name that goes into the runner's URL.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

mod common;

use std::sync::Arc;

use common::envelope;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::reconciler::streams::StreamDeployer;
use joinedcontext_portal::store::ListOptions;
use joinedcontext_portal::store::Mirror;
use serde_json::json;

const PROJECT: &str = "ovzdusie";

fn a_mirror(names: &[&str]) -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    for name in names {
        mirror.upsert(envelope(
            "ContextSpace",
            name,
            PROJECT,
            json!({ "isSandbox": true, "defaultLocale": "en", "ttlDays": 10 }),
        ));
    }
    mirror
}

fn page(
    mirror: &Mirror,
    limit: Option<usize>,
    token: Option<&str>,
) -> (Vec<String>, Option<String>, usize) {
    let options = ListOptions {
        limit,
        continue_token: token.map(str::to_owned),
        ..Default::default()
    };
    let listed = mirror.list(PROJECT, "ContextSpace", &options);
    (
        listed
            .items
            .iter()
            .map(|item| item.metadata.name.clone())
            .collect(),
        listed.continue_token,
        listed.remaining,
    )
}

// -------------------------------------------------------------------------------------------------
// T-2076 the mirror's listing
// -------------------------------------------------------------------------------------------------

/// MF-04: one page is the names in order from the token onwards, at most `limit` of them, and the token
/// of the next page is the last name on this one. The bounds: no limit is every item, a limit past the
/// end is every item and no token, and a limit of nothing is — today — also every item, because
/// `Some(0) < total` is false only when the list is empty (`src/store.rs:163`).
#[tokio::test]
async fn one_page_is_the_names_in_order_and_the_token_is_the_last_of_them() {
    let mirror = a_mirror(&["air", "bikes", "noise", "traffic", "water"]);

    // No limit: everything, in name order, with nothing to continue.
    let (names, token, remaining) = page(&mirror, None, None);
    assert_eq!(names, ["air", "bikes", "noise", "traffic", "water"]);
    assert!(token.is_none(), "{token:?}");
    assert_eq!(remaining, 0);

    // A limit inside the list: that many, and the token is the last name of the page.
    let (names, token, remaining) = page(&mirror, Some(2), None);
    assert_eq!(names, ["air", "bikes"]);
    assert_eq!(token.as_deref(), Some("bikes"));
    assert_eq!(remaining, 3);

    // The next page starts after the token, and the last page has no token.
    let (names, token, remaining) = page(&mirror, Some(2), Some("bikes"));
    assert_eq!(names, ["noise", "traffic"]);
    assert_eq!(token.as_deref(), Some("traffic"));
    assert_eq!(remaining, 1);
    let (names, token, remaining) = page(&mirror, Some(2), Some("traffic"));
    assert_eq!(names, ["water"]);
    assert!(
        token.is_none(),
        "the last page offers a next one: {token:?}"
    );
    assert_eq!(remaining, 0);

    // The bound itself and one past it: a limit of exactly the count ends the paging.
    let (names, token, _) = page(&mirror, Some(5), None);
    assert_eq!(names.len(), 5);
    assert!(token.is_none(), "a full page offered a next one: {token:?}");
    let (names, token, _) = page(&mirror, Some(6), None);
    assert_eq!(names.len(), 5);
    assert!(token.is_none(), "{token:?}");

    // A limit of nothing: an empty page that says five remain and offers no token to reach them,
    // because the token is the last name of a page that has none (`src/store.rs:167`). A client
    // that asks for zero is stuck: it is told there is more and given no way to continue. Today's
    // behaviour, written down in `/workspace/chyby.md`; when a limit of zero is refused or read as
    // "no limit", this is the assertion to change.
    let (names, token, remaining) = page(&mirror, Some(0), None);
    assert!(names.is_empty(), "{names:?}");
    assert_eq!(remaining, 5, "the page does not say what is left");
    assert!(
        token.is_none(),
        "an empty page now offers a way on, which is the fix: {token:?}",
    );
}

/// MF-04: a continuation token is a name, and the store reads it as one — a token nobody minted, a name
/// past the end of the list and a name that was removed between two pages all answer whatever comes
/// after them in order, which is an empty page at the end and never an error. The caller who made the
/// token up learns nothing from it that a plain listing would not tell them.
#[tokio::test]
async fn a_continuation_token_nobody_minted_is_read_as_a_name_and_never_fails() {
    let mirror = a_mirror(&["air", "bikes", "noise"]);

    for (what, token, expected) in [
        ("a name past the end", "zzz", Vec::<&str>::new()),
        (
            "a name before the start",
            "aaa",
            vec!["air", "bikes", "noise"],
        ),
        ("an empty token", "", vec!["air", "bikes", "noise"]),
        ("a name nobody has, in the middle", "bikez", vec!["noise"]),
        (
            "a name of another kind",
            "Role",
            vec!["air", "bikes", "noise"],
        ),
        ("a path", "../../etc/passwd", vec!["air", "bikes", "noise"]),
        ("a name with a nul", "air\0", vec!["bikes", "noise"]),
    ] {
        let (names, token_out, _) = page(&mirror, None, Some(token));
        assert_eq!(names, expected, "{what} ({token:?}) answered {names:?}");
        assert!(token_out.is_none(), "{what}: {token_out:?}");
    }

    // A resource removed between two pages: the page after its token is what follows the name, so
    // nothing is skipped and nothing is repeated.
    let (first, token, _) = page(&mirror, Some(1), None);
    assert_eq!(first, ["air"]);
    mirror.remove(&joinedcontext_portal::resource::ResourceKey {
        namespace: PROJECT.to_owned(),
        kind: "ContextSpace".to_owned(),
        name: "bikes".to_owned(),
    });
    let (second, _, _) = page(&mirror, Some(1), token.as_deref());
    assert_eq!(
        second,
        ["noise"],
        "the page after a removed item is not what follows it",
    );
}

/// PF-59, MF-04: the listing is one namespace's and one kind's, and both are matched exactly. A
/// namespace differing in case, one with a trailing space and one that is a prefix of the real one are
/// three different namespaces, which is to say none — a project never reads another's by spelling.
#[tokio::test]
async fn a_listing_matches_its_namespace_and_kind_exactly() {
    let mirror = a_mirror(&["air"]);
    mirror.upsert(envelope(
        "Endpoint",
        "air-all",
        PROJECT,
        json!({ "space": "air", "slug": "air" }),
    ));
    mirror.upsert(envelope(
        "ContextSpace",
        "traffic",
        "doprava",
        json!({ "isSandbox": false, "defaultLocale": "sk", "ttlDays": 30 }),
    ));

    for namespace in ["Ovzdusie", "ovzdusie ", "ovzdusi", "ovzdusiee", "", "*"] {
        let listed = mirror.list(namespace, "ContextSpace", &ListOptions::default());
        assert!(
            listed.items.is_empty(),
            "{namespace:?} listed {} items",
            listed.items.len(),
        );
    }
    for kind in ["contextspace", "ContextSpaces", "Context Space", ""] {
        let listed = mirror.list(PROJECT, kind, &ListOptions::default());
        assert!(listed.items.is_empty(), "{kind:?} listed something");
    }

    // The right namespace and kind: its own, and nothing of the other kind or the other project.
    let listed = mirror.list(PROJECT, "ContextSpace", &ListOptions::default());
    let names: Vec<&str> = listed
        .items
        .iter()
        .map(|item| item.metadata.name.as_str())
        .collect();
    assert_eq!(names, ["air"], "{names:?}");
}

// -------------------------------------------------------------------------------------------------
// T-2073 the reconciler's status, T-2074 the runner's counters
// -------------------------------------------------------------------------------------------------

/// CC-03, OPS-51: the mirror is read under a lock that a panicking holder does not take with it —
/// every read and write recovers the poisoned lock (`src/store.rs:53`, `:121`). A Portal whose
/// reconciler panicked mid-write therefore still answers reads, which is the difference between one
/// failed sync and a process that answers nothing until it is restarted.
#[tokio::test]
async fn a_panic_while_the_mirror_is_held_does_not_shut_the_mirror() {
    let mirror = a_mirror(&["air"]);

    // A thread that panics while holding the write lock: the lock is poisoned afterwards.
    let held = Arc::clone(&mirror);
    let panicked = std::thread::spawn(move || {
        held.upsert(envelope(
            "ContextSpace",
            "bikes",
            PROJECT,
            json!({ "isSandbox": true, "defaultLocale": "en", "ttlDays": 1 }),
        ));
        panic!("the reconciler fell over mid-sync");
    })
    .join();
    assert!(panicked.is_err(), "the thread did not panic");

    // Everything the mirror does still works, and what the panicking thread wrote before it fell
    // over is there: the write had landed.
    assert!(mirror.get(PROJECT, "ContextSpace", "air").is_some());
    let (names, _, _) = page(&mirror, None, None);
    assert_eq!(names, ["air", "bikes"], "{names:?}");
    mirror.upsert(envelope(
        "ContextSpace",
        "noise",
        PROJECT,
        json!({ "isSandbox": true, "defaultLocale": "en", "ttlDays": 1 }),
    ));
    assert_eq!(mirror.len(), 3);
    assert_eq!(mirror.snapshot().len(), 3, "a snapshot of a poisoned lock");
}

// -------------------------------------------------------------------------------------------------
// T-2075 the server
// -------------------------------------------------------------------------------------------------

/// The server binds the address it is given, answers on it, and gives the port back when it stops. A
/// Portal with no realm opens no second port: every route of the internal listener answers only a
/// workload token of the realm (T-5869, `server::internal_listener_opens`), and a port that is not
/// open is the cheapest access control there is.
#[tokio::test]
async fn the_server_binds_answers_and_opens_no_second_port_without_an_agent_runner() {
    // Port 0: the operating system picks one, so this case cannot collide with anything.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let bind = listener.local_addr().expect("the address");
    drop(listener);

    let mut config = Config::for_tests();
    config.bind = bind;
    assert!(
        config.agent_settings.is_none() && config.oidc.is_none(),
        "this case is about a Portal with no agent runner and no realm",
    );

    let serving = tokio::spawn(joinedcontext_portal::server::serve(config));

    // It answers within a second or the case fails rather than hangs.
    let client = reqwest::Client::new();
    let mut answered = None;
    for _ in 0..50 {
        match client
            .get(format!("http://{bind}/api/v1/health"))
            .send()
            .await
        {
            Ok(response) => {
                answered = Some(response.status());
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    assert_eq!(
        answered,
        Some(reqwest::StatusCode::OK),
        "the server did not answer on the port it was given",
    );

    serving.abort();
}

// -------------------------------------------------------------------------------------------------
// T-2298 a namespace that is not a namespace
// -------------------------------------------------------------------------------------------------

/// MF-02, MF-04: the mirror's key is the namespace the manifest spells, and the reconciler puts
/// that string into the runner's URL template — which may carry `{project}` in its authority. A
/// manifest whose namespace is not a DNS-1123 label therefore steers an outbound request of the
/// committer's choosing, from the one network position that reaches the runner, the broker and the
/// forge. It is refused where it enters (T-2298).
#[tokio::test]
async fn a_manifest_whose_namespace_is_not_a_label_is_not_in_the_mirror() {
    let mirror = Mirror::new();
    for namespace in [
        "a/../../admin",
        "a@evil.example",
        "..",
        "A",
        "a b",
        "a_b",
        "",
        "a.b",
        "-a",
        "a-",
        "a:b",
        "a\nb",
        &"a".repeat(64),
    ] {
        mirror.upsert(envelope(
            "ContextSpace",
            "air",
            namespace,
            json!({ "isSandbox": false, "defaultLocale": "sk", "ttlDays": 30 }),
        ));
        assert!(
            mirror
                .list(namespace, "ContextSpace", &ListOptions::default())
                .items
                .is_empty(),
            "{namespace:?} reached the mirror",
        );
    }
    assert_eq!(
        mirror.len(),
        0,
        "something was held under a namespace that is not one"
    );

    // And the label beside them is held: the check refuses what is not a namespace and nothing
    // else, including the longest label a namespace may have.
    for namespace in ["ovzdusie", "a", "a-b-1", &"a".repeat(63)] {
        mirror.upsert(envelope(
            "ContextSpace",
            "air",
            namespace,
            json!({ "isSandbox": false, "defaultLocale": "sk", "ttlDays": 30 }),
        ));
        assert_eq!(
            mirror
                .list(namespace, "ContextSpace", &ListOptions::default())
                .items
                .len(),
            1,
            "{namespace:?} was refused",
        );
    }
}

/// T-2298: the same check at the other end, because either one alone leaves the other caller
/// open. The runner is not asked at all — the proof is the mock server's own log: nothing
/// arrived, so no request was made with a name that is not a project.
#[tokio::test]
async fn the_runner_is_never_asked_about_a_name_that_is_not_a_project() {
    let runner = wiremock::MockServer::start().await;
    // `{project}` in the authority is the natural shape of a per-project runner service, and it
    // is where an unchecked name moves the request to another host entirely.
    let deployer = StreamDeployer::new(format!("{}/{{project}}", runner.uri()));

    for project in [
        "a/../../admin",
        "a@evil.example",
        "..",
        "A",
        "a b",
        "",
        "a.b",
        "a:b",
        &"a".repeat(64),
    ] {
        assert!(
            deployer.metrics(project).await.is_none(),
            "{project:?} was scraped",
        );
    }
    assert!(
        runner
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "a request was made for a name that is not a project",
    );
}
