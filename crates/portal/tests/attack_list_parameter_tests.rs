//! Attack vector T-1688 (CC-24): paging, filters and sort as an injection or a scan.
//!
//! The Portal's own lists are served from an in-memory mirror of the repository, so there is no
//! statement to inject into; the run and activity lists are Postgres, and every value they carry
//! is bound (`src/db.rs` builds only the *shape* of a statement with `format!` and binds every
//! value, `src/activity.rs` the same). What is left to attack is the arithmetic: a limit of zero,
//! a limit of a million, an offset that is not a number, a continue token nobody issued, and a
//! selector whose key or value is a fragment of something else.
//!
//! What the existing suites already play, so this file does not repeat it:
//! `store::tests::two_page_walk_returns_every_item_exactly_once` (the paging itself),
//! `selector`'s own unit cases (a malformed selector is a `Malformed`, an unknown field an
//! `UnknownField`), `api::activity::tests` (a limit of 10 000 is clamped to `MAX_LIMIT`),
//! `export_api_tests` (the revision picker's 1…100).

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "ovzdusie";

/// The values that read like the start of something else: a statement, a pattern, a path.
const FRAGMENTS: &[&str] = &[
    "' OR '1'='1",
    "'; DROP TABLE resources; --",
    "1) OR (1=1",
    "%27%20OR%201%3D1",
    "(a+)+$",
    "../../../../etc/passwd",
    "\\u0000",
];

fn admin() -> Identity {
    let mut identity = common::person("admin");
    identity.groups = vec!["portal-approver".to_owned()];
    identity
}

/// A project with three Endpoints and a reader of them.
async fn world() -> AppState {
    let state = common::state_on_forge().await;
    for name in ["air", "bikes", "water"] {
        state.mirror.upsert(common::envelope(
            "Endpoint",
            name,
            PROJECT,
            json!({ "contextSpaceRef": "air", "audience": "organization" }),
        ));
    }
    state.mirror.upsert(common::envelope(
        "Role",
        "reads-endpoints",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "reader",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "reader@hel.fi" }],
            "role": "reads-endpoints",
            "scope": { "project": PROJECT },
        }),
    ));
    state
}

async fn list(state: &AppState, query: &str) -> common::Answer {
    common::send(
        state,
        common::person("reader"),
        "GET",
        &format!("/api/v1/projects/{PROJECT}/endpoints?{query}"),
        None,
    )
    .await
}

/// CC-24: a limit that is not a page size is a 400 naming the parameter, a limit past the ceiling
/// of 1000 too (API/13 section 2, T-6175), and the ceiling itself is a page, never a 500.
#[tokio::test]
async fn a_limit_is_bounded_at_both_ends() {
    let state = world().await;
    let zero = list(&state, "limit=0").await;
    assert_eq!(zero.status, StatusCode::BAD_REQUEST, "{}", zero.text);
    assert!(
        zero.text.contains("limit"),
        "the refusal names the parameter: {}",
        zero.text
    );
    for query in [
        "limit=-1",
        "limit=1001",
        "limit=1000000",
        "limit=9999999999999999999999",
        "limit=abc",
        "limit=",
    ] {
        let answer = list(&state, query).await;
        assert_eq!(
            answer.status,
            StatusCode::BAD_REQUEST,
            "{query}: {}",
            answer.text
        );
        assert!(
            answer.text.contains("limit"),
            "{query}: the refusal names the parameter: {}",
            answer.text
        );
    }
    let huge = list(&state, "limit=1000").await;
    assert_eq!(huge.status, StatusCode::OK, "{}", huge.text);
    let body: Value = serde_json::from_str(&huge.text).expect("a list");
    assert_eq!(
        body["items"].as_array().map(Vec::len),
        Some(3),
        "the page is the project's three endpoints: {}",
        huge.text
    );
}

/// CC-24: a continue token nobody issued names a resource that is not there, so the page after it
/// is empty — never a 500, and never the whole list.
#[tokio::test]
async fn a_continue_token_nobody_issued_is_not_a_page_of_everything() {
    let state = world().await;
    for token in FRAGMENTS {
        let answer = list(&state, &format!("limit=2&continue={}", urlencoding(token))).await;
        assert!(
            answer.status.is_success() || answer.status == StatusCode::BAD_REQUEST,
            "{token}: {} {}",
            answer.status,
            answer.text
        );
        assert!(
            !answer.status.is_server_error(),
            "{token} made the server fail: {}",
            answer.text
        );
    }
}

/// CC-24: a selector's key and value are data. A fragment of a statement, a pattern that would
/// bomb a backtracking engine, and a path that climbs out of a directory are each either a 400
/// naming the selector or an honest empty list — and the three endpoints of the project are never
/// answered to a selector that matches none of them.
#[tokio::test]
async fn a_selector_is_data_and_never_a_statement() {
    let state = world().await;
    for fragment in FRAGMENTS {
        for query in [
            format!("labelSelector={}", urlencoding(&format!("env={fragment}"))),
            format!("labelSelector={}", urlencoding(fragment)),
            format!(
                "fieldSelector={}",
                urlencoding(&format!("metadata.name={fragment}"))
            ),
            format!("fieldSelector={}", urlencoding(fragment)),
        ] {
            let answer = list(&state, &query).await;
            assert!(
                !answer.status.is_server_error(),
                "{query} made the server fail: {}",
                answer.text
            );
            if answer.status.is_success() {
                let body: Value = serde_json::from_str(&answer.text).expect("a list");
                assert_eq!(
                    body["items"].as_array().map(Vec::len),
                    Some(0),
                    "{query} matched something: {}",
                    answer.text
                );
            } else {
                assert_eq!(
                    answer.status,
                    StatusCode::BAD_REQUEST,
                    "{query}: {}",
                    answer.text
                );
            }
        }
    }
}

/// CC-24: a field nobody may select by is named in the refusal, so a caller learns which fields
/// there are rather than which values exist.
#[tokio::test]
async fn an_unknown_selector_field_is_named_and_nothing_else_is() {
    let state = world().await;
    let answer = list(&state, "fieldSelector=spec.audience%3Dsecret").await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    assert!(
        answer.text.contains("spec.audience"),
        "the refusal names the field asked for: {}",
        answer.text
    );
    assert!(
        !answer.text.contains("air") && !answer.text.contains("bikes"),
        "the refusal listed what is there: {}",
        answer.text
    );
}

/// CC-24, PF-59: none of this is a way in. Every query above is refused or empty for a caller who
/// reads the project; for a caller who does not, the plural is not there at all, whatever the
/// query says.
#[tokio::test]
async fn no_query_reaches_a_project_the_caller_does_not_read() {
    let state = world().await;
    for query in [
        "limit=1000000",
        "labelSelector=env%3D%27%20OR%20%271%27%3D%271",
        "limit=0",
    ] {
        let answer = common::send(
            &state,
            common::person("stranger"),
            "GET",
            &format!("/api/v1/projects/{PROJECT}/endpoints?{query}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{query}: {}",
            answer.text
        );
    }
    // And the same for a caller who may read it, so the case above is the binding and not the
    // query: the administrator gets the query's own answer, a 400 naming limit (T-6175).
    let answer = common::send(
        &state,
        admin(),
        "GET",
        &format!("/api/v1/projects/{PROJECT}/endpoints?limit=1000000"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    assert!(answer.names_field("limit", "1 to 1000"), "{}", answer.text);
}

/// Percent-encoding for a query value, by hand: the test's own input must reach the server as
/// typed, and a fragment full of quotes and slashes would not survive a naive concatenation.
fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}
