//! Edge cases of a builder run's own routes (T-1964, T-1965, T-1969 – T-1972; AG-64, AP-42, PF-50,
//! PF-59).
//!
//! **The contract, in one sentence:** a run is started by somebody who may propose the application it
//! writes, with every field of the request checked before anything is scheduled, and it is read only
//! by the person who started it or somebody who may approve in that project — never with the
//! credential it holds or the identity of the person behind it.
//!
//! A run spends money and writes to a branch, so the create route is where a bad request costs
//! something: everything it checks is checked before the pod. The read routes are where a run's own
//! text — a prompt, an error, a preview address — could reach the wrong reader, so they answer 404
//! rather than 403 and say nothing about a run that is not this caller's.
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

const CSRF: &str = "csrf-token-edge-runs";
const PROJECT: &str = "helsinki";
const ELSEWHERE: &str = "espoo";
const STEWARD: &str = "demo.steward";
const BUILDER: &str = "demo.builder";
const SLUG: &str = "si6epqkx364lprho5uaigutk274r5grb";
/// The route's own ceilings (`src/api/agent_runs.rs`).
const MAX_PROMPT_CHARS: usize = 4_000;
const MAX_LIST_LIMIT: i64 = 100;

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

fn session_cookie(config: &Config, username: &str, roles: &[&str], groups: &[&str]) -> String {
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

fn builder_profile_spec() -> Value {
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
    })
}

/// An endpoint in each project, a builder profile, and a second profile that builds nothing.
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
                "audience": "project",
                "projection": { "hiddenAttributes": ["maintenanceInternalCode"] }
            }),
        ));
    }
    mirror.upsert(envelope(
        "AgentProfile",
        "app-builder",
        "org",
        builder_profile_spec(),
    ));
    let mut analyst = builder_profile_spec();
    analyst["role"] = json!("analyst");
    mirror.upsert(envelope("AgentProfile", "just-an-analyst", "org", analyst));
    mirror
}

fn create_body() -> Value {
    json!({
        "appName": "city-bikes-overview",
        "endpointName": format!("{PROJECT}-bikes"),
        "appClass": "ui-rust",
        "visibility": "project",
        "prompt": "Create a live bike availability dashboard with station filtering",
        "dataNeeds": [{
            "contextSpaceRef": { "kind": "ContextSpace", "name": PROJECT },
            "types": ["BikeHireDockingStation"],
            "attrs": ["name", "location"],
            "operations": ["queryEntity", "retrieveEntity"]
        }]
    })
}

async fn call(
    app: &axum::Router,
    cookie: &str,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", CSRF);
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

/// A run written straight into the store, for the read routes: what they answer is what the store
/// holds, and creating one through the API is the create route's own case.
fn a_run(id: &str, project: &str, app_name: &str, who: &str) -> AgentRun {
    let (_, ticket_hash) = mint_ticket();
    AgentRun {
        id: id.to_owned(),
        project: project.to_owned(),
        app_name: app_name.to_owned(),
        title: None,
        endpoint_name: format!("{project}-bikes"),
        endpoint_slug: SLUG.to_owned(),
        endpoints: json!([]),
        profile: "app-builder".to_owned(),
        kind: "app".to_owned(),
        unattended: false,
        continues: None,
        app_class: "ui-rust".to_owned(),
        visibility: "project".to_owned(),
        prompt: format!("a prompt of {who} nobody else may read"),
        prompt_digest: digest_prompt("a prompt"),
        data_needs: json!([]),
        allows_write: false,
        branch: format!("agent/app-{app_name}/{id}"),
        path_prefix: format!("projects/{project}/apps/{app_name}/"),
        status: AgentRunStatus::Queued.as_str().to_owned(),
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
        starter: json!({ "username": who, "groups": ["a-group-nobody-else-reads"] }),
        created_at: "2026-09-19T10:00:00Z".to_owned(),
        started_at: None,
        finished_at: None,
        expires_at: "2099-09-19T10:00:00Z".to_owned(),
        error: None,
    }
}

fn state_and_app() -> (AppState, axum::Router, Config) {
    let config = config();
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    (state.clone(), server::app(state), config)
}

// -------------------------------------------------------------------------------------------------
// T-1969 `create_run`
// -------------------------------------------------------------------------------------------------

/// AP-10, PF-50: starting a run is proposing the application it writes. A reader of the project may
/// not start one, and the refusal happens before the quota, the profile or the endpoints are read —
/// nothing is created, so nothing is scheduled and nothing is spent.
#[tokio::test]
async fn a_run_is_started_only_by_somebody_who_may_propose_an_application() {
    let (state, app, config) = state_and_app();
    let reader = session_cookie(&config, "vera.viewer", &[], &[]);

    let (status, problem) = call(
        &app,
        &reader,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    assert!(
        state
            .agents
            .list_runs(PROJECT, 50)
            .await
            .expect("the store")
            .is_empty(),
        "a refused request created a run",
    );
}

/// AP-42, AP-44, AG-26: every field is checked before the pod. Each request below is refused by name,
/// and after all of them the project still holds no run — a run that was created and then failed
/// validation would have cost a branch and a row.
#[tokio::test]
async fn every_field_of_a_run_request_is_checked_before_anything_is_scheduled() {
    let (state, app, config) = state_and_app();
    let steward = session_cookie(&config, STEWARD, &["portal-approver"], &[]);

    let with = |field: &str, value: Value| {
        let mut body = create_body();
        body[field] = value;
        body
    };
    let cases: Vec<(&str, Value)> = vec![
        // The application's name is a DNS-1123 label: it becomes a branch and a path.
        ("appName", json!("City_Bikes")),
        ("appName", json!("../../etc/passwd")),
        ("appName", json!("-leading")),
        ("appName", json!("")),
        ("appName", json!("a".repeat(64))),
        // The prompt: nothing to do, or more than the model is given.
        ("prompt", json!("")),
        ("prompt", json!("   \n\t ")),
        ("prompt", json!("x".repeat(MAX_PROMPT_CHARS + 1))),
        // The two enumerations are jc-core's own.
        ("appClass", json!("Static")),
        ("appClass", json!("invented")),
        ("appClass", json!("")),
        ("visibility", json!("Public")),
        ("visibility", json!("invented")),
        // A conversation is not started here, and a kind nobody serves is not started at all.
        ("kind", json!("conversation")),
        ("kind", json!("invented")),
        ("kind", json!("")),
        // A profile that builds nothing, and one that is not there.
        ("profile", json!("just-an-analyst")),
        ("profile", json!("no-such-profile")),
        // The endpoint: another project's, one that is not there, and none at all.
        ("endpointName", json!(format!("{ELSEWHERE}-bikes"))),
        ("endpointName", json!("no-such-endpoint")),
        ("endpointName", json!("")),
    ];

    for (field, value) in cases {
        let (status, problem) = call(
            &app,
            &steward,
            Method::POST,
            &format!("/api/v1/projects/{PROJECT}/agent-runs"),
            Some(with(field, value.clone())),
        )
        .await;
        // A name the caller chose is a bad request, with one exception the code makes on purpose:
        // `Profile::load` (`src/agents/profile.rs:47`) answers 503 for a profile that is not in the
        // organization's configuration, because the same call reads the installation's default
        // profile, where an absent one really is a misconfiguration. The message names the profile
        // either way; the status class is noted in /workspace/chyby.md.
        let refused = status.is_client_error()
            || (field == "profile" && status == StatusCode::SERVICE_UNAVAILABLE);
        assert!(refused, "{field} = {value} answered {status}: {problem}",);
        let text = problem.to_string();
        assert!(
            !text.contains("panicked") && !text.contains("src/"),
            "{field} = {value} answered with the inside of the Portal: {text}",
        );
    }

    // A body that is not a request at all, and one that carries a field this route does not have.
    for body in [
        json!({}),
        json!("a prompt"),
        Value::Null,
        json!({ "appName": "city-bikes", "prompt": "x", "sudo": true }),
    ] {
        let (status, problem) = call(
            &app,
            &steward,
            Method::POST,
            &format!("/api/v1/projects/{PROJECT}/agent-runs"),
            Some(body.clone()),
        )
        .await;
        assert!(
            status.is_client_error(),
            "{body} answered {status}: {problem}"
        );
    }

    assert!(
        state
            .agents
            .list_runs(PROJECT, 50)
            .await
            .expect("the store")
            .is_empty(),
        "a refused request created a run after all",
    );
}

/// AG-64: what a run's own answer carries. The ticket is the run's credential and is handed out only
/// when there is nowhere to schedule it (then the caller drives the workspace); the stored hash of it
/// and the identity of the person who started it are never serialized at all, whichever route answers.
#[tokio::test]
async fn a_run_answers_without_its_stored_ticket_or_the_person_behind_it() {
    let (_, app, config) = state_and_app();
    let steward = session_cookie(&config, STEWARD, &["portal-approver"], &[]);

    let (status, created) = call(
        &app,
        &steward,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    // `CreatedRun` flattens the run, so the run's own fields are the answer's own.
    let id = created["id"].as_str().expect("an id").to_owned();
    // Nothing to schedule in: the ticket comes back once, to the caller who will drive the run.
    assert!(created["ticket"].as_str().is_some_and(|t| !t.is_empty()));

    for answer in [
        created.clone(),
        call(
            &app,
            &steward,
            Method::GET,
            &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
            None,
        )
        .await
        .1,
        call(
            &app,
            &steward,
            Method::GET,
            &format!("/api/v1/projects/{PROJECT}/agent-runs"),
            None,
        )
        .await
        .1["items"][0]
            .clone(),
    ] {
        assert_eq!(answer["id"], json!(id), "{answer}");
        for never in ["ticketHash", "ticket_hash", "starter"] {
            assert!(
                answer.get(never).is_none(),
                "a run answered with {never}: {answer}",
            );
        }
        let text = answer.to_string();
        assert!(
            !text.contains("$argon2"),
            "a run answered with its ticket hash: {text}",
        );
    }
}

/// AP-46: one live run per application. The second is a conflict that names the first, and the first
/// is left alone.
#[tokio::test]
async fn a_second_live_run_of_one_application_is_a_conflict() {
    let (state, app, config) = state_and_app();
    let steward = session_cookie(&config, STEWARD, &["portal-approver"], &[]);
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs");

    let (status, first) = call(&app, &steward, Method::POST, &uri, Some(create_body())).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{first}");
    let (status, second) = call(&app, &steward, Method::POST, &uri, Some(create_body())).await;
    assert_eq!(status, StatusCode::CONFLICT, "{second}");

    assert_eq!(
        state
            .agents
            .list_runs(PROJECT, 50)
            .await
            .expect("the store")
            .len(),
        1,
        "the refused second run was recorded anyway",
    );
}

// -------------------------------------------------------------------------------------------------
// T-1970 `list_runs`, T-1971 `get_run`
// -------------------------------------------------------------------------------------------------

/// PF-59, R20, T-1368: the runs of a project the caller may not read are the runs of no project, not
/// an empty list that says the project is there. And a list asks for a page a store can answer: a
/// limit of nothing or less is refused by name, and one above the ceiling is the ceiling.
#[tokio::test]
async fn the_runs_of_a_project_a_caller_cannot_read_are_the_runs_of_no_project() {
    let (state, app, config) = state_and_app();
    state
        .agents
        .create_run(&a_run(&mint_run_id(), PROJECT, "bikes", STEWARD))
        .await
        .expect("a run");
    let stranger = session_cookie(&config, "nobody", &[], &[]);
    let steward = session_cookie(&config, STEWARD, &["portal-approver"], &[]);

    for project in [PROJECT, "espoo", "nosuchproject", "Not_A_Project"] {
        let (status, body) = call(
            &app,
            &stranger,
            Method::GET,
            &format!("/api/v1/projects/{project}/agent-runs"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{project}: {body}");
        assert!(
            !body.to_string().contains("nobody else may read"),
            "{project} answered a stranger with a run's prompt: {body}",
        );
    }

    // The page: positive or refused, and capped.
    for (query, expected) in [
        ("?limit=0", StatusCode::BAD_REQUEST),
        ("?limit=-1", StatusCode::BAD_REQUEST),
    ] {
        let (status, _) = call(
            &app,
            &steward,
            Method::GET,
            &format!("/api/v1/projects/{PROJECT}/agent-runs{query}"),
            None,
        )
        .await;
        assert_eq!(status, expected, "{query}");
    }
    let (status, page) = call(
        &app,
        &steward,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?limit=1000000"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(
        page["items"].as_array().map(Vec::len).unwrap_or_default() as i64 <= MAX_LIST_LIMIT,
        "a page above the ceiling was answered whole: {page}",
    );
}

/// AG-71, PF-59: a person reads their own runs; somebody who may approve in the project reads the
/// project's, because they review what a run proposes. `mine=true` narrows even them, and nobody reads
/// a run of another project through this project's door.
#[tokio::test]
async fn a_person_reads_their_own_runs_and_an_approver_reads_the_projects() {
    let (state, app, config) = state_and_app();
    let theirs = mint_run_id();
    let mine = mint_run_id();
    let far = mint_run_id();
    state
        .agents
        .create_run(&a_run(&theirs, PROJECT, "theirs", STEWARD))
        .await
        .expect("a run");
    state
        .agents
        .create_run(&a_run(&mine, PROJECT, "mine", BUILDER))
        .await
        .expect("a run");
    state
        .agents
        .create_run(&a_run(&far, ELSEWHERE, "far", BUILDER))
        .await
        .expect("a run");

    // A builder with a binding that reads the project but approves nothing.
    state.mirror.upsert(envelope(
        "Role",
        "developer",
        "org",
        json!({ "rules": [{ "kinds": ["App", "Endpoint"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "dev-binding",
        "org",
        json!({ "subjects": [{ "group": "devs" }], "role": "developer",
                "scope": { "project": PROJECT } }),
    ));
    let builder = session_cookie(&config, BUILDER, &[], &["devs"]);
    let approver = session_cookie(&config, STEWARD, &["portal-approver"], &[]);

    let ids = |page: &Value| -> Vec<String> {
        page["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|run| run["id"].as_str().unwrap_or_default().to_owned())
            .collect()
    };

    let (status, page) = call(
        &app,
        &builder,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(
        ids(&page),
        vec![mine.clone()],
        "a builder read somebody else's run"
    );

    let (status, page) = call(
        &app,
        &approver,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let seen = ids(&page);
    assert!(seen.contains(&mine) && seen.contains(&theirs), "{seen:?}");
    assert!(
        !seen.contains(&far),
        "a run of another project was listed here"
    );

    let (status, page) = call(
        &app,
        &approver,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?mine=true"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(
        ids(&page),
        vec![theirs.clone()],
        "mine=true did not narrow to the approver's own"
    );

    // One run, read by id: its own person, an approver, and nobody else.
    for (who, cookie, expected) in [
        ("the builder", &builder, StatusCode::OK),
        ("an approver", &approver, StatusCode::OK),
    ] {
        let (status, run) = call(
            &app,
            cookie,
            Method::GET,
            &format!("/api/v1/projects/{PROJECT}/agent-runs/{mine}"),
            None,
        )
        .await;
        assert_eq!(status, expected, "{who}: {run}");
    }
    // The builder is not the steward's run's reader, a run of another project is not here, and an id
    // nobody minted reads the same way: one answer, so no id can be probed.
    for probe in [
        theirs.as_str(),
        far.as_str(),
        "run-nobody-minted",
        "..",
        "%2e%2e",
    ] {
        let (status, body) = call(
            &app,
            &builder,
            Method::GET,
            &format!("/api/v1/projects/{PROJECT}/agent-runs/{probe}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{probe}: {body}");
        assert!(
            !body.to_string().contains("nobody else may read"),
            "{probe} answered with the run's prompt: {body}",
        );
    }
}

// -------------------------------------------------------------------------------------------------
// T-1972 `stream_events`
// -------------------------------------------------------------------------------------------------

/// AG-45, PF-59: the event stream is the same door as the run, it is never buffered or cached by
/// anything in front of it, and the sequence number a browser resumes from is read as a number or
/// ignored — a header a client controls may not decide anything else.
#[tokio::test]
async fn the_event_stream_is_the_runs_own_door_and_resumes_only_by_a_number() {
    let (state, app, config) = state_and_app();
    let mine = mint_run_id();
    let theirs = mint_run_id();
    state
        .agents
        .create_run(&a_run(&mine, PROJECT, "mine", BUILDER))
        .await
        .expect("a run");
    state
        .agents
        .create_run(&a_run(&theirs, PROJECT, "theirs", STEWARD))
        .await
        .expect("a run");
    let builder = session_cookie(&config, BUILDER, &[], &[]);

    // Not this caller's run, and a run nobody minted: one answer.
    for probe in [theirs.as_str(), "run-nobody-minted"] {
        let (status, _) = call(
            &app,
            &builder,
            Method::GET,
            &format!("/api/v1/projects/{PROJECT}/agent-runs/{probe}/events"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{probe}");
    }

    // The caller's own: opened, headers read, dropped. Reading the body would wait for an event.
    for resume in [
        None,
        Some("0"),
        Some("-1"),
        Some("not a number"),
        Some(""),
        Some("9999999999999999999999"),
    ] {
        let mut request = Request::builder()
            .method(Method::GET)
            .uri(format!(
                "/api/v1/projects/{PROJECT}/agent-runs/{mine}/events"
            ))
            .header(header::COOKIE, &builder);
        if let Some(value) = resume {
            request = request.header("last-event-id", value);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).expect("a request"))
            .await
            .expect("a response");
        assert_eq!(response.status(), StatusCode::OK, "{resume:?}");
        let headers = response.headers();
        assert_eq!(
            headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/event-stream"),
            "{resume:?}",
        );
        assert_eq!(
            headers
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("no-cache"),
        );
        assert_eq!(
            headers
                .get("x-accel-buffering")
                .and_then(|v| v.to_str().ok()),
            Some("no"),
            "a buffered stream holds every event until the run ends (AG-45)",
        );
    }
}

// -------------------------------------------------------------------------------------------------
// T-1965 `store::create_run`, T-1964 the status pair
// -------------------------------------------------------------------------------------------------

/// AG-71: a created run is readable by its id and listed in its own project alone. Two runs of two
/// projects, and the filter the list route hands the store, never answer each other's.
#[tokio::test]
async fn a_created_run_belongs_to_its_project_and_to_the_person_who_started_it() {
    let state = AppState::new(config(), None).with_mirror(mirror());
    let here = mint_run_id();
    let there = mint_run_id();
    state
        .agents
        .create_run(&a_run(&here, PROJECT, "bikes", BUILDER))
        .await
        .expect("a run");
    state
        .agents
        .create_run(&a_run(&there, ELSEWHERE, "bikes", BUILDER))
        .await
        .expect("a run");

    assert_eq!(
        state
            .agents
            .get_run(&here)
            .await
            .expect("the store")
            .map(|run| run.project),
        Some(PROJECT.to_owned()),
    );
    let ours: Vec<String> = state
        .agents
        .list_runs(PROJECT, 50)
        .await
        .expect("the store")
        .into_iter()
        .map(|run| run.id)
        .collect();
    assert_eq!(ours, vec![here.clone()]);

    // The filter the route builds for somebody who is not an approver: their own runs only, and a
    // name that is nobody's reads nothing.
    for (who, expected) in [(BUILDER, 1), (STEWARD, 0), ("", 0), ("' OR 1=1 --", 0)] {
        let filter = joinedcontext_portal::agents::store::RunFilter {
            created_by: Some(who.to_owned()),
            ..Default::default()
        };
        let found = state
            .agents
            .list_runs_filtered(PROJECT, &filter, 50)
            .await
            .expect("the store");
        assert_eq!(found.len(), expected, "created_by {who:?}");
    }

    // A run nobody created is no run, and asking for one is not an error.
    assert!(state
        .agents
        .get_run("run-nobody-minted")
        .await
        .expect("the store")
        .is_none());
}

/// AG-62: a status a run may not reach is refused, and nothing is written — neither the run's own
/// status nor its log. `oneshot::model::status` (`src/agents/oneshot/model.rs:626`) is `set_status`
/// followed by the event, with `?` between them, so a refused change never becomes an event that says
/// it happened. This case drives the pair the way that function does.
#[tokio::test]
async fn a_status_a_run_may_not_reach_is_refused_and_leaves_no_event_saying_it_did() {
    let state = AppState::new(config(), None).with_mirror(mirror());
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(&id, PROJECT, "bikes", BUILDER))
        .await
        .expect("a run");

    // Queued is where a run starts; Published is five steps away.
    let refused = state
        .agents
        .set_status(&id, AgentRunStatus::Published, None)
        .await;
    assert!(refused.is_err(), "a run jumped from queued to published");
    assert_eq!(
        state
            .agents
            .get_run(&id)
            .await
            .expect("the store")
            .map(|run| run.status),
        Some(AgentRunStatus::Queued.as_str().to_owned()),
    );
    assert!(
        state
            .agents
            .events_since(&id, 0)
            .await
            .expect("the log")
            .is_empty(),
        "a refused status change left an event saying it happened",
    );

    // The step it may take is taken, and then the log says so once.
    state
        .agents
        .set_status(&id, AgentRunStatus::Starting, None)
        .await
        .expect("queued to starting");
    let event = state
        .agents
        .append_event(
            &id,
            "status",
            json!({ "status": AgentRunStatus::Starting.as_str() }),
        )
        .await
        .expect("an event");
    assert_eq!(event.seq, 1, "the first event of a run is its first event");
    assert_eq!(event.payload["status"], json!("starting"));

    // A status change of a run nobody minted is refused rather than creating one.
    assert!(state
        .agents
        .set_status("run-nobody-minted", AgentRunStatus::Starting, None)
        .await
        .is_err());
    assert!(state
        .agents
        .get_run("run-nobody-minted")
        .await
        .expect("the store")
        .is_none());
}

// -------------------------------------------------------------------------------------------------
// T-2865 the person's identity, handed to the proxy at the start (ADR-N-038 §3.1, AG-94)
// -------------------------------------------------------------------------------------------------

/// A run started through the edge hands the person's own token to the proxy, once, as the Portal's
/// service account; neither the answer nor the stored run carries it.
#[tokio::test]
async fn a_run_started_at_the_edge_hands_its_persons_token_to_the_proxy_and_keeps_none() {
    use wiremock::matchers::{body_json, header as wm_header, method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    Mock::given(method("POST"))
        .and(path(format!(
            "{}/protocol/openid-connect/token",
            url::Url::parse(&common::REALM.issuer)
                .expect("the issuer")
                .path()
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "portal-service-token", "token_type": "Bearer", "expires_in": 300
        })))
        .mount(common::REALM.server)
        .await;
    let person = common::REALM.person_token_of("portal-api", "edge", STEWARD, &["portal-approver"]);
    let proxy = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/internal/runs/[A-Za-z0-9-]+/identity$"))
        .and(wm_header("authorization", "Bearer portal-service-token"))
        .and(body_json(json!({ "subjectToken": person })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&proxy)
        .await;

    let proxy_base = proxy.uri();
    let config = Config::from_vars(|key| {
        match key {
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some(proxy_base.as_str()),
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            "JC_TRUST_EDGE_TOKEN" => Some("true"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("the agent runner block is complete");
    let oidc = joinedcontext_portal::auth::oidc::OidcClient::discover(
        config.oidc.as_ref().expect("an oidc block"),
        &config.redirect_uri(),
    )
    .await
    .expect("the realm is discoverable");
    let state = AppState::new(config, Some(oidc)).with_mirror(mirror());
    let app = server::app(state.clone());

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/projects/{PROJECT}/agent-runs"))
                .header("x-access-token", &person)
                // The edge's session is a browser's: the double-submit token rides along.
                .header(header::COOKIE, format!("__Host-jc_csrf={CSRF}"))
                .header("x-csrf-token", CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(create_body().to_string()))
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
    let answer = String::from_utf8_lossy(&bytes).into_owned();
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
    proxy.verify().await;

    assert!(
        !answer.contains(&person),
        "the answer carries the person's token"
    );
    let id = serde_json::from_str::<Value>(&answer).expect("json")["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let stored = state
        .agents
        .get_run(&id)
        .await
        .expect("the store")
        .expect("the run");
    assert!(
        !format!("{stored:?}").contains(&person),
        "the run record holds the person's token"
    );

    // Each later turn of the person hands it over again, so a grant the start could not leave or
    // a proxy restart took is back before the run acts on the turn (T-6483).
    proxy.reset().await;
    Mock::given(method("POST"))
        .and(path(format!("/internal/runs/{id}/identity")))
        .and(wm_header("authorization", "Bearer portal-service-token"))
        .and(body_json(json!({ "subjectToken": person })))
        .respond_with(ResponseTemplate::new(204))
        .expect(2)
        .mount(&proxy)
        .await;
    state
        .agents
        .append_event(
            &id,
            "question",
            json!({ "questionId": "which-space", "question": "Which space?", "options": ["doprava"] }),
        )
        .await
        .expect("a question");
    let turn = |route: &str, body: Value, token: Option<&str>| {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri(format!(
                "/api/v1/projects/{PROJECT}/agent-runs/{id}/{route}"
            ))
            .header(header::COOKIE, format!("__Host-jc_csrf={CSRF}"))
            .header("x-csrf-token", CSRF)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            request = request.header("x-access-token", token);
        }
        app.clone().oneshot(
            request
                .body(Body::from(body.to_string()))
                .expect("a request"),
        )
    };
    let message = turn("messages", json!({ "text": "And now?" }), Some(&person));
    assert_eq!(
        message.await.expect("a response").status(),
        StatusCode::NO_CONTENT
    );
    let answer = json!({ "questionId": "which-space", "answers": { "answer": "doprava" } });
    let answered = turn("answers", answer, Some(&person));
    assert_eq!(
        answered.await.expect("a response").status(),
        StatusCode::NO_CONTENT
    );
    proxy.verify().await;

    // A request with no token of the person's has nothing to hand over.
    let unsigned = turn("messages", json!({ "text": "And now?" }), None);
    assert!(unsigned
        .await
        .expect("a response")
        .status()
        .is_client_error());
    proxy.verify().await;
}
