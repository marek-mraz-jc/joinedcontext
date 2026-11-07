//! Edge cases of the MCP doors and of the resource listing behind them (T-2065 … T-2068; AG-77,
//! PF-50, PF-59, R20, RFC 9728).
//!
//! **The contract, in one sentence:** the MCP door is opened by an audience-bound bearer and nothing
//! else, every refusal at it is the same 401 with the same pointer to the metadata document, and the
//! listing behind it answers what the caller may read in that project and no more.
//!
//! `mcp_portal_tests.rs` covers the door working (the handshake, `tools/list` narrowed by the caller,
//! a call that becomes a task, the rate limit, the byte bound, the metadata document) and
//! `ops_resource_tests.rs` the listing's happy paths. This file is what those leave: the shapes of an
//! `Authorization` header that are not a bearer, the metadata document of an installation with no
//! identity provider, the tasks of one caller never appearing in another's list, and what a listing
//! does with an unknown kind, a space nobody has and a kind that lives in two homes.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::{envelope, person};
use http_body_util::BodyExt;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::mcp::tasks::McpTasks;
use joinedcontext_portal::ops::{self, Caller, Via};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

const MCP: &str = "/api/v1/mcp";
const METADATA: &str = "/.well-known/oauth-protected-resource";

async fn get(state: &AppState, uri: &str, headers: &[(&str, &str)]) -> (StatusCode, String, Value) {
    let mut request = Request::builder().uri(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = server::app(state.clone())
        .oneshot(request.body(Body::empty()).expect("a request"))
        .await
        .expect("a response");
    let status = response.status();
    let authenticate = response
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        authenticate,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn post(state: &AppState, headers: &[(&str, &str)], body: &str) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method("POST")
        .uri(MCP)
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = server::app(state.clone())
        .oneshot(
            request
                .body(Body::from(body.to_owned()))
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status();
    let authenticate = response
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    (status, authenticate)
}

// -------------------------------------------------------------------------------------------------
// T-2065 the protected-resource metadata document
// -------------------------------------------------------------------------------------------------

/// RFC 9728: the metadata document is what an MCP client reads before it has a token, so it is served
/// to anybody, at both paths, and it says only what a client needs: this resource, the servers that
/// may issue for it, and how a token travels. An installation with no identity provider answers an
/// empty list of servers rather than a guess or a missing member — a client can then say that this
/// Portal cannot be signed in to, instead of failing on a document it could not parse.
#[tokio::test]
async fn the_metadata_document_is_public_well_formed_and_the_same_at_both_paths() {
    let state = AppState::new(Config::for_tests(), None);

    for uri in [METADATA, "/.well-known/oauth-protected-resource/api/v1/mcp"] {
        // Anonymous, and with a token that means nothing here: the same document either way.
        for headers in [
            Vec::new(),
            vec![(header::AUTHORIZATION.as_str(), "Bearer x")],
        ] {
            let (status, _, doc) = get(&state, uri, &headers).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {doc}");
            assert_eq!(
                doc["resource"],
                json!(format!(
                    "{}/api/v1/mcp",
                    Config::for_tests()
                        .public_base_url
                        .as_str()
                        .trim_end_matches('/')
                )),
                "{uri}: {doc}",
            );
            assert!(
                doc["resource"]
                    .as_str()
                    .is_some_and(|resource| !resource.contains("//api")),
                "a trailing slash in the base URL doubled: {doc}",
            );
            assert_eq!(doc["bearer_methods_supported"], json!(["header"]), "{uri}");
            assert_eq!(doc["scopes_supported"], json!(["mcp:portal"]), "{uri}");
            assert_eq!(
                doc["authorization_servers"],
                json!([]),
                "an installation with no realm named one: {doc}",
            );
            // Nothing of the installation beyond those two facts, and no session.
            for leaked in ["client_secret", "cookie", "__Host-jc_session", "password"] {
                assert!(
                    !doc.to_string().contains(leaked),
                    "{uri} carries {leaked:?}: {doc}",
                );
            }
        }
    }
}

// -------------------------------------------------------------------------------------------------
// T-2066 the MCP door
// -------------------------------------------------------------------------------------------------

/// PF-50, RFC 9728: only `Bearer <token>` opens the door, and everything else is one 401 that points
/// at the metadata document. This case is about the shapes that look like credentials and are not —
/// a lowercase scheme, a scheme with no token, a token of spaces, Basic, and a cookie — so that none
/// of them can be told apart from a token that simply did not verify.
#[tokio::test]
async fn one_401_for_every_shape_that_is_not_a_bearer() {
    let state = AppState::new(Config::for_tests(), None);
    let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }).to_string();

    let not_a_bearer = [
        ("nothing at all", Vec::new()),
        (
            "an empty header",
            vec![(header::AUTHORIZATION.as_str(), "")],
        ),
        (
            "the scheme alone",
            vec![(header::AUTHORIZATION.as_str(), "Bearer")],
        ),
        (
            "the scheme and spaces",
            vec![(header::AUTHORIZATION.as_str(), "Bearer    ")],
        ),
        (
            "a lowercase scheme",
            vec![(header::AUTHORIZATION.as_str(), "bearer a-token")],
        ),
        (
            "two spaces before the token",
            vec![(header::AUTHORIZATION.as_str(), "Bearer  a-token")],
        ),
        (
            "basic credentials",
            vec![(header::AUTHORIZATION.as_str(), "Basic am M6am M=")],
        ),
        (
            "a session cookie instead",
            vec![(header::COOKIE.as_str(), "__Host-jc_session=whatever")],
        ),
        (
            "a token that verifies against nothing",
            vec![(header::AUTHORIZATION.as_str(), "Bearer not.a.jwt")],
        ),
    ];

    let mut challenges = Vec::new();
    for (what, headers) in not_a_bearer {
        let (status, authenticate) = post(&state, &headers, &call).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{what}");
        assert!(
            authenticate.starts_with("Bearer"),
            "{what}: the challenge is not a bearer challenge: {authenticate:?}",
        );
        assert!(
            authenticate.contains("resource_metadata="),
            "{what}: the challenge does not point at the metadata document: {authenticate:?}",
        );
        challenges.push(authenticate);
    }
    // One challenge for all of them: the shape of what was presented is not a signal.
    assert!(
        challenges.windows(2).all(|pair| pair[0] == pair[1]),
        "the challenge differs by what was presented: {challenges:?}",
    );
}

/// PF-50: the door authenticates before it reads, so a body nobody may send is never parsed and never
/// counted. An oversized body, a body that is not JSON and a body that is JSON but not JSON-RPC all
/// answer the same 401 to a caller with no token — the byte bound and the parse error belong to a
/// caller who got in, and `mcp_portal_tests.rs` is where they are proved.
#[tokio::test]
async fn a_caller_without_a_token_never_reaches_the_parser_or_the_byte_bound() {
    let state = AppState::new(Config::for_tests(), None);

    let bodies = [
        String::new(),
        "not json at all".to_owned(),
        json!({ "jsonrpc": "1.0", "method": "tools/list" }).to_string(),
        json!({ "method": "tools/call", "params": { "name": "jc_endpoint_delete" } }).to_string(),
        // Past `MAX_REQUEST_BYTES`: still a 401, because the token is read first.
        format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"pad\":\"{}\"}}",
            "x".repeat(1024 * 1024 + 16)
        ),
    ];
    for body in bodies {
        let (status, authenticate) = post(&state, &[], &body).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "a body of {} bytes was answered something else",
            body.len(),
        );
        assert!(
            authenticate.contains("resource_metadata="),
            "{authenticate}"
        );
    }
}

// -------------------------------------------------------------------------------------------------
// T-2067 the task store behind a long call
// -------------------------------------------------------------------------------------------------

/// AG-77, PF-59: a task belongs to the subject of the token that started it, and the listing is that
/// subject's alone. Newest first, nobody else's in it, and an owner with no tasks gets an empty list
/// rather than everybody's.
#[tokio::test]
async fn the_task_list_is_one_subjects_own_newest_first() {
    let tasks = McpTasks::new();

    // Three of one caller's and one of another's, started in order.
    let mut mine = Vec::new();
    for n in 0..3 {
        let started = tasks.start("f:1:jana", async move { json!({ "n": n }) });
        mine.push(started["taskId"].as_str().expect("a task id").to_owned());
        // The store orders by the second, so the ids are what a tie is told apart by; this case
        // asserts membership and the newest, not a total order of three starts in one instant.
    }
    let theirs = tasks.start("f:1:petra", async { json!({ "n": "theirs" }) });
    let theirs_id = theirs["taskId"].as_str().expect("a task id").to_owned();

    let listed = tasks.list("f:1:jana");
    let ids: Vec<&str> = listed
        .iter()
        .filter_map(|row| row["taskId"].as_str())
        .collect();
    assert_eq!(ids.len(), 3, "{listed:?}");
    for id in &mine {
        assert!(ids.contains(&id.as_str()), "{id} is not in {ids:?}");
    }
    assert!(
        !ids.contains(&theirs_id.as_str()),
        "another subject's task is in this list: {ids:?}",
    );

    // Every row carries what a client polls with and nothing about the work itself.
    for row in &listed {
        assert!(row["status"].is_string(), "{row}");
        assert!(row["createdAt"].is_i64(), "{row}");
        assert!(row["ttl"].is_i64(), "{row}");
        assert!(row["pollInterval"].is_u64(), "{row}");
        assert!(
            row.get("result").is_none() && row.get("owner").is_none(),
            "the listing carries the work or the owner: {row}",
        );
    }

    // A subject with nothing running reads an empty list, not everybody's.
    assert!(tasks.list("f:1:nobody").is_empty());
    assert!(tasks.list("").is_empty(), "an empty subject matched a task");
    assert_eq!(tasks.list("f:1:petra").len(), 1);
}

// -------------------------------------------------------------------------------------------------
// T-2068 the resource listing behind the door
// -------------------------------------------------------------------------------------------------

/// PF-59, R20, AG-77: a kind the caller may not read is not there, and so is a kind the platform does
/// not serve — the listing never answers 403, and it never answers rows from a project the caller has
/// no grant in. A `space` nobody has is an empty list rather than an error, because a filter that
/// matches nothing is a legitimate question.
#[tokio::test]
async fn a_listing_answers_nothing_for_a_kind_or_a_project_the_caller_does_not_read() {
    let state = AppState::new(Config::for_tests(), None);
    state.mirror.upsert(envelope(
        "ContextSpace",
        "air",
        "ovzdusie",
        json!({ "isSandbox": true, "defaultLocale": "en", "ttlDays": 10 }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "space-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "readers",
        ORG_NAMESPACE,
        json!({ "role": "space-reader", "subjects": [{ "user": "vera@hel.fi" }],
                "scope": { "project": "ovzdusie" } }),
    ));

    let list = ops::find("jc_resource_list").expect("jc_resource_list is registered");
    let reader = Caller {
        bearer: None,
        identity: person("vera"),
        via: Via::Mcp,
        access: None,
    };
    let stranger = Caller {
        bearer: None,
        identity: person("nobody"),
        via: Via::Mcp,
        access: None,
    };

    // What the reader may read, they read.
    let answer = ops::call(
        list,
        &reader,
        &state,
        "ovzdusie",
        json!({ "kind": "ContextSpace" }),
    )
    .await
    .expect("the spaces of ovzdusie");
    assert_eq!(answer["items"][0]["name"], json!("air"), "{answer}");

    // A kind their binding does not carry, and a project they are not bound to: told it is not
    // there, never that they are forbidden (PF-59, R20), and never with a resource named in the
    // refusal.
    for (who, project, input) in [
        (&reader, "ovzdusie", json!({ "kind": "Endpoint" })),
        (&reader, "doprava", json!({ "kind": "ContextSpace" })),
        (&stranger, "ovzdusie", json!({ "kind": "ContextSpace" })),
    ] {
        let error = ops::call(list, who, &state, project, input.clone())
            .await
            .expect_err(&format!("{input} in {project} was answered"));
        let text = format!("{error:?}");
        assert!(
            text.contains("not found"),
            "{input} in {project} was refused with something other than a miss: {text}",
        );
        assert!(
            !text.contains("air"),
            "{input} named a resource the caller may not read: {text}",
        );
    }

    // A kind that is not one is the caller's mistake and is refused by the operation's validator,
    // before any permission is read — so the answer is an invalid input naming the kinds this
    // Portal serves, which is its published surface and not a disclosure. The comparison is
    // case-sensitive: `contextspace` is not `ContextSpace`.
    for input in [
        json!({ "kind": "NotAKind" }),
        json!({ "kind": "contextspace" }),
        json!({ "kind": "CONTEXTSPACE" }),
        json!({ "kind": "" }),
        json!({ "kind": " ContextSpace" }),
        json!({}),
    ] {
        let error = ops::call(list, &stranger, &state, "ovzdusie", input.clone())
            .await
            .expect_err(&format!("{input} was answered"));
        let text = format!("{error:?}");
        assert!(
            text.contains("InvalidInput"),
            "{input} was not refused as an invalid input: {text}",
        );
        assert!(
            !text.contains("air") && !text.contains("vera"),
            "{input} named a resource or a person: {text}",
        );
    }

    // A space filter that matches nothing: an empty list, and not an error.
    let answer = ops::call(
        list,
        &reader,
        &state,
        "ovzdusie",
        json!({ "kind": "ContextSpace", "space": "no-such-space" }),
    )
    .await
    .expect("an empty list is a legitimate answer");
    assert_eq!(answer["items"], json!([]), "{answer}");
}

/// A kind that lives in the organization's home as well as the project's is listed from both, and the
/// rows are not de-duplicated: a Role of one name in the organization and a Role of that name in the
/// project are two rows with one name for a caller who reads Roles in both (`src/ops/resources.rs`). That is today's behaviour and it is
/// arguably right — they are two different manifests — but a client that keys a list by name shows one
/// and hides the other, so it is written down in `/workspace/chyby.md` and pinned here.
#[tokio::test]
async fn a_kind_that_lives_in_two_homes_is_listed_from_both_without_de_duplication() {
    let state = AppState::new(Config::for_tests(), None);
    for namespace in [ORG_NAMESPACE, "ovzdusie"] {
        state.mirror.upsert(envelope(
            "Role",
            "analyst",
            namespace,
            json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
        ));
    }
    state.mirror.upsert(envelope(
        "RoleBinding",
        "admins",
        ORG_NAMESPACE,
        json!({ "role": "analyst", "subjects": [{ "user": "jana@hel.fi" }],
                "scope": { "project": "ovzdusie" } }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "analyst-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Role"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "role-readers",
        ORG_NAMESPACE,
        // Read in both homes: each home is judged by the grants held there (T-6273).
        json!({ "role": "analyst-reader", "subjects": [{ "user": "jana@hel.fi" }],
                "scope": { "organization": "hel" } }),
    ));

    let list = ops::find("jc_resource_list").expect("registered");
    let caller = Caller {
        bearer: None,
        identity: person("jana"),
        via: Via::Mcp,
        access: None,
    };
    let answer = ops::call(list, &caller, &state, "ovzdusie", json!({ "kind": "Role" }))
        .await
        .expect("the roles in force");
    let named: Vec<&Value> = answer["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|item| item["name"] == json!("analyst"))
        .collect();
    assert_eq!(
        named.len(),
        2,
        "the two homes' roles of one name were folded into one: {answer}",
    );
}
