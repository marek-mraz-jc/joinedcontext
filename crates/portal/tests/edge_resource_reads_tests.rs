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

// -------------------------------------------------------------------------------------------------
// T-6632, T-6655 the project's OData service, generated from the operation registry
// -------------------------------------------------------------------------------------------------

const VIEWER: &str = "vera.viewer@hel.fi";
const EDITOR: &str = "eddie.editor@hel.fi";
const OUTSIDER: &str = "oscar.espoo@espoo.fi";

/// `helsinki` with two context spaces, three Endpoints and two Pipelines; a viewer and an editor of
/// `helsinki`, a reader bound to the `air` space alone, and a reader of `espoo` alone.
fn odata_world() -> AppState {
    let state = world();
    for space in ["air", "water"] {
        state
            .mirror
            .upsert(envelope("ContextSpace", space, PROJECT, json!({})));
    }
    for (name, space) in [
        ("air-counters", "air"),
        ("air-sensors", "air"),
        ("water-gauges", "water"),
    ] {
        state.mirror.upsert(envelope(
            "Endpoint",
            name,
            PROJECT,
            json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": space },
                    "slug": "si6epqkx364lprho5uaigutk274r5grb", "audience": "public" }),
        ));
    }
    for (name, space) in [("air-intake", "air"), ("water-intake", "water")] {
        state.mirror.upsert(envelope(
            "Pipeline",
            name,
            PROJECT,
            json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": space } }),
        ));
    }
    for (role, verbs) in [
        ("odata-viewer", json!(["read"])),
        ("odata-editor", json!(["read", "propose"])),
    ] {
        state.mirror.upsert(org(
            "Role",
            role,
            json!({ "rules": [{ "kinds": ["Endpoint", "Pipeline", "ContextSpace"], "verbs": verbs }] }),
        ));
    }
    state.mirror.upsert(org(
        "Role",
        "endpoint-reader",
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read"] }] }),
    ));
    for (binding, user, role, scope) in [
        (
            "odata-viewers",
            VIEWER,
            "odata-viewer",
            json!({ "project": PROJECT }),
        ),
        (
            "odata-editors",
            EDITOR,
            "odata-editor",
            json!({ "project": PROJECT }),
        ),
        (
            "odata-outsiders",
            OUTSIDER,
            "odata-viewer",
            json!({ "project": ELSEWHERE }),
        ),
        (
            "air-readers",
            "ari.air@hel.fi",
            "endpoint-reader",
            json!({ "contextSpace": "air" }),
        ),
    ] {
        state.mirror.upsert(org(
            "RoleBinding",
            binding,
            json!({ "subjects": [{ "user": user }], "role": role, "scope": scope }),
        ));
    }
    state
}

fn odata(query: &str) -> String {
    format!("/api/v1/projects/{PROJECT}/odata/{query}")
}

/// The sorted `key` strings of the members of `body[member]`.
fn names_in(body: &Value, member: &str, key: &str) -> Vec<String> {
    let mut names: Vec<String> = body[member]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.pointer(key).and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

async fn post(
    state: &AppState,
    email: Option<&str>,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(joinedcontext_portal::auth::csrf::CSRF_HEADER, CSRF);
    if let Some(email) = email {
        request = request.header(header::COOKIE, cookie(&state.config, email));
    }
    let response = server::app(state.clone())
        .oneshot(
            request
                .body(Body::from(body.to_string()))
                .expect("a request"),
        )
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

/// T-6655 parity: every mapped list operation answers OData with the rows and the status its REST
/// route answers, for a viewer, an editor, a member of another project and nobody signed in. The
/// REST list names a row by `metadata.name` (or `name`), the entity set by its key `name`.
#[tokio::test]
async fn every_entity_set_answers_what_its_rest_route_answers() {
    let state = odata_world();
    for (set, rest, key) in [
        ("endpoints", "endpoints", "/metadata/name"),
        ("pipelines", "pipelines", "/metadata/name"),
        ("spaces", "spaces", "/metadata/name"),
        ("changes", "changes", "/metadata/name"),
        ("workspaces", "workspaces", "/name"),
    ] {
        for who in [
            Some(VIEWER),
            Some(EDITOR),
            Some(OUTSIDER),
            Some(STRANGER),
            None,
        ] {
            let (rest_status, list) =
                get(&state, who, &format!("/api/v1/projects/{PROJECT}/{rest}")).await;
            let (status, body) = get(&state, who, &odata(set)).await;
            assert_eq!(
                status, rest_status,
                "{set} for {who:?}: OData {body} against REST {list}"
            );
            if status == StatusCode::OK {
                assert_eq!(
                    names_in(&body, "value", "/name"),
                    names_in(&list, "items", key),
                    "{set} for {who:?}"
                );
            }
        }
    }
    // A member of another project and a stranger are told the project is not there, never 403.
    for who in [OUTSIDER, STRANGER] {
        let (status, body) = get(&state, Some(who), &odata("endpoints")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who}: {body}");
        let (status, _) = get(&state, Some(who), &odata("$metadata")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who}");
    }
    let (status, _) = get(&state, None, &odata("endpoints")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// T-6632, T-6655: the read check runs before `$filter`: a reader bound to one context space finds
/// no other space's Endpoint by filtering, counting or naming its key.
#[tokio::test]
async fn a_filter_reaches_only_the_rows_the_list_reads() {
    let state = odata_world();
    let ari = Some("ari.air@hel.fi");
    let (status, list) = get(
        &state,
        ari,
        &format!("/api/v1/projects/{PROJECT}/endpoints"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let (status, all) = get(&state, ari, &odata("endpoints?$count=true")).await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert_eq!(
        names_in(&all, "value", "/name"),
        names_in(&list, "items", "/metadata/name")
    );
    assert_eq!(
        names_in(&all, "value", "/name"),
        ["air-counters", "air-sensors"]
    );
    assert_eq!(all["@odata.count"], json!(2));

    let (status, water) = get(
        &state,
        ari,
        &odata("endpoints?$filter=space%20eq%20'water'%20or%20true&$count=true"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{water}");
    assert_eq!(
        names_in(&water, "value", "/name"),
        ["air-counters", "air-sensors"]
    );
    assert!(!water.to_string().contains("water-gauges"), "{water}");
    let (status, hidden) = get(&state, ari, &odata("endpoints('water-gauges')")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{hidden}");
    let (status, counted) = get(&state, ari, &odata("endpoints/$count")).await;
    assert_eq!((status, counted), (StatusCode::OK, Value::from(2)));

    // A member no list answers (a manifest's spec, where a secretRef sits) is no property.
    for query in [
        "endpoints?$filter=spec%20eq%20'x'",
        "endpoints?$select=slug",
    ] {
        let (status, refused) = get(&state, ari, &odata(query)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {refused}");
        assert!(
            refused["error"]["message"].is_string(),
            "an OData error: {refused}"
        );
    }
}

/// T-6655: `$filter`, `$orderby`, `$top`, `$select` and paging on two entity sets.
#[tokio::test]
async fn filter_orderby_and_top_answer_on_two_sets() {
    let state = odata_world();
    let viewer = Some(VIEWER);
    let (status, desc) = get(
        &state,
        viewer,
        &odata("endpoints?$filter=startswith(name,'air')&$orderby=name%20desc&$select=name,space"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{desc}");
    assert_eq!(
        desc["value"],
        json!([{ "name": "air-sensors", "space": "air" }, { "name": "air-counters", "space": "air" }])
    );
    assert!(desc["@odata.context"]
        .as_str()
        .is_some_and(|c| c.ends_with("$metadata#endpoints(name,space)")));

    let (status, top) = get(
        &state,
        viewer,
        &odata("endpoints?$orderby=name&$top=1&$count=true"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{top}");
    assert_eq!(names_in(&top, "value", "/name"), ["air-counters"]);
    assert_eq!(
        top["@odata.count"],
        json!(4),
        "helsinki-bikes of world() too"
    );
    assert!(
        top.get("@odata.nextLink").is_none(),
        "$top ends here: {top}"
    );

    let (status, pipelines) = get(
        &state,
        viewer,
        &odata("pipelines?$filter=space%20eq%20'water'%20and%20kind%20eq%20'Pipeline'"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pipelines}");
    assert_eq!(names_in(&pipelines, "value", "/name"), ["water-intake"]);
    let (status, first) = get(
        &state,
        viewer,
        &odata("pipelines?$orderby=name%20desc&$top=1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(names_in(&first, "value", "/name"), ["water-intake"]);
    let (status, one) = get(
        &state,
        viewer,
        &odata("pipelines('air-intake')?$select=name"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(one["name"], json!("air-intake"));
    assert!(
        one.get("space").is_none(),
        "$select keeps name alone: {one}"
    );

    let (status, unknown) = get(&state, viewer, &odata("nothings")).await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
        "{status} {unknown}"
    );
    let (status, _) = get(&state, viewer, &odata("endpoints?$expand=x")).await;
    assert!(
        status.is_client_error() || status == StatusCode::NOT_IMPLEMENTED,
        "{status}"
    );
}

/// T-6655: an action runs the writing operation through the registry: a viewer is refused as
/// `POST /ops/{name}` refuses them, nobody signed in is refused by the CSRF check first, and a read
/// operation is no action.
#[tokio::test]
async fn an_action_is_refused_as_its_operation_is() {
    let state = odata_world();
    let input = json!({ "kind": "Endpoint", "name": "air-counters", "confirm": "air-counters" });
    let ops = format!("/api/v1/projects/{PROJECT}/ops/jc_resource_delete");
    let (rest_status, rest) = post(&state, Some(VIEWER), &ops, input.clone()).await;
    let (status, body) = post(
        &state,
        Some(VIEWER),
        &odata("jc_resource_delete"),
        input.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(status, rest_status, "OData {body} against ops {rest}");
    let (status, _) = post(
        &state,
        Some(OUTSIDER),
        &odata("jc_resource_delete"),
        input.clone(),
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "{status}"
    );

    let mut anonymous = Request::builder()
        .method("POST")
        .uri(odata("jc_resource_delete"))
        .header(header::CONTENT_TYPE, "application/json");
    anonymous = anonymous.header("x-unused", "1");
    let response = server::app(state.clone())
        .oneshot(
            anonymous
                .body(Body::from(input.to_string()))
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let (status, body) = post(
        &state,
        Some(EDITOR),
        &odata("jc_resource_list"),
        json!({ "kind": "Endpoint" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a read is no action: {body}");
}

/// T-6655: `$metadata` equals the committed snapshot. After a registry change rewrite it with
/// `cargo test --test edge_resource_reads_tests -- --ignored write_odata_metadata`.
#[tokio::test]
async fn the_metadata_is_the_snapshot() {
    let state = odata_world();
    let xml = metadata_text(&state).await;
    let snapshot = std::fs::read_to_string(SNAPSHOT).unwrap_or_default();
    assert!(
        xml == snapshot,
        "$metadata moved from {SNAPSHOT}; rerun with -- --ignored write_odata_metadata and read the diff"
    );
    for set in ["endpoints", "pipelines", "changes", "workspaces"] {
        assert!(
            xml.contains(&format!(r#"<EntitySet Name="{set}""#)),
            "{set}"
        );
    }
    assert!(xml.contains(r#"<ActionImport Name="jc_resource_delete""#));
    let (status, service) = get(&state, Some(VIEWER), &odata("")).await;
    assert_eq!(status, StatusCode::OK, "{service}");
    assert!(names_in(&service, "value", "/url").contains(&"endpoints".to_owned()));
}

const SNAPSHOT: &str = "tests/odata_metadata.xml";

async fn metadata_text(state: &AppState) -> String {
    let request = Request::builder()
        .uri(odata("$metadata"))
        .header(header::COOKIE, cookie(&state.config, VIEWER));
    let response = server::app(state.clone())
        .oneshot(request.body(Body::empty()).expect("a request"))
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("UTF-8")
}

#[tokio::test]
#[ignore = "writes tests/odata_metadata.xml"]
async fn write_odata_metadata() {
    let xml = metadata_text(&odata_world()).await;
    std::fs::write(SNAPSHOT, xml).expect("write the snapshot");
}

// -------------------------------------------------------------------------------------------------
// T-6645 the Endpoints page's rows over OData
// -------------------------------------------------------------------------------------------------

/// T-6645: the OData set holds the rows the Endpoints page lists (T-6169), each after the read
/// check of the project it lives in: the project's own Endpoints, those another project shares
/// with it (EP-14) and its references whose Endpoint does not show (EP-15). A person who reads
/// only this project sees no other project's Endpoint, by filter, count or key, and `$allowed`
/// and the write doors answer from the same decision.
#[tokio::test]
async fn odata_endpoints_carry_the_shared_and_gone_rows_the_page_lists_and_no_other() {
    let state = world();
    for (name, sharing) in [
        ("espoo-open", json!({ "audience": "public" })),
        (
            "espoo-listed",
            json!({ "audience": "project-list", "allowedProjects": [PROJECT] }),
        ),
        (
            "espoo-private",
            json!({ "audience": "project-list", "allowedProjects": [] }),
        ),
    ] {
        let mut spec = json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": ELSEWHERE },
                               "slug": "si6epqkx364lprho5uaigutk274r5grc" });
        if let (Some(spec), Some(sharing)) = (spec.as_object_mut(), sharing.as_object()) {
            spec.extend(sharing.clone());
        }
        state
            .mirror
            .upsert(envelope("Endpoint", name, ELSEWHERE, spec));
    }
    for (name, target) in [
        ("uses-listed", "espoo-listed"),
        ("uses-gone", "espoo-removed"),
    ] {
        state.mirror.upsert(envelope(
            "SharedSpaceReference",
            name,
            PROJECT,
            json!({ "endpointRef": { "project": ELSEWHERE, "name": target }, "alias": name }),
        ));
    }
    state.mirror.upsert(org(
        "Role",
        "endpoint-viewer",
        json!({ "rules": [{ "kinds": ["Endpoint", "SharedSpaceReference"], "verbs": ["read"] }] }),
    ));
    for (binding, user, project) in [
        ("vera-here", "vera.both@hel.fi", PROJECT),
        ("vera-there", "vera.both@hel.fi", ELSEWHERE),
        ("hana-here", "hana.here@hel.fi", PROJECT),
    ] {
        state.mirror.upsert(org(
            "RoleBinding",
            binding,
            json!({ "subjects": [{ "user": user }], "role": "endpoint-viewer",
                    "scope": { "project": project } }),
        ));
    }
    let odata = |rest: &str| format!("/api/v1/projects/{PROJECT}/odata/endpointList{rest}");
    let ids = |body: &Value| -> Vec<String> {
        let mut ids: Vec<String> = body["value"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| r["id"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        ids.sort();
        ids
    };
    let vera = Some("vera.both@hel.fi");
    let hana = Some("hana.here@hel.fi");

    // Vera reads both projects: her rows are the page's, the private Endpoint is no row.
    let (status, all) = get(&state, vera, &odata("?$count=true")).await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert_eq!(
        ids(&all),
        [
            "espoo/espoo-listed",
            "espoo/espoo-open",
            "helsinki-bikes",
            "reference:uses-gone"
        ]
    );
    assert_eq!(all["@odata.count"], json!(4));
    let row = |id: &str| {
        all["value"]
            .as_array()
            .and_then(|rows| rows.iter().find(|r| r["id"] == json!(id)))
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(row("espoo/espoo-listed")["kind"], json!("shared"));
    assert_eq!(row("espoo/espoo-listed")["source"], json!(ELSEWHERE));
    assert_eq!(row("espoo/espoo-listed")["reference"], json!("uses-listed"));
    assert_eq!(row("espoo/espoo-open")["reference"], Value::Null);
    assert_eq!(row("reference:uses-gone")["kind"], json!("gone"));
    assert_eq!(row("reference:uses-gone")["title"], json!("uses-gone"));
    assert_eq!(row("helsinki-bikes")["kind"], json!("own"));
    assert_eq!(row("helsinki-bikes")["representations"], json!("ngsi-ld"));

    // The same rows the plain lists give the page: its own list, and the other project's list
    // narrowed to what that project shares with this one.
    let (_, own) = get(
        &state,
        vera,
        &format!("/api/v1/projects/{PROJECT}/endpoints"),
    )
    .await;
    let (_, theirs) = get(
        &state,
        vera,
        &format!("/api/v1/projects/{ELSEWHERE}/endpoints"),
    )
    .await;
    let listed: Vec<String> = own["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i["metadata"]["name"].as_str().map(str::to_owned))
        .chain(
            theirs["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|i| i["metadata"]["name"] != json!("espoo-private"))
                .filter(|i| i["metadata"]["name"] != json!(format!("{ELSEWHERE}-bikes")))
                .filter_map(|i| {
                    i["metadata"]["name"]
                        .as_str()
                        .map(|n| format!("{ELSEWHERE}/{n}"))
                }),
        )
        .collect();
    for id in &listed {
        assert!(ids(&all).contains(id), "{id} missing from {all}");
    }

    let (status, shared) = get(
        &state,
        vera,
        &odata("?$filter=kind%20eq%20'shared'&$count=true"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    assert_eq!(shared["@odata.count"], json!(2), "{shared}");

    // Hana reads this project alone: no row of the other one, whatever she asks; both references
    // show as gone, as the page shows them to her.
    let (status, hers) = get(&state, hana, &odata("?$count=true")).await;
    assert_eq!(status, StatusCode::OK, "{hers}");
    assert_eq!(
        ids(&hers),
        [
            "helsinki-bikes",
            "reference:uses-gone",
            "reference:uses-listed"
        ]
    );
    let (status, probe) = get(
        &state,
        hana,
        &odata("?$filter=source%20eq%20'espoo'%20or%20startswith(id,'espoo')&$count=true"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{probe}");
    assert_eq!(probe["@odata.count"], json!(0), "{probe}");
    let (status, keyed) = get(
        &state,
        hana,
        &format!("/api/v1/projects/{PROJECT}/odata/endpointList('espoo%2Fespoo-open')"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{keyed}");
    let (status, keyed) = get(
        &state,
        vera,
        &format!("/api/v1/projects/{PROJECT}/odata/endpointList('espoo%2Fespoo-open')"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{keyed}");
    assert_eq!(keyed["source"], json!(ELSEWHERE));

    // `$allowed`: a reader may not propose, a shared row is changed where it is published, a key
    // the caller does not read is not found, as a missing one is.
    let (status, own_update) = get(
        &state,
        vera,
        &odata("/$allowed?op=update&key=helsinki-bikes"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{own_update}");
    assert_eq!(own_update["allowed"], json!(false), "{own_update}");
    assert!(
        own_update["reason"]
            .as_str()
            .unwrap_or("")
            .contains("propose"),
        "{own_update}"
    );
    let (_, shared_update) = get(
        &state,
        vera,
        &odata("/$allowed?op=update&key=espoo%2Fespoo-open"),
    )
    .await;
    assert_eq!(shared_update["allowed"], json!(false), "{shared_update}");
    assert!(
        shared_update["reason"]
            .as_str()
            .unwrap_or("")
            .contains("project espoo publishes"),
        "{shared_update}"
    );
    let (_, hidden) = get(
        &state,
        hana,
        &odata("/$allowed?op=update&key=espoo%2Fespoo-open"),
    )
    .await;
    let (_, missing) = get(
        &state,
        hana,
        &odata("/$allowed?op=update&key=espoo%2Fnothing"),
    )
    .await;
    assert_eq!(hidden["notFound"], json!(true), "{hidden}");
    assert_eq!(hidden["allowed"], missing["allowed"]);
    assert_eq!(hidden["notFound"], missing["notFound"]);
    let (status, _) = get(&state, None, &odata("/$allowed?op=read")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The write doors run the same decision before anything: a shared row is 403 with where it
    // is changed, a reader's own row 403 with the missing verb, a hidden key 404, and a proposer
    // changing a property the row does not edit 400 naming the ones it does.
    state.mirror.upsert(org(
        "Role",
        "endpoint-proposer",
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "pat-here",
        json!({ "subjects": [{ "user": "pat.proposer@hel.fi" }], "role": "endpoint-proposer",
                "scope": { "project": PROJECT } }),
    ));
    let entity = |key: &str| format!("/api/v1/projects/{PROJECT}/odata/endpointList('{key}')");
    let cases = [
        (
            "PATCH",
            vera,
            "espoo%2Fespoo-open",
            json!({ "mcp": false }),
            StatusCode::FORBIDDEN,
            "project espoo publishes",
        ),
        (
            "DELETE",
            vera,
            "reference%3Auses-gone",
            Value::Null,
            StatusCode::FORBIDDEN,
            "Stop using",
        ),
        (
            "PATCH",
            vera,
            "helsinki-bikes",
            json!({ "mcp": false }),
            StatusCode::FORBIDDEN,
            "propose",
        ),
        (
            "DELETE",
            vera,
            "helsinki-bikes",
            Value::Null,
            StatusCode::FORBIDDEN,
            "delete",
        ),
        (
            "PATCH",
            hana,
            "espoo%2Fespoo-open",
            json!({ "mcp": false }),
            StatusCode::NOT_FOUND,
            "no Endpoint",
        ),
        (
            "PATCH",
            Some("pat.proposer@hel.fi"),
            "helsinki-bikes",
            json!({ "title": "x" }),
            StatusCode::BAD_REQUEST,
            "contextSpace, audience, mcp",
        ),
        (
            "PATCH",
            Some("pat.proposer@hel.fi"),
            "helsinki-bikes",
            json!({ "mcp": "no" }),
            StatusCode::BAD_REQUEST,
            "true or false",
        ),
    ];
    for (method, who, key, body, want, says) in cases {
        let request = Request::builder()
            .method(method)
            .uri(entity(key))
            .header(
                header::COOKIE,
                cookie(&state.config, who.unwrap_or_default()),
            )
            .header("x-csrf-token", CSRF)
            .header(header::CONTENT_TYPE, "application/json")
            .body(if body.is_null() {
                Body::empty()
            } else {
                Body::from(body.to_string())
            })
            .expect("a request");
        let response = server::app(state.clone())
            .oneshot(request)
            .await
            .expect("a response");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(status, want, "{method} {key} as {who:?}: {text}");
        assert!(text.contains(says), "{method} {key} as {who:?}: {text}");
    }
}
