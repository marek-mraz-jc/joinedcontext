//! Edge cases of the three resource reads (T-2033, T-2034, T-2035; PF-59, PF-60, PF-61, R20, CC-76).
//!
//! **The contract, in one sentence:** a list and a read answer one body for everything a caller may not
//! have — an unknown plural, a kind no binding of theirs reads, a project they hold nothing in and a
//! resource that is there but not theirs — and the endpoint listing across projects is that same rule
//! applied project by project, an empty list rather than a refusal.
//!
//! The happy paths live in `resource_list_tests.rs` and `resource_get_tests.rs` (the pages, the
//! selectors, the envelopes). This file is the other side: a selector that is not one, a limit of
//! nothing, a workspace that is not the caller's, and the four ways of being told "not found".
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::auth::csrf::CSRF_COOKIE;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "helsinki";
const ELSEWHERE: &str = "espoo";
const CSRF: &str = "csrf-token-edge-reads";
const SPACE_READER: &str = "peter.spaces@hel.fi";
const STRANGER: &str = "nobody@hel.fi";

fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(namespace.into()),
            ..Default::default()
        },
        spec,
        status: None,
    }
}

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    envelope(kind, name, ORG_NAMESPACE, spec)
}

fn cookie(config: &Config, email: &str) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let username = email.split('@').next().unwrap_or(email).to_owned();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{username}"),
            username,
            email: Some(email.to_owned()),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &session)
        .expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

/// Two projects with a space and an endpoint each, and a caller who reads only `helsinki`'s spaces —
/// so the kind and the project are two separate reasons to be told nothing.
fn world() -> AppState {
    let state = AppState::new(Config::for_tests(), None);
    for project in [PROJECT, ELSEWHERE] {
        state
            .mirror
            .upsert(envelope("ContextSpace", project, project, json!({})));
        state.mirror.upsert(envelope(
            "Endpoint",
            &format!("{project}-bikes"),
            project,
            json!({
                "contextSpaceRef": { "kind": "ContextSpace", "name": project },
                "slug": "si6epqkx364lprho5uaigutk274r5grb",
                "audience": "project",
                "enabledRepresentations": ["ngsi-ld"],
            }),
        ));
    }
    state.mirror.upsert(org(
        "Role",
        "space-reader",
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "space-readers",
        json!({ "subjects": [{ "user": SPACE_READER }], "role": "space-reader",
                "scope": { "project": PROJECT } }),
    ));
    state
}

async fn get(state: &AppState, email: Option<&str>, uri: &str) -> (StatusCode, Value) {
    let mut request = Request::builder().uri(uri);
    if let Some(email) = email {
        request = request.header(header::COOKIE, cookie(&state.config, email));
    }
    let response = server::app(state.clone())
        .oneshot(request.body(Body::empty()).expect("a request"))
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

// -------------------------------------------------------------------------------------------------
// T-2033 `list`
// -------------------------------------------------------------------------------------------------

/// PF-59, R20: a listing has one body for every reason a caller may not have it — a plural that is no
/// kind, a kind their bindings do not read, and a project they hold nothing in. The three are
/// indistinguishable, which is the point: a 404 that varied would be a way to ask what exists.
#[tokio::test]
async fn one_body_answers_an_unknown_plural_an_unread_kind_and_an_unheld_project() {
    let state = world();

    // A plural that is no kind of this platform, a kind this reader does not read, and the project
    // next door: one answer for all three.
    let (unknown, unknown_body) = get(
        &state,
        Some(SPACE_READER),
        &format!("/api/v1/projects/{PROJECT}/teapots"),
    )
    .await;
    let (unread, unread_body) = get(
        &state,
        Some(SPACE_READER),
        &format!("/api/v1/projects/{PROJECT}/endpoints"),
    )
    .await;
    let (unheld, unheld_body) = get(
        &state,
        Some(SPACE_READER),
        &format!("/api/v1/projects/{ELSEWHERE}/spaces"),
    )
    .await;
    assert_eq!(unknown, StatusCode::NOT_FOUND, "{unknown_body}");
    assert_eq!(unread, StatusCode::NOT_FOUND, "{unread_body}");
    assert_eq!(unheld, StatusCode::NOT_FOUND, "{unheld_body}");
    assert_eq!(
        unread_body["title"], unknown_body["title"],
        "an unread kind reads differently from a plural that is no kind",
    );
    for body in [&unread_body, &unheld_body] {
        let text = body.to_string();
        for leaked in [
            "helsinki-bikes",
            "espoo-bikes",
            "si6epqkx364lprho5uaigutk274r5grb",
        ] {
            assert!(!text.contains(leaked), "the 404 carried {leaked:?}: {text}");
        }
    }

    // The one listing this reader may have.
    let (status, list) = get(
        &state,
        Some(SPACE_READER),
        &format!("/api/v1/projects/{PROJECT}/spaces"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"].as_array().map(Vec::len), Some(1), "{list}");
}

/// The query of a listing is read before the mirror is: a page of nothing, a selector that is not one
/// and a revision this route does not serve are each answered as the caller's mistake, and a limit past
/// the ceiling is the ceiling rather than a refusal.
#[tokio::test]
async fn a_query_that_is_not_one_is_the_callers_mistake_and_never_a_page() {
    let state = world();
    let spaces = format!("/api/v1/projects/{PROJECT}/spaces");

    // A page of nothing.
    let (status, body) = get(&state, Some(SPACE_READER), &format!("{spaces}?limit=0")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // A limit that is not a number at all, and one past what a page may be: both are refused, the
    // second naming limit (API/13 section 2, T-6175).
    for limit in ["abc", "-1", "1.5"] {
        let (status, body) = get(
            &state,
            Some(SPACE_READER),
            &format!("{spaces}?limit={limit}"),
        )
        .await;
        assert!(
            status.is_client_error(),
            "limit={limit} answered {status}: {body}",
        );
    }
    let (status, body) = get(&state, Some(SPACE_READER), &format!("{spaces}?limit=9999")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["errors"][0]["path"], "limit", "{body}");

    // A selector that is not a selector.
    for query in [
        "labelSelector=%3D%3D",
        "labelSelector=a%20b%20c",
        "fieldSelector=metadata.name",
        "fieldSelector=%3D%3Dx",
    ] {
        let (status, body) = get(&state, Some(SPACE_READER), &format!("{spaces}?{query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
        assert!(
            body["detail"].is_string(),
            "{query} was answered without saying what is wrong: {body}",
        );
    }

    // A revision that is not a commit id: refused rather than answered with today's list as
    // though it were the one asked for (MF-11, T-2375).
    let (status, body) = get(
        &state,
        Some(SPACE_READER),
        &format!("{spaces}?revision=main"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // A continuation token nobody minted is ignored and the first page comes back: today's
    // behaviour, written down in `/workspace/chyby.md` rather than asserted as wanted, because a
    // client walking a list would restart it instead of being told its token is stale. What matters
    // here and holds either way: the page is this caller's own reading and carries nothing else.
    let (status, body) = get(
        &state,
        Some(SPACE_READER),
        &format!("{spaces}?continue=bm90LWEtdG9rZW4"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["items"].as_array().is_some_and(|items| items
            .iter()
            .all(|item| item["kind"] == json!("ContextSpace"))),
        "a stale token answered with something other than this caller's own kind: {body}",
    );
    assert!(
        body["metadata"]["continue"].is_null(),
        "a stale token answered with a next page: {body}",
    );
}

// -------------------------------------------------------------------------------------------------
// T-2034 `get_resource`
// -------------------------------------------------------------------------------------------------

/// R20, CC-76: a single read is the same 404 for a resource that is not there and for one that is there
/// and not the caller's — and a copy it names has to be one the caller may read in, so a workspace name
/// is not a way around either rule.
#[tokio::test]
async fn a_single_read_never_says_whether_what_it_refuses_exists() {
    let state = world();

    // The endpoint exists in `helsinki`; this reader does not read endpoints. A name nobody has reads
    // exactly the same.
    let (there, there_body) = get(
        &state,
        Some(SPACE_READER),
        &format!("/api/v1/projects/{PROJECT}/endpoints/helsinki-bikes"),
    )
    .await;
    let (ghost, ghost_body) = get(
        &state,
        Some(SPACE_READER),
        &format!("/api/v1/projects/{PROJECT}/endpoints/nothing-like-this"),
    )
    .await;
    assert_eq!(there, StatusCode::NOT_FOUND, "{there_body}");
    assert_eq!(ghost, StatusCode::NOT_FOUND, "{ghost_body}");
    assert_eq!(
        there_body["detail"]
            .as_str()
            .map(|d| d.contains("helsinki-bikes")),
        Some(true),
        "the message should name what was asked for: {there_body}",
    );
    assert_eq!(
        there_body["title"], ghost_body["title"],
        "an endpoint that exists reads differently from one that does not",
    );
    assert!(
        !there_body
            .to_string()
            .contains("si6epqkx364lprho5uaigutk274r5grb"),
        "the 404 carried the endpoint's slug: {there_body}",
    );

    // A name that tries to leave the project, and a plural that is not one.
    for uri in [
        format!("/api/v1/projects/{PROJECT}/spaces/%2e%2e"),
        format!("/api/v1/projects/{PROJECT}/spaces/..%2fespoo"),
        format!("/api/v1/projects/{PROJECT}/contextspaces/{PROJECT}"),
        format!("/api/v1/projects/{ELSEWHERE}/spaces/{ELSEWHERE}"),
    ] {
        let (status, body) = get(&state, Some(SPACE_READER), &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }

    // A workspace nobody opened: refused, and never by falling back to the project's own copy.
    let (status, body) = get(
        &state,
        Some(SPACE_READER),
        &format!("/api/v1/projects/{PROJECT}/spaces/{PROJECT}?workspace=somebody-elses"),
    )
    .await;
    assert!(
        status.is_client_error(),
        "a workspace nobody opened was read as the project: {status} {body}",
    );
    assert_ne!(status, StatusCode::OK);

    // And the read this caller may have.
    let (status, space) = get(
        &state,
        Some(SPACE_READER),
        &format!("/api/v1/projects/{PROJECT}/spaces/{PROJECT}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{space}");
    assert_eq!(space["kind"], json!("ContextSpace"), "{space}");
}

// -------------------------------------------------------------------------------------------------
// T-2035 `list_endpoints_everywhere`
// -------------------------------------------------------------------------------------------------

/// PF-61, PF-03, R20, T-2877: the endpoint listing across projects is an administration view. An
/// administrator of the organization reads every project's endpoints, each item saying which project
/// it is in; everyone else is answered `404`, a reader of one project's endpoints included, who
/// still reads that project's own list; and half an administrator's verbs is not an administrator.
#[tokio::test]
async fn the_endpoints_of_every_project_are_for_an_administrator_of_the_organization_only() {
    let state = world();
    state.mirror.upsert(org(
        "Role",
        "endpoint-reader",
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "endpoint-readers",
        json!({ "subjects": [{ "user": "eva.endpoints@hel.fi" }], "role": "endpoint-reader",
                "scope": { "project": PROJECT } }),
    ));
    state.mirror.upsert(org(
        "Role",
        "administrator",
        json!({ "rules": [
            { "kinds": ["RoleBinding"], "verbs": ["approve", "delete"] },
            { "kinds": ["Endpoint"], "verbs": ["read"] },
        ] }),
    ));
    state.mirror.upsert(org(
        "Role",
        "binding-approver",
        json!({ "rules": [
            { "kinds": ["RoleBinding"], "verbs": ["approve"] },
            { "kinds": ["Endpoint"], "verbs": ["read"] },
        ] }),
    ));
    for (name, user, role) in [
        ("administrators", "ada.admin@hel.fi", "administrator"),
        (
            "binding-approvers",
            "bo.approver@hel.fi",
            "binding-approver",
        ),
    ] {
        state.mirror.upsert(org(
            "RoleBinding",
            name,
            json!({ "subjects": [{ "user": user }], "role": role,
                    "scope": { "organization": "hel" } }),
        ));
    }

    // The administrator: every project's endpoint, each saying which project it lives in.
    let (status, all) = get(&state, Some("ada.admin@hel.fi"), "/api/v1/endpoints").await;
    assert_eq!(status, StatusCode::OK, "{all}");
    let listed: Vec<(Value, Value)> = all["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            (
                item["metadata"]["namespace"].clone(),
                item["metadata"]["name"].clone(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        vec![
            (json!(ELSEWHERE), json!("espoo-bikes")),
            (json!(PROJECT), json!("helsinki-bikes")),
        ],
        "{all}"
    );

    // The one list contract (API/13 section 2, T-6175): sort, q, count and a cursor that walks
    // the whole list under them, a bad value refused naming its parameter.
    let ada = Some("ada.admin@hel.fi");
    let start = "/api/v1/endpoints?sort=-metadata.name&q=BIKES&limit=1&count=true";
    let (status, first) = get(&state, ada, start).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["totalCount"], 2, "{first}");
    assert_eq!(first["items"][0]["metadata"]["name"], "helsinki-bikes");
    let cursor: String = url::form_urlencoded::byte_serialize(
        first["nextCursor"].as_str().expect("a cursor").as_bytes(),
    )
    .collect();
    let (_, second) = get(&state, ada, &format!("{start}&cursor={cursor}")).await;
    assert_eq!(
        second["items"][0]["metadata"]["name"], "espoo-bikes",
        "{second}"
    );
    assert!(second.get("nextCursor").is_none(), "{second}");
    let (_, none) = get(&state, ada, "/api/v1/endpoints?q=trams&count=true").await;
    assert_eq!(none["totalCount"], 0, "{none}");
    for (query, status, field) in [
        ("limit=0", StatusCode::BAD_REQUEST, "limit"),
        ("count=1", StatusCode::BAD_REQUEST, "count"),
        ("sort=-nosuch", StatusCode::UNPROCESSABLE_ENTITY, "sort"),
    ] {
        let (got, body) = get(&state, ada, &format!("/api/v1/endpoints?{query}")).await;
        assert_eq!(got, status, "{query}: {body}");
        assert_eq!(body["errors"][0]["path"], field, "{query}: {body}");
    }

    // Nobody else is offered the page: not a stranger, not a reader of one project's endpoints,
    // not a reader of spaces, and not a person who approves bindings but may not delete them.
    for caller in [
        STRANGER,
        "eva.endpoints@hel.fi",
        SPACE_READER,
        "bo.approver@hel.fi",
    ] {
        let (status, body) = get(&state, Some(caller), "/api/v1/endpoints").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{caller}: {body}");
        assert!(
            !body.to_string().contains("bikes"),
            "the refusal carried an endpoint: {body}"
        );
    }

    // The reader of one project still reads that project's own endpoints, and only those.
    let (status, mine) = get(
        &state,
        Some("eva.endpoints@hel.fi"),
        &format!("/api/v1/projects/{PROJECT}/endpoints"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    assert_eq!(mine["items"].as_array().map(Vec::len), Some(1), "{mine}");

    let (status, _) = get(&state, None, "/api/v1/endpoints").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
