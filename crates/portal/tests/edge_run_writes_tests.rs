//! Edge cases of what a person writes to a run (T-1973 – T-1979; AG-83, AP-46, PF-50, PF-59, UI-73).
//!
//! **The contract, in one sentence:** every one of these routes is the run's own door — the person who
//! started it or somebody who may approve in that project, never anybody else — a run that is over
//! takes nothing more, and what is written is inside the bounds the route publishes.
//!
//! They all write to the same log, which the workspace reads as instructions (`internal_inbox`), so a
//! value that gets past a bound reaches the agent: an answer nobody offered, a message longer than the
//! model is given, a second observation of one version that starts a second check. The seven routes
//! share `own_run_to_write` and `terminal`, so the first two cases sweep all of them and the rest take each
//! route's own bounds.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::agents::run::{
    digest_prompt, mint_run_id, mint_ticket, AgentRun, AgentRunStatus,
};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

mod common;

const CSRF: &str = "csrf-token-edge-writes";
const PROJECT: &str = "helsinki";
const ELSEWHERE: &str = "espoo";
const OWNER: &str = "demo.builder";
const OTHER: &str = "demo.other";
const SLUG: &str = "si6epqkx364lprho5uaigutk274r5grb";
/// The bounds these routes publish (`src/api/agent_runs.rs`).
const MAX_MESSAGE_CHARS: usize = 4_000;
const MAX_PREVIEW_ERROR_CHARS: usize = 2_000;
const MAX_PREVIEW_FILE_CHARS: usize = 300;

fn config() -> Config {
    Config::from_vars(|key| {
        match key {
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some("http://jc-agent-proxy.agents.svc.cluster.local:8080"),
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("the agent runner block is complete")
}

fn cookie(config: &Config, username: &str, roles: &[&str], groups: &[&str]) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{username}"),
            username: username.into(),
            email: Some(format!("{username}@hel.fi")),
            name: None,
            roles: roles.iter().map(|role| role.to_string()).collect(),
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

/// A project whose members may propose an App, plus the endpoints and the builder profile a run names.
fn mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    for project in [PROJECT, ELSEWHERE] {
        mirror.upsert(envelope(
            "Endpoint",
            &format!("{project}-bikes"),
            project,
            json!({
                "slug": SLUG,
                "contextSpaceRef": { "kind": "ContextSpace", "name": project },
                "audience": "project"
            }),
        ));
    }
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
    mirror.upsert(envelope(
        "Role",
        "developer",
        "org",
        json!({ "rules": [{ "kinds": ["App", "Endpoint"], "verbs": ["read", "propose"] }] }),
    ));
    for (name, project) in [("dev-here", PROJECT), ("dev-there", ELSEWHERE)] {
        mirror.upsert(envelope(
            "RoleBinding",
            name,
            "org",
            json!({ "subjects": [{ "group": "devs" }], "role": "developer",
                    "scope": { "project": project } }),
        ));
    }
    mirror
}

fn a_run(id: &str, project: &str, who: &str, status: AgentRunStatus, kind: &str) -> AgentRun {
    let (_, ticket_hash) = mint_ticket();
    AgentRun {
        id: id.to_owned(),
        project: project.to_owned(),
        app_name: "city-bikes".to_owned(),
        title: None,
        endpoint_name: format!("{project}-bikes"),
        endpoint_slug: SLUG.to_owned(),
        endpoints: json!([]),
        profile: "app-builder".to_owned(),
        kind: kind.to_owned(),
        unattended: false,
        continues: None,
        app_class: "ui-rust".to_owned(),
        visibility: "project".to_owned(),
        prompt: "a prompt nobody else may read".to_owned(),
        prompt_digest: digest_prompt("a prompt"),
        data_needs: json!([]),
        allows_write: false,
        branch: format!("agent/app-city-bikes/{id}"),
        path_prefix: format!("projects/{project}/apps/city-bikes/"),
        status: status.as_str().to_owned(),
        ticket_hash,
        workspace: None,
        merge_request: None,
        change_id: None,
        source_url: None,
        mirror_url: None,
        preview_url: None,
        first_frame_ms: None,
        first_version_ms: None,
        files: json!({}),
        steps: 0,
        tokens_used: 0,
        created_by: who.to_owned(),
        origin: "person".to_owned(),
        starter: json!({ "username": who }),
        created_at: "2026-09-19T10:00:00Z".to_owned(),
        started_at: None,
        finished_at: None,
        expires_at: "2099-09-19T10:00:00Z".to_owned(),
        error: None,
    }
}

async fn post(
    app: &axum::Router,
    cookie: Option<&str>,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(Method::POST).uri(uri);
    if let Some(cookie) = cookie {
        request = request
            .header(header::COOKIE, cookie)
            .header("x-csrf-token", CSRF);
    }
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
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

/// The seven routes, each with a body that would be accepted on a live run of one's own.
fn every_write(id: &str) -> Vec<(&'static str, String, Option<Value>)> {
    vec![
        (
            "answers",
            format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/answers"),
            Some(json!({ "questionId": "q1", "answers": { "answer": "yes" } })),
        ),
        (
            "messages",
            format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
            Some(json!({ "text": "make the map bigger" })),
        ),
        (
            "preview-errors",
            format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview-errors"),
            Some(json!({ "message": "TypeError: x is not a function" })),
        ),
        (
            "preview-observations",
            format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview-observations"),
            Some(
                json!({ "version": 1, "pages": [{ "label": "Home", "text": "12 stations", "rows": [] }] }),
            ),
        ),
        (
            "functions",
            format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/functions/stations"),
            Some(json!({})),
        ),
        (
            "cancel",
            format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/cancel"),
            None,
        ),
        (
            "publish",
            format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/publish"),
            None,
        ),
    ]
}

fn state_and_app() -> (AppState, axum::Router, Config) {
    let config = config();
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    (state.clone(), server::app(state), config)
}

// -------------------------------------------------------------------------------------------------
// The door every one of them shares
// -------------------------------------------------------------------------------------------------

/// PF-59, R20: every write route is the run's own door. Another person's run, a run of another
/// project, an id nobody minted and a path that is not an id all answer 404 — one answer, so no id can
/// be probed — and nothing of the run's own text comes back.
#[tokio::test]
async fn not_one_write_route_writes_to_a_run_that_is_not_this_callers() {
    let (state, app, config) = state_and_app();
    let theirs = mint_run_id();
    let far = mint_run_id();
    state
        .agents
        .create_run(&a_run(
            &theirs,
            PROJECT,
            OWNER,
            AgentRunStatus::Previewing,
            "app",
        ))
        .await
        .expect("a run");
    state
        .agents
        .create_run(&a_run(
            &far,
            ELSEWHERE,
            OTHER,
            AgentRunStatus::Previewing,
            "app",
        ))
        .await
        .expect("a run");
    // A member of both projects who started neither run: a reader of runs, not their owner.
    let outsider = cookie(&config, OTHER, &[], &["devs"]);

    for probe in [
        theirs.as_str(),
        far.as_str(),
        "run-nobody-minted",
        "..",
        "%2e%2e",
    ] {
        for (route, uri, body) in every_write(probe) {
            let (status, answer) = post(&app, Some(&outsider), &uri, body).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "{route} on {probe} answered {status}: {answer}",
            );
            assert!(
                !answer.to_string().contains("nobody else may read"),
                "{route} on {probe} answered with the run's prompt: {answer}",
            );
        }
    }

    // And nothing was written to either run's log by any of it.
    for id in [theirs, far] {
        assert!(
            state
                .agents
                .events_since(&id, 0)
                .await
                .expect("the log")
                .is_empty(),
            "a refused write left an event on the log",
        );
    }
}

/// PF-50, R20: a write is a write, so it carries the double-submit token. Without it none of these
/// routes is reached at all, whatever session the request carries.
#[tokio::test]
async fn not_one_write_route_is_reachable_without_the_csrf_token() {
    let (state, app, config) = state_and_app();
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(
            &id,
            PROJECT,
            OWNER,
            AgentRunStatus::Previewing,
            "app",
        ))
        .await
        .expect("a run");
    let owner = cookie(&config, OWNER, &[], &["devs"]);
    // The session cookie without the header: a cross-site form's shape.
    let without_header = owner.clone();

    for (route, uri, body) in every_write(&id) {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri(&uri)
            .header(header::COOKIE, &without_header);
        let body = match body {
            Some(json) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(json.to_string())
            }
            None => Body::empty(),
        };
        let response = app
            .clone()
            .oneshot(request.body(body).expect("a request"))
            .await
            .expect("a response");
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{route} took a write with no CSRF token",
        );
    }
    assert!(
        state
            .agents
            .events_since(&id, 0)
            .await
            .expect("the log")
            .is_empty(),
        "a write with no token reached the log",
    );
}

/// AP-46: a run that is over takes nothing more. Every route that writes to a live run answers 409 on
/// a cancelled one, and the sentence says what the run is.
#[tokio::test]
async fn a_run_that_is_over_takes_no_more_writing() {
    let (state, app, config) = state_and_app();
    let owner = cookie(&config, OWNER, &[], &["devs"]);

    for over in [
        AgentRunStatus::Cancelled,
        AgentRunStatus::Failed,
        AgentRunStatus::Published,
        AgentRunStatus::Expired,
    ] {
        let id = mint_run_id();
        state
            .agents
            .create_run(&a_run(&id, PROJECT, OWNER, over, "app"))
            .await
            .expect("a run");
        for (route, uri, body) in every_write(&id) {
            // Cancelling a run that is already over, and publishing one, have their own answers:
            // those two are the next case.
            if route == "cancel" || route == "publish" {
                continue;
            }
            let (status, answer) = post(&app, Some(&owner), &uri, body).await;
            assert_eq!(
                status,
                StatusCode::CONFLICT,
                "{route} wrote to a {} run: {answer}",
                over.as_str(),
            );
            assert!(
                answer.to_string().contains(over.as_str()),
                "{route} refused without saying what the run is: {answer}",
            );
        }
    }
}

// -------------------------------------------------------------------------------------------------
// T-1973 `answer_question`
// -------------------------------------------------------------------------------------------------

/// AG-83, UI-73: an answer is one of what the question offered. A value nobody offered, the same
/// option twice, a count outside `min`/`max` and a list where one answer was asked for are each
/// refused, and the question stays open — the log gets no `answer` event for any of them.
#[tokio::test]
async fn an_answer_is_one_of_what_the_question_offered_and_nothing_else() {
    let (state, app, config) = state_and_app();
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(&id, PROJECT, OWNER, AgentRunStatus::Building, "app"))
        .await
        .expect("a run");
    let owner = cookie(&config, OWNER, &[], &["devs"]);
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/answers");

    // A question the Portal filled: pick one of three.
    state
        .agents
        .append_event(
            &id,
            "question",
            json!({
                "questionId": "which-space",
                "pick": "space",
                "options": [
                    { "value": "air-quality" },
                    { "value": "mobility" },
                    { "value": "waste" }
                ],
            }),
        )
        .await
        .expect("a question");
    // And one that takes two or three of four.
    state
        .agents
        .append_event(
            &id,
            "question",
            json!({
                "questionId": "which-pages",
                "multiple": true,
                "min": 2,
                "max": 3,
                "options": [
                    { "value": "home" },
                    { "value": "map" },
                    { "value": "table" },
                    { "value": "about" }
                ],
            }),
        )
        .await
        .expect("a question");
    let questions = state
        .agents
        .events_since(&id, 0)
        .await
        .expect("the log")
        .len();

    for (question, answer) in [
        // A value nobody offered, in every shape a browser could send it.
        ("which-space", json!({ "answer": "everything" })),
        ("which-space", json!({ "answer": "Air-Quality" })),
        ("which-space", json!({ "answer": "air-quality " })),
        ("which-space", json!({ "answer": "" })),
        ("which-space", json!({ "answer": null })),
        ("which-space", json!({ "answer": 1 })),
        ("which-space", json!({ "answer": ["air-quality"] })),
        ("which-space", json!({})),
        // The list question: one answer, too few, too many, a repeat, a stranger among them.
        ("which-pages", json!({ "answer": "home" })),
        ("which-pages", json!({ "answer": ["home"] })),
        (
            "which-pages",
            json!({ "answer": ["home", "map", "table", "about"] }),
        ),
        ("which-pages", json!({ "answer": ["home", "home"] })),
        ("which-pages", json!({ "answer": ["home", "everything"] })),
        ("which-pages", json!({ "answer": [] })),
        ("which-pages", json!({ "answer": [1, 2] })),
    ] {
        let (status, problem) = post(
            &app,
            Some(&owner),
            &uri,
            Some(json!({ "questionId": question, "answers": answer.clone() })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{question} took {answer}: {problem}",
        );
    }

    // Nothing above was written: the questions are still the whole log.
    assert_eq!(
        state
            .agents
            .events_since(&id, 0)
            .await
            .expect("the log")
            .len(),
        questions,
        "a refused answer reached the run's log",
    );

    // What the question offered is taken, once each.
    for (question, answer) in [
        ("which-space", json!({ "answer": "mobility" })),
        ("which-pages", json!({ "answer": ["home", "map"] })),
    ] {
        let (status, problem) = post(
            &app,
            Some(&owner),
            &uri,
            Some(json!({ "questionId": question, "answers": answer.clone() })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{question}: {problem}");
    }
    let log = state.agents.events_since(&id, 0).await.expect("the log");
    assert_eq!(log.iter().filter(|event| event.kind == "answer").count(), 2);
    // The log says who answered, because a run's decisions are somebody's (AG-80).
    for event in log.iter().filter(|event| event.kind == "answer") {
        assert_eq!(event.payload["answeredBy"], json!(OWNER));
    }
}

// -------------------------------------------------------------------------------------------------
// T-1974 `post_message`, T-1975 `post_preview_error`, T-1976 `post_preview_observation`
// -------------------------------------------------------------------------------------------------

/// AG-45: a message is text a person typed, inside the bounds the route publishes, and the endpoints of
/// a conversation are a conversation's own business.
#[tokio::test]
async fn a_message_is_text_within_its_bounds_and_endpoints_belong_to_a_conversation() {
    let (state, app, config) = state_and_app();
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(&id, PROJECT, OWNER, AgentRunStatus::Building, "app"))
        .await
        .expect("a run");
    let owner = cookie(&config, OWNER, &[], &["devs"]);
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages");

    for body in [
        json!({ "text": "" }),
        json!({ "text": "   \n\t " }),
        json!({ "text": "x".repeat(MAX_MESSAGE_CHARS + 1) }),
        json!({ "text": 7 }),
        json!({}),
        // This run is an app, not a conversation: its endpoints are not changed by a message.
        json!({ "text": "hello", "endpointNames": [format!("{PROJECT}-bikes")] }),
        json!({ "text": "hello", "endpointNames": [] }),
    ] {
        let (status, problem) = post(&app, Some(&owner), &uri, Some(body.clone())).await;
        assert!(
            status.is_client_error(),
            "{body} answered {status}: {problem}",
        );
    }
    assert!(
        state
            .agents
            .events_since(&id, 0)
            .await
            .expect("the log")
            .is_empty(),
        "a refused message reached the log",
    );

    // The text at the ceiling is taken, trimmed, and says who sent it.
    let (status, problem) = post(
        &app,
        Some(&owner),
        &uri,
        Some(json!({ "text": format!("  {}  ", "x".repeat(MAX_MESSAGE_CHARS)) })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{problem}");
    let log = state.agents.events_since(&id, 0).await.expect("the log");
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].kind, "message");
    assert_eq!(log[0].payload["sentBy"], json!(OWNER));
    assert_eq!(
        log[0].payload["text"].as_str().map(str::len),
        Some(MAX_MESSAGE_CHARS),
        "the message was not trimmed to what was typed",
    );
}

/// SDK-14, SDK-20: a preview error is a message, a file and a line, each within its bound, and a line
/// starts at 1. SDK-27, SDK-28: an observation is of one version, with pages inside their bounds, and
/// the first observation of a version is the one that counts.
#[tokio::test]
async fn a_preview_error_and_an_observation_stay_inside_the_bounds_the_route_publishes() {
    let (state, app, config) = state_and_app();
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(
            &id,
            PROJECT,
            OWNER,
            AgentRunStatus::Previewing,
            "app",
        ))
        .await
        .expect("a run");
    let owner = cookie(&config, OWNER, &[], &["devs"]);
    let errors = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview-errors");
    let observations = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview-observations");

    for body in [
        json!({ "message": "" }),
        json!({ "message": "   " }),
        json!({ "message": "x".repeat(MAX_PREVIEW_ERROR_CHARS + 1) }),
        json!({ "message": "boom", "file": "x".repeat(MAX_PREVIEW_FILE_CHARS + 1) }),
        json!({ "message": "boom", "line": 0 }),
        json!({}),
    ] {
        // A value outside a bound is the route's own 400; a body missing a required field is the
        // 422 the JSON extractor answers before the handler runs. Both are the caller's to fix.
        let (status, problem) = post(&app, Some(&owner), &errors, Some(body.clone())).await;
        assert!(
            status.is_client_error(),
            "{body} answered {status}: {problem}"
        );
    }

    for body in [
        json!({ "version": 0, "pages": [{ "label": "Home", "text": "ok", "rows": [] }] }),
        json!({ "version": 1, "pages": [] }),
        json!({ "version": 1 }),
        json!({ "pages": [{ "label": "Home", "text": "ok", "rows": [] }] }),
        json!({ "version": -1, "pages": [{ "label": "Home", "text": "ok", "rows": [] }] }),
    ] {
        let (status, problem) = post(&app, Some(&owner), &observations, Some(body.clone())).await;
        assert!(
            status.is_client_error(),
            "{body} answered {status}: {problem}",
        );
    }
    assert!(
        state
            .agents
            .events_since(&id, 0)
            .await
            .expect("the log")
            .is_empty(),
        "a refused report reached the log",
    );

    // One good error and one good observation, and then the same version again: refused, so a reload
    // of the preview does not start a second check of the same files.
    let (status, problem) = post(
        &app,
        Some(&owner),
        &errors,
        Some(json!({ "message": "TypeError: x is not a function", "file": "src/App.tsx", "line": 12 })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{problem}");

    let observation = json!({
        "version": 1,
        "pages": [{ "label": "Home", "text": "12 stations", "rows": [] }]
    });
    let (status, problem) =
        post(&app, Some(&owner), &observations, Some(observation.clone())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{problem}");
    let (status, problem) = post(&app, Some(&owner), &observations, Some(observation)).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a version was observed twice: {problem}"
    );

    let log = state.agents.events_since(&id, 0).await.expect("the log");
    assert_eq!(log.iter().filter(|e| e.kind == "preview_error").count(), 1);
    assert_eq!(
        log.iter()
            .filter(|e| e.kind == "preview_observation")
            .count(),
        1
    );
    // Both say who reported them: a run's log is a record of what people and agents did (AG-80).
    for event in &log {
        assert_eq!(event.payload["reportedBy"], json!(OWNER), "{event:?}");
    }
}

// -------------------------------------------------------------------------------------------------
// T-1977 `call_function`, T-1978 `cancel_run`, T-1979 `publish_run`
// -------------------------------------------------------------------------------------------------

/// SDK-20: a function of a run is one the run has. A name it does not have is 404, a body that is not
/// JSON is 400, and the answer never carries the inside of the Portal.
#[tokio::test]
async fn a_function_call_is_a_function_the_run_has_and_a_body_that_is_json() {
    let (state, app, config) = state_and_app();
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(
            &id,
            PROJECT,
            OWNER,
            AgentRunStatus::Previewing,
            "app",
        ))
        .await
        .expect("a run");
    let owner = cookie(&config, OWNER, &[], &["devs"]);

    // This run wrote no functions at all, so every name is one it does not have.
    for name in [
        "stations",
        "../../etc/passwd",
        "STATIONS",
        "a".repeat(200).as_str(),
    ] {
        let (status, problem) = post(
            &app,
            Some(&owner),
            &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/functions/{name}"),
            Some(json!({})),
        )
        .await;
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
            "{name} answered {status}: {problem}",
        );
        let text = problem.to_string();
        for internal in ["panicked", "src/", "sqlx", "jc-functions.svc"] {
            assert!(
                !text.contains(internal),
                "{name} answered with {internal:?}: {text}"
            );
        }
    }

    // A body that is not JSON is refused as a body, not as a function.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/projects/{PROJECT}/agent-runs/{id}/functions/stations"
                ))
                .header(header::COOKIE, &owner)
                .header("x-csrf-token", CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{not json"))
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// AP-46, PF-50: cancelling is a write to the project, so it needs the App proposal grant as well as
/// the run; and a cancelled run cannot be cancelled into something else.
#[tokio::test]
async fn cancelling_needs_the_grant_that_started_the_run() {
    let (state, app, config) = state_and_app();
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(&id, PROJECT, OWNER, AgentRunStatus::Building, "app"))
        .await
        .expect("a run");
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/cancel");

    // The person who started it, after their grant was taken away: the run is theirs to read and not
    // theirs to stop, and the refusal says which verb is missing rather than what the run holds.
    let ungranted = cookie(&config, OWNER, &[], &[]);
    let (status, problem) = post(&app, Some(&ungranted), &uri, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    assert!(
        !problem.to_string().contains("nobody else may read"),
        "the refusal carried the run's prompt: {problem}",
    );
    assert_eq!(
        state
            .agents
            .get_run(&id)
            .await
            .expect("the store")
            .map(|run| run.status),
        Some(AgentRunStatus::Building.as_str().to_owned()),
        "a refused cancel changed the run",
    );

    // With the grant: cancelled, and the ticket hash is left as nothing a workspace can match.
    let owner = cookie(&config, OWNER, &[], &["devs"]);
    let (status, cancelled) = post(&app, Some(&owner), &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(
        cancelled["status"],
        json!(AgentRunStatus::Cancelled.as_str())
    );
    let stored = state
        .agents
        .get_run(&id)
        .await
        .expect("the store")
        .expect("the run");
    assert!(
        stored.ticket_hash.is_empty(),
        "a cancelled run kept a ticket hash something could still match",
    );
}

/// AP-46, AP-55: only a run that has a preview is published, a dashboard and an analysis are not
/// published here at all, and the refusal says what the run is.
#[tokio::test]
async fn only_a_run_with_a_preview_is_published_and_never_a_dashboard_or_an_analysis() {
    let (state, app, config) = state_and_app();
    let owner = cookie(&config, OWNER, &[], &["devs"]);

    for (kind, status_now, expected_word) in [
        ("dashboard", AgentRunStatus::Previewing, "dashboard"),
        ("analysis", AgentRunStatus::Previewing, "analysis"),
        ("app", AgentRunStatus::Queued, "queued"),
        ("app", AgentRunStatus::Building, "building"),
        ("app", AgentRunStatus::Testing, "testing"),
    ] {
        let id = mint_run_id();
        state
            .agents
            .create_run(&a_run(&id, PROJECT, OWNER, status_now, kind))
            .await
            .expect("a run");
        let (status, problem) = post(
            &app,
            Some(&owner),
            &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/publish"),
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "{kind} in {} was published: {problem}",
            status_now.as_str(),
        );
        assert!(
            problem.to_string().contains(expected_word),
            "the refusal of a {kind} in {} says nothing a person can act on: {problem}",
            status_now.as_str(),
        );
        assert_eq!(
            state
                .agents
                .get_run(&id)
                .await
                .expect("the store")
                .map(|run| run.status),
            Some(status_now.as_str().to_owned()),
            "a refused publish moved the run",
        );
    }
}

// -------------------------------------------------------------------------------------------------
// A binding, not authorship, lets a person write to a run (T-2486)
// -------------------------------------------------------------------------------------------------

/// PF-50, T-2486: the person who started a run and then lost their binding in the project (left the
/// department, the RoleBinding removed) writes nothing more to it. Every write route answers `403`,
/// and no event reaches the log the workspace reads as instructions.
#[tokio::test]
async fn a_run_whose_owner_lost_the_binding_takes_no_write() {
    let (state, app, config) = state_and_app();
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(
            &id,
            PROJECT,
            OWNER,
            AgentRunStatus::Previewing,
            "app",
        ))
        .await
        .expect("a run");
    let binding = envelope("RoleBinding", "dev-here", "org", json!({}));
    assert!(
        state.mirror.remove(&binding.key()).is_some(),
        "the fixture binds devs in {PROJECT}",
    );
    // The same session as before: still in the group, which no binding names any more.
    let owner = cookie(&config, OWNER, &[], &["devs"]);

    for (route, uri, body) in every_write(&id) {
        let (status, answer) = post(&app, Some(&owner), &uri, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{route} answered {status}: {answer}"
        );
        assert!(
            !answer.to_string().contains("nobody else may read"),
            "{route} answered with the run's prompt: {answer}",
        );
    }
    assert!(
        state
            .agents
            .events_since(&id, 0)
            .await
            .expect("the log")
            .is_empty(),
        "a refused write left an event on the log",
    );
    assert_eq!(
        state
            .agents
            .get_run(&id)
            .await
            .expect("the store")
            .map(|run| run.status),
        Some(AgentRunStatus::Previewing.as_str().to_owned()),
        "a refused write changed the run",
    );
}

/// AG-64, PF-50: a stranger to the project hears the same answer from a run's REST route as from
/// the operation of the same action, and the answer does not depend on whether the id exists.
#[tokio::test]
async fn the_rest_routes_and_the_ops_door_refuse_a_stranger_alike() {
    let (state, app, config) = state_and_app();
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(
            &id,
            PROJECT,
            OWNER,
            AgentRunStatus::Previewing,
            "app",
        ))
        .await
        .expect("a run");
    let stranger = cookie(&config, "eva", &[], &["another-city"]);

    for probe in [id.as_str(), "run-nobody-minted"] {
        for (route, op, input) in [
            (
                "messages",
                "jc_run_message",
                json!({ "id": probe, "text": "make the map bigger" }),
            ),
            (
                "answers",
                "jc_run_answer",
                json!({ "id": probe, "questionId": "q1", "answers": { "answer": "yes" } }),
            ),
            ("cancel", "jc_run_cancel", json!({ "id": probe })),
            ("publish", "jc_run_publish", json!({ "id": probe })),
        ] {
            let rest = every_write(probe)
                .into_iter()
                .find(|(name, _, _)| *name == route)
                .expect("a write route");
            let (rest_status, rest_answer) = post(&app, Some(&stranger), &rest.1, rest.2).await;
            let (door_status, door_answer) = post(
                &app,
                Some(&stranger),
                &format!("/api/v1/projects/{PROJECT}/ops/{op}"),
                Some(input),
            )
            .await;
            assert_eq!(
                rest_status,
                StatusCode::FORBIDDEN,
                "{route} on {probe}: {rest_answer}"
            );
            assert_eq!(
                rest_status, door_status,
                "{route} on {probe}: {rest_answer} vs {door_answer}",
            );
        }
    }
    assert!(
        state
            .agents
            .events_since(&id, 0)
            .await
            .expect("the log")
            .is_empty(),
        "a refused write left an event on the log",
    );
}
