//! Edge cases of the read-only doors: the revision history, the federation graph, the form
//! arrangements, the two probes and the operations listing (T-2013, T-2014, T-2015, T-2016, T-2017,
//! T-2022; AG-59, MF-17, OPS-51, PF-59, R20, UI-56).
//!
//! **The contract, in one sentence:** the history and the graph of a project are reads of it, refused as
//! the `404` of a project that is not there; the forms and the probes are the installation's own and
//! say nothing about its configuration; and the operations listing is what this caller could actually
//! call, never the whole registry.
//!
//! The happy paths live in `export_api_tests.rs`, `federation_graph_tests.rs`, `forms_api_tests.rs` and
//! `ops_registry_tests.rs`. This file is the other side: a limit nobody serves, a stranger, a probe read
//! by whoever reaches the port.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::auth::csrf::CSRF_COOKIE;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "helsinki";
const ELSEWHERE: &str = "espoo";
const CSRF: &str = "csrf-token-edge-doors";
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

/// Two projects, a reader bound to one of them, and the pieces a graph is drawn from.
fn state_with_two_projects() -> AppState {
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

/// The federation graph of `project`, through `jc_federation_graph` (its one door since T-4453).
async fn federation_graph(
    state: &AppState,
    email: Option<&str>,
    project: &str,
) -> (StatusCode, Value) {
    let csrf = format!("{CSRF_COOKIE}={CSRF}");
    let cookies = email.map_or(csrf, |email| cookie(&state.config, email));
    let request = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/projects/{project}/ops/jc_federation_graph"
        ))
        .header(header::COOKIE, cookies)
        .header("x-csrf-token", CSRF)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

// -------------------------------------------------------------------------------------------------
// T-2013 `revisions`
// -------------------------------------------------------------------------------------------------

/// MF-17, PF-59: the history of a project says who changed what, so it is a read of the project — and
/// the number of commits asked for has to be one this Portal serves. A limit of nothing and a limit
/// past the ceiling are refused here, before the forge is asked, and the commits it does ask for are
/// the ones under this project's own path.
#[tokio::test]
async fn a_history_is_a_read_of_one_project_and_of_as_many_commits_as_it_serves() {
    let forge = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/commits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "sha": "0f1e2d3c4b5a69788796a5b4c3d2e1f0a1b2c3d4",
            "commit": {
                "message": "add the bikes endpoint",
                "author": { "name": "Jana", "email": "jana@hel.fi",
                            "date": "2026-09-18T09:12:03Z" }
            }
        }])))
        .mount(&forge)
        .await;
    let client = GiteaClient::new(
        forge.uri().parse().expect("a url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("a client");
    let state = state_with_two_projects().with_gitea(Arc::new(client));

    // A caller no binding covers, and a name that could not be a project.
    for (who, project) in [
        (STRANGER, PROJECT),
        (MEMBER, "Helsinki"),
        (MEMBER, "helsinki_1"),
        (MEMBER, ELSEWHERE),
    ] {
        let (status, body) = get(
            &state,
            Some(who),
            &format!("/api/v1/projects/{project}/revisions"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who} {project}: {body}");
    }

    // A limit that is not a number the route serves.
    for limit in ["0", "1000", "-1", "abc", "1.5", ""] {
        let (status, body) = get(
            &state,
            Some(MEMBER),
            &format!("/api/v1/projects/{PROJECT}/revisions?limit={limit}"),
        )
        .await;
        assert!(
            status.is_client_error(),
            "limit={limit:?} answered {status}: {body}",
        );
    }

    // The member's own history: answered, and the forge was asked for the commits of this project's
    // own path — never for the whole repository.
    let (status, list) = get(
        &state,
        Some(MEMBER),
        &format!("/api/v1/projects/{PROJECT}/revisions?limit=5"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"].as_array().map(Vec::len), Some(1), "{list}");
    let asked: Vec<String> = forge
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.url.path().ends_with("/commits"))
        .map(|request| request.url.query().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(
        asked[0].contains(&format!("projects%2F{PROJECT}"))
            || asked[0].contains(&format!("projects/{PROJECT}")),
        "the history was not narrowed to the project's path: {}",
        asked[0],
    );
}

// -------------------------------------------------------------------------------------------------
// T-2014 `jc_federation_graph` (the route `get_graph` until T-4453)
// -------------------------------------------------------------------------------------------------

/// AG-59, PF-59: the federation graph is a project's topology, so it is a read of the project and it
/// draws that project alone — the node of the endpoint next door is in the graph of the project next
/// door, and a caller no binding covers is told the project is not there.
#[tokio::test]
async fn a_federation_graph_draws_one_project_and_is_read_by_its_members() {
    let state = state_with_two_projects();

    let (status, body) = federation_graph(&state, Some(STRANGER), PROJECT).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        !body.to_string().contains("helsinki-bikes"),
        "the 404 carried a node of the graph: {body}",
    );

    let (status, graph) = federation_graph(&state, Some(MEMBER), PROJECT).await;
    assert_eq!(status, StatusCode::OK, "{graph}");
    let text = graph.to_string();
    assert!(text.contains("helsinki-bikes"), "{graph}");
    assert!(
        !text.contains("espoo"),
        "the graph of one project drew another's: {graph}",
    );

    // The project next door, which this reader holds nothing in.
    let (status, body) = federation_graph(&state, Some(MEMBER), ELSEWHERE).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // And no session at all: the graph is behind the same session as every other read.
    let (status, _) = federation_graph(&state, None, PROJECT).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// -------------------------------------------------------------------------------------------------
// T-2015 `list_forms` and T-2022 `list_ops`
// -------------------------------------------------------------------------------------------------

/// UI-56: the form arrangements are the organization's, so a `UiSchema` filed under a project is not in
/// the list — nobody arranges the installation's dialogs by choosing a namespace. The operations listing
/// beside it is the caller's own: it holds what this caller could call in this project, and a caller no
/// binding covers is offered nothing to call.
#[tokio::test]
async fn the_forms_are_the_organizations_and_the_operations_are_the_callers() {
    let state = state_with_two_projects();
    state.mirror.upsert(org(
        "UiSchema",
        "endpoint-form",
        json!({ "kind": "Endpoint", "groups": [{ "title": "Basics", "fields": ["metadata.name"] }] }),
    ));
    state.mirror.upsert(envelope(
        "UiSchema",
        "planted-in-a-project",
        PROJECT,
        json!({ "kind": "Endpoint", "groups": [] }),
    ));

    let (status, forms) = get(&state, Some(MEMBER), "/api/v1/forms").await;
    assert_eq!(status, StatusCode::OK, "{forms}");
    let names: Vec<&str> = forms["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["metadata"]["name"].as_str())
        .collect();
    assert_eq!(names, vec!["endpoint-form"], "{forms}");

    // A form arrangement is chrome, not data: every session gets the same one, and no session gets
    // none of it.
    let (status, theirs) = get(&state, Some(STRANGER), "/api/v1/forms").await;
    assert_eq!(status, StatusCode::OK, "{theirs}");
    assert_eq!(theirs["items"], forms["items"], "{theirs}");
    let (status, _) = get(&state, None, "/api/v1/forms").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The operations listing: the member is offered what their bindings allow, a stranger is offered
    // nothing, and a name that could not be a project is the 404 of a project that is not there.
    let (status, mine) = get(
        &state,
        Some(MEMBER),
        &format!("/api/v1/projects/{PROJECT}/ops"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    let mine_names: Vec<&str> = mine
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|op| op["name"].as_str())
        .collect();
    assert!(!mine_names.is_empty(), "{mine}");

    let (status, theirs) = get(
        &state,
        Some(STRANGER),
        &format!("/api/v1/projects/{PROJECT}/ops"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{theirs}");
    let their_names: Vec<&str> = theirs
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|op| op["name"].as_str())
        .collect();
    for name in &their_names {
        assert!(
            mine_names.contains(name),
            "a stranger is offered '{name}', which a member of the project is not",
        );
    }
    assert!(
        their_names.len() < mine_names.len(),
        "a stranger is offered as much as a member: {their_names:?}",
    );

    for project in ["Helsinki", "helsinki_1", "%2e%2e"] {
        let (status, body) = get(
            &state,
            Some(MEMBER),
            &format!("/api/v1/projects/{project}/ops"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{project}: {body}");
    }
}

// -------------------------------------------------------------------------------------------------
// T-2016 `health` and T-2017 `ready`
// -------------------------------------------------------------------------------------------------

/// OPS-51: the two probes are read by a kubelet, which carries no session, so they answer without one —
/// and because they answer without one, they say nothing about this installation: the name and the
/// version of the software, `ok`, `ready` or `loading`, and not one URL, client id, realm or branding
/// value.
#[tokio::test]
async fn the_two_probes_answer_without_a_session_and_say_nothing_about_the_installation() {
    let mut config = Config::for_tests();
    config.gateway_client_id = Some("context-gateway".into());
    config.org_domain = Some("hel.fi".into());
    let state = AppState::new(config, None);

    for uri in ["/api/v1/health", "/api/v1/ready"] {
        let (status, body) = get(&state, None, uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        let text = body.to_string();
        for never in [
            "hel.fi",
            "context-gateway",
            "localhost:8080",
            "token",
            "secret",
            "cookie",
        ] {
            assert!(!text.contains(never), "{uri} carried {never:?}: {text}");
        }
        // A session changes nothing about either answer.
        let (with_session, same) = get(&state, Some(MEMBER), uri).await;
        assert_eq!(with_session, StatusCode::OK, "{uri}");
        assert_eq!(same, body, "{uri} answers a session differently");
    }

    // What each one does say, and nothing more: three fields and one.
    let (_, health) = get(&state, None, "/api/v1/health").await;
    assert_eq!(health["status"], json!("ok"), "{health}");
    assert_eq!(
        health.as_object().map(|fields| fields.len()),
        Some(3),
        "the health answer grew a field: {health}",
    );
    let (_, ready) = get(&state, None, "/api/v1/ready").await;
    // No forge is configured here, so there is no repository to wait for.
    assert_eq!(ready, json!({ "status": "ready" }), "{ready}");
}
