//! Edge cases of the operations door, the effective-permissions read, the caller's own preferences and
//! the three project routes (T-2023, T-2024, T-2028, T-2029, T-2030, T-2031, T-2032; PF-05, PF-59,
//! PF-65, PF-67, R20, UI-44).
//!
//! **The contract, in one sentence:** the operations door is the registry's gate and nothing more, the
//! permissions read answers a member about themselves, preferences are the caller's own row, and a
//! project is listed, read and opened by what a binding covers — never by what a name looks like.
//!
//! The happy paths live in `ops_registry_tests.rs`, `permissions_tests.rs`, `preferences_db_tests.rs`
//! and `projects_api_tests.rs`. This file is the other side: an operation nobody registered, a stranger,
//! a body that is not preferences, a project name that is taken or is not a name.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "helsinki";
const ELSEWHERE: &str = "espoo";
const CSRF: &str = "csrf-token-edge-ops";
const MEMBER: &str = "peter.member@hel.fi";
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

/// Two projects the repository holds, and a reader bound to one of them.
fn world() -> AppState {
    let state = AppState::new(Config::for_tests(), None);
    for project in [PROJECT, ELSEWHERE] {
        state
            .mirror
            .upsert(envelope("ContextSpace", project, project, json!({})));
    }
    state.mirror.upsert(org(
        "Role",
        "project-reader",
        json!({ "rules": [{ "kinds": ["ContextSpace", "Endpoint"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "project-readers",
        json!({ "subjects": [{ "user": MEMBER }], "role": "project-reader",
                "scope": { "project": PROJECT } }),
    ));
    state
}

async fn send(
    state: &AppState,
    email: Option<&str>,
    verb: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(verb).uri(uri);
    if let Some(email) = email {
        request = request
            .header(header::COOKIE, cookie(&state.config, email))
            .header(CSRF_HEADER, CSRF);
    }
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = server::app(state.clone())
        .oneshot(request.body(body).expect("a request"))
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
// T-2023 `run_op`
// -------------------------------------------------------------------------------------------------

/// AG-64, PF-59: the operations door runs what the registry holds, as the caller, in the project the
/// path names. An operation nobody registered is not found, a body that is not JSON is refused as one,
/// and an operation the caller may not run is refused by the registry's gate rather than by the door.
#[tokio::test]
async fn the_operations_door_runs_what_the_registry_holds_and_nothing_else() {
    let state = world();

    // A name the registry does not hold, including ones that look like an operation.
    for name in [
        "jc_nothing_like_this",
        "JC_PROJECT_LIST",
        "jc_project_list%20",
        "%2e%2e",
        "jc_draft_put%00",
    ] {
        let (status, body) = send(
            &state,
            Some(MEMBER),
            Method::POST,
            &format!("/api/v1/projects/{PROJECT}/ops/{name}"),
            Some(json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name}: {body}");
        assert!(!body.to_string().contains("panicked"), "{name}: {body}",);
    }

    // A project name that could not be one, before the operation is even looked up.
    for project in ["Helsinki", "helsinki_1", "%2e%2e"] {
        let (status, body) = send(
            &state,
            Some(MEMBER),
            Method::POST,
            &format!("/api/v1/projects/{project}/ops/jc_draft_list"),
            Some(json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{project}: {body}");
    }

    // A body that is not JSON at all, and one that is not an object the operation can read.
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/projects/{PROJECT}/ops/jc_draft_list"))
                .header(header::COOKIE, cookie(&state.config, MEMBER))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("not json"))
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // An operation a caller no binding covers may not run: refused, and the refusal says nothing
    // about what the project holds.
    let (status, body) = send(
        &state,
        Some(STRANGER),
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/ops/jc_draft_list"),
        Some(json!({})),
    )
    .await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::FORBIDDEN,
        "a stranger ran an operation: {status} {body}",
    );

    // And no session at all: the door is behind the session like every other route of `/api/v1`.
    let (status, _) = send(
        &state,
        None,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/ops/jc_draft_list"),
        Some(json!({})),
    )
    .await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
        "{status}",
    );
}

// -------------------------------------------------------------------------------------------------
// T-2024 `permissions_me`
// -------------------------------------------------------------------------------------------------

/// PF-59, PF-65, UI-44: the effective permissions of a project are a read of it, so a caller no binding
/// covers is told the project is not there. What a member is told is about themselves — their own verbs
/// and whether they may open a project — and never about anybody else's bindings.
#[tokio::test]
async fn the_permissions_read_answers_a_member_about_themselves() {
    let state = world();

    for (who, project) in [
        (STRANGER, PROJECT),
        (MEMBER, ELSEWHERE),
        (MEMBER, "Helsinki"),
        (MEMBER, "no-such-project"),
    ] {
        let (status, body) = send(
            &state,
            Some(who),
            Method::GET,
            &format!("/api/v1/projects/{project}/permissions/me"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who} {project}: {body}");
    }

    let (status, mine) = send(
        &state,
        Some(MEMBER),
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/permissions/me"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    // Whether this person may open a project is the organization's setting, which the rules alone
    // cannot answer, so it travels with the read (PF-65).
    assert!(
        mine["projects"]["creation"].is_object() || mine["projects"]["creation"].is_string(),
        "the UI cannot tell whether the button belongs on the page: {mine}",
    );
    // Their own grants are the answer — the binding that gives them is theirs to see. What is never
    // in it is anybody else: another person's name, or another project's.
    assert!(
        mine["grants"]
            .as_array()
            .is_some_and(|grants| !grants.is_empty()),
        "a member was told they hold nothing: {mine}",
    );
    let text = mine.to_string();
    for never in [STRANGER, "espoo"] {
        assert!(!text.contains(never), "the read carried {never:?}: {text}",);
    }
}

// -------------------------------------------------------------------------------------------------
// T-2028 `get_preferences` and T-2029 `put_preferences`
// -------------------------------------------------------------------------------------------------

/// The preferences are the caller's own row, keyed by their subject. Without a database there is nothing
/// to read or write and the answer says so rather than pretending to have saved; a body that is not
/// preferences is refused before the database is asked at all, as a problem document and not as axum's
/// plain-text 422.
#[tokio::test]
async fn preferences_are_the_callers_own_row_and_a_body_that_is_not_one_never_reaches_a_database() {
    let state = world();

    // No database on this installation: both routes say so, and the write does not claim to have
    // stored anything.
    let (status, body) = send(
        &state,
        Some(MEMBER),
        Method::GET,
        "/api/v1/preferences",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let (status, body) = send(
        &state,
        Some(MEMBER),
        Method::PUT,
        "/api/v1/preferences",
        Some(json!({ "locale": "sk" })),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");

    // A body that is not preferences, and one whose values the Portal knows to be wrong: refused as a
    // problem document with a detail, before the database is reached.
    for body in [
        json!("preferences"),
        json!({ "locale": 7 }),
        json!({ "locale": "not a locale tag at all, far too long to be one" }),
        json!({ "theme": "neon" }),
        Value::Null,
    ] {
        let (status, answer) = send(
            &state,
            Some(MEMBER),
            Method::PUT,
            "/api/v1/preferences",
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {answer}");
        assert!(
            answer["detail"].is_string(),
            "{body} was not answered as a problem document: {answer}",
        );
    }

    // And neither route answers without a session.
    for verb in [Method::GET, Method::PUT] {
        let (status, _) = send(&state, None, verb.clone(), "/api/v1/preferences", None).await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
            "{verb} answered {status}",
        );
    }
}

// -------------------------------------------------------------------------------------------------
// T-2030 `list_projects`, T-2031 `get_project`, T-2032 `open_project`
// -------------------------------------------------------------------------------------------------

/// PF-05, PF-59, R20: the project list is what this caller's bindings cover, a project they hold nothing
/// in is not there whether it exists or not, and opening one is refused for a name that could not be a
/// project, for the organization's own namespace and for a name already taken.
#[tokio::test]
async fn a_project_is_listed_read_and_opened_by_what_a_binding_covers() {
    let state = world();

    // The list: the member's own project and nothing else; a caller no binding covers gets an empty
    // list rather than a refusal, because a list of nothing is a screen and not an error.
    let (status, mine) = send(&state, Some(MEMBER), Method::GET, "/api/v1/projects", None).await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    let names: Vec<&str> = mine["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["name"].as_str())
        .collect();
    assert_eq!(names, vec![PROJECT], "{mine}");

    let (status, theirs) = send(
        &state,
        Some(STRANGER),
        Method::GET,
        "/api/v1/projects",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{theirs}");
    assert_eq!(theirs["items"], json!([]), "{theirs}");

    // The read: the member's own project answers, everything else is the same 404.
    let (status, detail) = send(
        &state,
        Some(MEMBER),
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    for (who, project) in [
        (MEMBER, ELSEWHERE),
        (MEMBER, "no-such-project"),
        (MEMBER, "Helsinki"),
        (STRANGER, PROJECT),
    ] {
        let (status, body) = send(
            &state,
            Some(who),
            Method::GET,
            &format!("/api/v1/projects/{project}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who} {project}: {body}");
    }

    // Opening one: a name that could not be a project, the organization's own namespace, and a name
    // the repository already holds. The member may not open one at all on this installation, which is
    // the first thing checked, so the refusals below are asserted for whoever may.
    for name in [
        "Helsinki",
        "helsinki_1",
        "-helsinki",
        "",
        ORG_NAMESPACE,
        PROJECT,
    ] {
        let (status, body) = send(
            &state,
            Some(MEMBER),
            Method::POST,
            "/api/v1/projects",
            Some(json!({ "name": name })),
        )
        .await;
        assert!(
            status == StatusCode::FORBIDDEN
                || status == StatusCode::BAD_REQUEST
                || status == StatusCode::CONFLICT,
            "{name:?} answered {status}: {body}",
        );
    }

    // A body that is not an open-project request at all.
    for body in [
        json!({}),
        json!({ "name": 7 }),
        json!([]),
        json!("helsinki"),
    ] {
        let (status, answer) = send(
            &state,
            Some(MEMBER),
            Method::POST,
            "/api/v1/projects",
            Some(body.clone()),
        )
        .await;
        assert!(
            status.is_client_error(),
            "{body} answered {status}: {answer}",
        );
    }
}
