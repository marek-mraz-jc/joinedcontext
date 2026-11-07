//! Edge cases of the four assistant doors of a project (T-1986, T-1987, T-1988, T-1989; AG-58,
//! AG-70, AG-77, EP-72, PF-50, PF-59).
//!
//! **The contract, in one sentence:** the catalogue, the share and the conversation are a member's,
//! refused as `404` for a project a caller cannot read and as `403` for a verb they do not hold — and
//! what they refuse, they refuse before anything is rendered, written or started.
//!
//! The happy paths live next door: `catalog_search_tests.rs` for what a question matches,
//! `assistant_share_tests.rs` for the manifests a share renders, `assistant_ask_tests.rs` and
//! `assistant_access_tests.rs` for a conversation and the two halves of its access. This file is the
//! other side: an empty question, a scope nobody defined, a form the Portal could not have, a
//! continuation of somebody else's run, a project name that is not a name.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const PROJECT: &str = "helsinki";
const ELSEWHERE: &str = "espoo";
const MEMBER: &str = "demo.builder";
const STRANGER: &str = "demo.stranger";
const EDITOR: &str = "demo.editor";
const VIEWER: &str = "demo.viewer";
const CSRF: &str = "csrf-token-edge-assistant";

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

/// Helsinki, whose `devs` may read and propose; Espoo, where nobody in this file holds anything.
fn mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    for project in [PROJECT, ELSEWHERE] {
        mirror.upsert(envelope(
            "ContextSpace",
            project,
            project,
            json!({ "dataModelRef": format!("{project}-model") }),
        ));
        mirror.upsert(envelope(
            "Endpoint",
            &format!("{project}-bikes"),
            project,
            json!({
                "contextSpaceRef": project,
                "slug": "si6epqkx364lprho5uaigutk274r5grb",
                "audience": "project",
                "enabledRepresentations": ["ngsi-ld"]
            }),
        ));
    }
    mirror.upsert(envelope(
        "Role",
        "proposer",
        "org",
        json!({ "rules": [{ "kinds": ["App", "Endpoint", "ContextSpace", "DataModel"],
                            "verbs": ["read", "propose"] }] }),
    ));
    mirror.upsert(envelope(
        "RoleBinding",
        "devs-here",
        "org",
        json!({ "subjects": [{ "group": "devs" }], "role": "proposer",
                "scope": { "project": PROJECT } }),
    ));
    // The seeded editor's shape: proposes data objects, no App (T-6464).
    mirror.upsert(envelope(
        "Role",
        "data-editor",
        "org",
        json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read", "propose"] }] }),
    ));
    mirror.upsert(envelope(
        "RoleBinding",
        "editors-here",
        "org",
        json!({ "subjects": [{ "group": "editors" }], "role": "data-editor",
                "scope": { "project": PROJECT } }),
    ));
    mirror.upsert(envelope(
        "Role",
        "reader",
        "org",
        json!({ "rules": [{ "kinds": ["App", "Endpoint"], "verbs": ["read"] }] }),
    ));
    mirror.upsert(envelope(
        "RoleBinding",
        "viewers-here",
        "org",
        json!({ "subjects": [{ "group": "viewers" }], "role": "reader",
                "scope": { "project": PROJECT } }),
    ));
    mirror.upsert(envelope(
        "AgentProfile",
        "app-builder",
        "org",
        json!({
            "role": "builder",
            "runtime": {
                "image": "ghcr.io/all-hands-ai/agent-server:v1.4.0",
                "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
            },
            "model": { "provider": "anthropic", "name": "claude-sonnet-5", "maxTokensPerRun": 400000 },
            "limits": {
                "stepsPerRun": 120,
                "wallClock": "PT20M",
                "concurrentRunsPerOrganization": 2,
                "requestsPerMinute": 60,
                "maxResponseBytes": 2097152
            },
            "egress": { "allowedHosts": ["registry.npmjs.org"] },
            "tools": ["shell", "npm", "git"],
            "workspace": { "cpu": "1", "memory": "2Gi", "ephemeralStorage": "4Gi" }
        }),
    ));
    mirror
}

fn cookie(config: &Config, username: &str, groups: &[&str]) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{username}"),
            username: username.into(),
            email: Some(format!("{username}@hel.fi")),
            name: None,
            roles: Vec::new(),
            groups: groups.iter().map(|group| group.to_string()).collect(),
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
        .map(|raw| raw.split(';').next().unwrap_or_default().to_string())
        .collect();
    parts.push(format!("__Host-jc_csrf={CSRF}"));
    parts.join("; ")
}

/// One Portal with an agent runner configured, so a conversation is refused on its own terms and
/// never for want of a runner.
fn world() -> (AppState, axum::Router, Config, String, String) {
    let config = Config::from_vars(|key| {
        match key {
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some("http://jc-agent-proxy.agents.svc.cluster.local:8080"),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("the agent runner block is complete");
    let member = cookie(&config, MEMBER, &["devs"]);
    let stranger = cookie(&config, STRANGER, &[]);
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    (state.clone(), server::app(state), config, member, stranger)
}

async fn get(app: &axum::Router, cookie: &str, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
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

async fn post(app: &axum::Router, cookie: &str, uri: &str, body: &Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header("x-csrf-token", CSRF)
                .header(header::CONTENT_TYPE, "application/json")
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

/// Asks `jc_catalog_search` (T-4453: the one door the search has) what `query`, written as the
/// old route's query string, asked.
async fn search(
    app: &axum::Router,
    cookie: &str,
    project: &str,
    query: &str,
) -> (StatusCode, Value) {
    let input: serde_json::Map<String, Value> = url::form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), Value::String(v.into_owned())))
        .collect();
    let uri = format!("/api/v1/projects/{project}/ops/jc_catalog_search");
    post(app, cookie, &uri, &Value::Object(input)).await
}

// -------------------------------------------------------------------------------------------------
// T-1986 `jc_catalog_search` (the route `get_catalog` until T-4453)
// -------------------------------------------------------------------------------------------------

/// AG-58, PF-59: a question with no words is a 422 (the schema requires a non-blank `q`, T-6274) to
/// a reader of the project and the plain 404 to a stranger, the answer a stranger gets for any question, so an empty `q` is not a way to ask
/// which projects exist (the registry checks the project before the input, T-4453).
#[tokio::test]
async fn a_question_with_no_words_tells_a_stranger_nothing_a_real_question_does_not() {
    let (_, app, _, member, stranger) = world();
    let (_, real) = search(&app, &stranger, PROJECT, "q=bikes").await;

    for query in ["", "q=", "q=%20%20", "q=%09", "scope=Endpoint", "q=&scope="] {
        let (mine, my_body) = search(&app, &member, PROJECT, query).await;
        let (theirs, their_body) = search(&app, &stranger, PROJECT, query).await;
        assert_eq!(mine, StatusCode::UNPROCESSABLE_ENTITY, "{query}: {my_body}");
        assert_eq!(theirs, StatusCode::NOT_FOUND, "{query}: {their_body}");
        assert_eq!(
            their_body, real,
            "{query} told a stranger more than a question does"
        );
    }

    let (status, body) = search(&app, &member, "no-such-project", "q=").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

/// PF-59, R20: a catalogue of a project a caller cannot read is a `404`, like a project that is not
/// there, and it names nothing the project holds. A name that could not be a project is the same 404.
#[tokio::test]
async fn a_catalogue_of_a_project_a_caller_cannot_read_names_nothing_of_it() {
    let (_, app, _, member, stranger) = world();

    // A real project, a real question, a caller with no binding in it.
    let (status, body) = search(&app, &stranger, PROJECT, "q=bikes").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let text = body.to_string();
    for leaked in [
        "helsinki-bikes",
        "si6epqkx364lprho5uaigutk274r5grb",
        "helsinki-model",
    ] {
        assert!(!text.contains(leaked), "the 404 carried {leaked:?}: {text}");
    }

    // The member's own project answers, and the project next door — which they hold nothing in — does
    // not: one binding, one catalogue.
    let (status, body) = search(&app, &member, PROJECT, "q=bikes").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = search(&app, &member, ELSEWHERE, "q=bikes").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        !body.to_string().contains("espoo-bikes"),
        "the 404 carried the other project's endpoint: {body}",
    );

    // Names that could not be a project at all: the same answer, never a panic and never a path.
    let too_long = "h".repeat(70);
    for name in [
        "Helsinki",
        "helsinki_1",
        "..%2Fespoo",
        "%2e%2e",
        "-helsinki",
        too_long.as_str(),
    ] {
        let (status, body) = search(&app, &member, name, "q=bikes").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name}: {body}");
    }
}

/// AG-58: `scope` is one of the three kinds the catalogue holds. Anything else is a 400 that names
/// them, and the check runs after the project's, so a stranger learns nothing from a bad scope either.
#[tokio::test]
async fn a_scope_is_one_of_the_three_kinds_the_catalogue_holds() {
    let (_, app, _, member, stranger) = world();

    // `Endpoint%20` is absent on purpose: the scope is trimmed before it is compared, so a
    // trailing space is the kind and not a mistake.
    for scope in [
        "Pipeline",
        "endpoint",
        "ENDPOINT",
        "*",
        "App",
        "Endpoint,DataModel",
    ] {
        let (status, body) =
            search(&app, &member, PROJECT, &format!("q=bikes&scope={scope}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{scope}: {body}");
        let detail = body["detail"].as_str().unwrap_or_default();
        for known in ["Endpoint", "ContextSpace", "DataModel"] {
            assert!(
                detail.contains(known),
                "{scope} did not name {known}: {detail}"
            );
        }
    }

    // A stranger's bad scope is the project's 404: the read is checked first.
    let (status, _) = search(&app, &stranger, PROJECT, "q=bikes&scope=Pipeline").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // No scope, an empty scope and a legal one: all answered, and the legal one narrows to its kind.
    for query in ["q=bikes", "q=bikes&scope=", "q=bikes&scope=Endpoint"] {
        let (status, body) = search(&app, &member, PROJECT, query).await;
        assert_eq!(status, StatusCode::OK, "{query}: {body}");
    }
    let (_, narrowed) = search(&app, &member, PROJECT, "q=bikes&scope=DataModel").await;
    for item in narrowed["items"].as_array().into_iter().flatten() {
        assert_eq!(item["kind"], json!("DataModel"), "{narrowed}");
    }
}

// -------------------------------------------------------------------------------------------------
// T-1987 `propose_endpoint`
// -------------------------------------------------------------------------------------------------

/// EP-72, PF-50: a share is rendered for a caller who may propose an Endpoint here and refused for
/// everybody else — and either way nothing is written, because a proposal is a document the person
/// submits themselves.
#[tokio::test]
async fn a_share_is_rendered_for_a_member_refused_for_a_stranger_and_never_written() {
    let (state, app, _, member, stranger) = world();
    let uri = format!("/api/v1/projects/{PROJECT}/assistant/propose-endpoint");
    let share = json!({
        "contextSpace": PROJECT,
        "name": "helsinki-open-bikes",
        "audience": "project-list",
        "allowedProjects": [ELSEWHERE],
        "entityTypes": ["BikeHireDockingStation"],
    });

    let (status, body) = post(&app, &member, &uri, &share).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["endpoint"]["kind"], json!("Endpoint"), "{body}");

    let (status, body) = post(&app, &stranger, &uri, &share).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // A name that could not be a project: not found, before a caller's rights are consulted.
    let (status, body) = post(
        &app,
        &member,
        "/api/v1/projects/Helsinki/assistant/propose-endpoint",
        &share,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // Rendered, never written: the mirror holds what it held, and no change was opened.
    assert!(
        state
            .mirror
            .get(PROJECT, "Endpoint", "helsinki-open-bikes")
            .is_none(),
        "a share wrote an Endpoint",
    );
}

/// EP-72: what cannot be rendered is one message a person can act on. A name that is not a label, a
/// space that is not one, an audience nobody defined, and a project list that is empty when the
/// audience needs one: each is a 400, and none of them is a half-rendered proposal.
#[tokio::test]
async fn what_the_share_cannot_render_is_one_message_and_never_half_a_proposal() {
    let (_, app, _, member, _) = world();
    let uri = format!("/api/v1/projects/{PROJECT}/assistant/propose-endpoint");

    for (params, expected) in [
        (json!({ "contextSpace": PROJECT, "name": "" }), "name"),
        (
            json!({ "contextSpace": PROJECT, "name": "Helsinki Bikes" }),
            "name",
        ),
        (
            json!({ "contextSpace": PROJECT, "name": "a".repeat(70) }),
            "name",
        ),
        (
            json!({ "contextSpace": "", "name": "helsinki-open" }),
            "contextSpace",
        ),
        (
            json!({ "contextSpace": PROJECT, "name": "helsinki-open", "audience": "everyone" }),
            "audience",
        ),
        (
            json!({ "contextSpace": PROJECT, "name": "helsinki-open",
                    "audience": "project-list", "allowedProjects": [] }),
            "allowedProjects",
        ),
        (
            json!({ "contextSpace": PROJECT, "name": "helsinki-open",
                    "audience": "project-list", "allowedProjects": ["Espoo"] }),
            "allowedProjects",
        ),
    ] {
        let (status, body) = post(&app, &member, &uri, &params).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{params}: {body}");
        let detail = body["detail"].as_str().unwrap_or_default();
        assert!(
            detail.contains(expected),
            "{params} did not say what is wrong with {expected}: {detail}",
        );
        assert!(body.get("endpoint").is_none(), "{params}: {body}");
    }

    // A body that is not a share at all: refused by the extractor, and never as a rendered proposal.
    for shapeless in [json!([]), json!("share"), Value::Null, json!({ "name": 7 })] {
        let (status, body) = post(&app, &member, &uri, &shapeless).await;
        assert!(
            status.is_client_error(),
            "{shapeless} answered {status}: {body}"
        );
        assert!(body.get("endpoint").is_none(), "{shapeless}: {body}");
    }
}

// -------------------------------------------------------------------------------------------------
// T-1988 `start_conversation`
// -------------------------------------------------------------------------------------------------

/// PF-50: a person who may propose nothing in a project starts no conversation in it, a reader of
/// the project included (AG-67), and the refusal comes before the message is read: an empty
/// message from a stranger is still the 403.
#[tokio::test]
async fn a_person_who_may_propose_nothing_starts_no_conversation() {
    let (state, app, config, _, stranger) = world();
    let viewer = cookie(&config, VIEWER, &["viewers"]);
    let uri = format!("/api/v1/projects/{PROJECT}/assistant/conversations");

    for who in [&stranger, &viewer] {
        for message in ["", "   ", "what data do we have?"] {
            let (status, body) = post(&app, who, &uri, &json!({ "message": message })).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{message:?}: {body}");
        }
    }
    let (_, refused) = post(&app, &viewer, &uri, &json!({ "message": "hello" })).await;
    assert!(
        refused["detail"]
            .as_str()
            .is_some_and(|d| d.contains("propose on some kind")),
        "the refusal says what is missing: {refused}"
    );
    assert!(
        state
            .agents
            .list_runs(PROJECT, 100)
            .await
            .expect("the store")
            .is_empty(),
        "a refused conversation created a run",
    );
}

/// T-6464: an editor who proposes data but no App asks about their own data and keeps talking.
/// What ends in an App stays refused, on the path and on a run that writes one.
#[tokio::test]
async fn an_editor_without_an_app_grant_converses_and_builds_no_app() {
    let (state, app, config, _, _) = world();
    let editor = cookie(&config, EDITOR, &["editors"]);
    let uri = format!("/api/v1/projects/{PROJECT}/assistant/conversations");

    for body in [
        json!({ "message": "Which endpoints publish bikes?" }),
        json!({ "message": "", "path": "find-data" }),
        json!({ "message": "", "path": "share-data" }),
    ] {
        let (status, run) = post(&app, &editor, &uri, &body).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}: {run}");
    }

    let (status, refused) = post(
        &app,
        &editor,
        &uri,
        &json!({ "message": "", "path": "build-app" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert!(refused["detail"].to_string().contains("App"), "{refused}");

    let (_, run) = post(&app, &editor, &uri, &json!({ "message": "bikes?" })).await;
    let id = run["id"].as_str().expect("a run id").to_owned();
    let runs = format!("/api/v1/projects/{PROJECT}/agent-runs");
    let (status, answer) = post(
        &app,
        &editor,
        &format!("{runs}/{id}/messages"),
        &json!({ "text": "and scooters?" }),
    )
    .await;
    assert_ne!(status, StatusCode::FORBIDDEN, "a follow-up: {answer}");
    let (status, answer) = post(&app, &editor, &format!("{runs}/{id}/cancel"), &json!({})).await;
    assert_ne!(status, StatusCode::FORBIDDEN, "a cancel: {answer}");

    // A run that writes an App is not a conversation: the same person is refused on it.
    let mut build = state
        .agents
        .get_run(&id)
        .await
        .expect("the store")
        .expect("the run");
    build.id = format!("{id}-b");
    build.kind = "app".to_owned();
    state.agents.create_run(&build).await.expect("stored");
    let (status, answer) = post(
        &app,
        &editor,
        &format!("{runs}/{}/messages", build.id),
        &json!({ "text": "publish it" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");

    // A viewer's message to a run is refused before the id is looked up.
    let viewer = cookie(&config, VIEWER, &["viewers"]);
    let (status, answer) = post(
        &app,
        &viewer,
        &format!("{runs}/{id}/messages"),
        &json!({ "text": "hello" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{answer}");
}

/// T-2463: a conversation whose run has not finished never stands in the way of the next one.
/// On dev a hung run left the owner with "i cannot create a new conversaion"; the server side of
/// that is this: the same person, the same project, a second and a third conversation, each its
/// own run, with the first still unfinished and then cancelled.
#[tokio::test]
async fn an_unfinished_conversation_does_not_block_starting_another() {
    let (state, app, _, member, _) = world();
    let uri = format!("/api/v1/projects/{PROJECT}/assistant/conversations");

    let (status, first) = post(
        &app,
        &member,
        &uri,
        &json!({ "message": "Which datasets say anything about bikes?" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first}");
    let first = first["id"].as_str().expect("a run id").to_owned();
    let run = state
        .agents
        .get_run(&first)
        .await
        .expect("the store")
        .expect("the run");
    assert!(
        !matches!(
            run.status.as_str(),
            "published" | "failed" | "cancelled" | "expired"
        ),
        "the first run is still going when the second is asked for: {}",
        run.status
    );

    let (status, second) = post(
        &app,
        &member,
        &uri,
        &json!({ "message": "Which datasets say anything about bikes?" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{second}");
    assert_ne!(
        second["id"],
        json!(first),
        "a new conversation is a new run"
    );

    let (status, body) = post(
        &app,
        &member,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{first}/cancel"),
        &json!({}),
    )
    .await;
    assert!(status.is_success(), "{status}: {body}");
    let (status, third) = post(
        &app,
        &member,
        &uri,
        &json!({ "message": "What alerts are there?" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{third}");
    assert_eq!(
        state
            .agents
            .list_runs(PROJECT, 100)
            .await
            .expect("the store")
            .len(),
        3
    );
}

/// AG-45: a conversation is words, no more than the prompt ceiling, and nothing is started for a
/// message that is neither.
#[tokio::test]
async fn a_conversation_is_words_and_no_more_than_the_prompt_ceiling() {
    let (state, app, _, member, _) = world();
    let uri = format!("/api/v1/projects/{PROJECT}/assistant/conversations");

    for message in [
        json!(""),
        json!("   "),
        json!("\n\t "),
        json!("a".repeat(4_001)),
        json!("\u{1F600}".repeat(4_001)),
    ] {
        let (status, body) = post(&app, &member, &uri, &json!({ "message": message })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }

    // A body that is not a conversation request: the extractor's refusal, and no run either.
    for shapeless in [
        json!({}),
        json!({ "message": 7 }),
        json!({ "message": "hello", "asUser": "portal-approver" }),
        json!("hello"),
    ] {
        let (status, body) = post(&app, &member, &uri, &shapeless).await;
        assert!(
            status.is_client_error(),
            "{shapeless} answered {status}: {body}"
        );
    }
    assert!(
        state
            .agents
            .list_runs(PROJECT, 100)
            .await
            .expect("the store")
            .is_empty(),
        "a refused conversation created a run",
    );
}

/// AG-75, AG-77: what a conversation is told about the person's situation is checked as strictly as
/// the message — a form of a kind this platform has, a draft named like a manifest, a field path a
/// form could have, and at most five endpoints, each named once and each one the project holds.
#[tokio::test]
async fn the_situation_a_conversation_is_given_is_one_this_portal_could_have() {
    let (state, app, _, member, _) = world();
    let uri = format!("/api/v1/projects/{PROJECT}/assistant/conversations");
    let ask = |extra: Value| {
        let mut body = json!({ "message": "help me share the bikes" });
        for (key, value) in extra.as_object().expect("an object") {
            body[key] = value.clone();
        }
        body
    };

    for extra in [
        // A form of a kind the platform does not have.
        json!({ "formContext": { "kind": "Spreadsheet" } }),
        json!({ "formContext": { "kind": "endpoint" } }),
        json!({ "formContext": { "kind": "" } }),
        // A draft name that is not a manifest name (MF-02).
        json!({ "formContext": { "kind": "Endpoint", "name": "Helsinki Bikes" } }),
        json!({ "formContext": { "kind": "Endpoint", "name": "../espoo-bikes" } }),
        // A field that is not a path of a form.
        json!({ "formContext": { "kind": "Endpoint", "field": "a".repeat(201) } }),
        json!({ "formContext": { "kind": "Endpoint", "field": "spec.source; drop" } }),
        json!({ "formContext": { "kind": "Endpoint", "field": "spec.source-name" } }),
        json!({ "formContext": { "kind": "Endpoint", "field": "" } }),
        json!({ "formContext": { "kind": "Endpoint", "field": "spec.source[0]/../q" } }),
        // A shape the request does not have.
        json!({ "formContext": "the endpoint form" }),
        json!({ "formContext": { "kind": "Endpoint", "invented": "x" } }),
        // More endpoints than an application reads, the same one twice, and an empty name.
        json!({ "endpointNames": ["a", "b", "c", "d", "e", "f"] }),
        json!({ "endpointNames": ["helsinki-bikes", "helsinki-bikes"] }),
        json!({ "endpointNames": [""] }),
        // An endpoint of another project, and one nobody has.
        json!({ "endpointNames": ["espoo-bikes"] }),
        json!({ "endpointNames": ["helsinki-ghosts"] }),
        // A continuation of a run that is not this project's conversation.
        json!({ "continues": "run-nobody-minted" }),
        json!({ "continues": "" }),
    ] {
        let (status, body) = post(&app, &member, &uri, &ask(extra.clone())).await;
        assert!(
            status.is_client_error(),
            "{extra} answered {status}: {body}",
        );
    }
    assert!(
        state
            .agents
            .list_runs(PROJECT, 100)
            .await
            .expect("the store")
            .is_empty(),
        "a refused conversation created a run",
    );
}

// -------------------------------------------------------------------------------------------------
// T-1989 `get_access`
// -------------------------------------------------------------------------------------------------

/// AG-70, UI-56: the access view is the two halves — what the profile names and what the person may
/// do — and a refusal always carries which half refused. Nothing is open unless both halves are.
#[tokio::test]
async fn the_access_view_names_both_halves_and_never_leaves_a_refusal_unexplained() {
    let (_, app, _, member, stranger) = world();
    let uri = format!("/api/v1/projects/{PROJECT}/assistant/access");

    let (status, mine) = get(&app, &member, &uri).await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    let (status, theirs) = get(&app, &stranger, &uri).await;
    assert_eq!(status, StatusCode::OK, "{theirs}");

    let open = |view: &Value| -> Vec<String> {
        let mut names = Vec::new();
        for profile in view["items"].as_array().into_iter().flatten() {
            for op in profile["operations"].as_array().into_iter().flatten() {
                let profile_half = op["profile"].as_bool().unwrap_or(false);
                let person_half = op["person"].as_bool().unwrap_or(false);
                // The invariant: a reason is absent exactly when neither half refuses.
                assert_eq!(
                    op["reason"].is_null(),
                    profile_half && person_half,
                    "{op} left a refusal unexplained",
                );
                if profile_half && person_half {
                    names.push(op["name"].as_str().unwrap_or_default().to_owned());
                }
            }
        }
        names
    };
    let mine_open = open(&mine);
    let theirs_open = open(&theirs);
    assert!(
        !mine_open.is_empty(),
        "a member of the project may run nothing at all: {mine}",
    );
    for op in &theirs_open {
        assert!(
            mine_open.contains(op),
            "a stranger may run '{op}', which a member of the project may not",
        );
    }
    assert!(
        theirs_open.len() < mine_open.len(),
        "a stranger sees exactly what a member sees: {theirs_open:?}",
    );
}

/// PF-59: the access view answers the profiles of the organization, so it reads the same for a
/// project that exists and one that does not — the door is not a way to ask which projects there are.
/// A name that could not be a project is a 404 before anything is read.
#[tokio::test]
async fn the_access_view_tells_nobody_which_projects_this_installation_has() {
    let (_, app, _, member, _) = world();

    let (status, here) = get(
        &app,
        &member,
        &format!("/api/v1/projects/{PROJECT}/assistant/access"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{here}");
    let (status, nowhere) = get(
        &app,
        &member,
        "/api/v1/projects/no-such-project/assistant/access",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{nowhere}");

    let names = |view: &Value| -> Vec<String> {
        view["items"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|item| item["name"].as_str().unwrap_or_default().to_owned())
            .collect()
    };
    assert_eq!(
        names(&here),
        names(&nowhere),
        "the profiles listed depend on whether the project exists",
    );
    assert!(!names(&here).is_empty(), "no profile was listed: {here}");

    let too_long = "h".repeat(70);
    for name in [
        "Helsinki",
        "helsinki_1",
        "%2e%2e",
        "-helsinki",
        too_long.as_str(),
    ] {
        let (status, body) = get(
            &app,
            &member,
            &format!("/api/v1/projects/{name}/assistant/access"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name}: {body}");
    }
}
