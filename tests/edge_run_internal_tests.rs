//! Edge cases of the three routes the credential proxy reads a run through (T-1980, T-1981, T-1982;
//! AG-52, AG-57, PF-46, PF-59).
//!
//! **The contract, in one sentence:** these routes answer the agent proxy's own ServiceAccount token
//! and nothing else, and what they answer is about one run and its own project — never another
//! project's pipeline, another run's inbox, or a component the door does not know.
//!
//! The internal listener carries no session and no CSRF guard: a NetworkPolicy admits one workload to
//! its port, and the token is the second control (T-1500). `internal_get_run` also hands out the run's
//! ticket **hash**, which is what lets the proxy check a workspace's ticket — so who may call it is the
//! whole of its security, and that is what the first case is about.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::agents::run::{
    digest_prompt, mint_run_id, mint_ticket, AgentRun, AgentRunStatus,
};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

mod common;

const PROJECT: &str = "helsinki";
const ELSEWHERE: &str = "espoo";
const OWNER: &str = "demo.builder";
const SLUG: &str = "si6epqkx364lprho5uaigutk274r5grb";

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
            "JC_PORTAL_AGENT_PROXY_CLIENT_ID" => Some(common::AGENT_PROXY_CLIENT),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("the agent runner block is complete")
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

fn mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    for project in [PROJECT, ELSEWHERE] {
        mirror.upsert(envelope(
            "Endpoint",
            &format!("{project}-bikes"),
            project,
            json!({ "slug": SLUG, "contextSpaceRef": { "kind": "ContextSpace", "name": project },
                    "audience": "project" }),
        ));
        mirror.upsert(envelope(
            "Pipeline",
            &format!("{project}-ingest"),
            project,
            json!({ "class": "resident" }),
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
    mirror
}

fn a_run(id: &str, project: &str, status: AgentRunStatus) -> AgentRun {
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
        kind: "app".to_owned(),
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
        created_by: OWNER.to_owned(),
        origin: "person".to_owned(),
        starter: json!({ "username": OWNER, "groups": ["a-group-nobody-else-reads"] }),
        created_at: "2026-09-19T10:00:00Z".to_owned(),
        started_at: None,
        finished_at: None,
        expires_at: "2099-09-19T10:00:00Z".to_owned(),
        error: None,
    }
}

async fn internal_get(app: &axum::Router, bearer: Option<&str>, uri: &str) -> (StatusCode, Value) {
    let mut request = Request::builder().method("GET").uri(uri);
    if let Some(token) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .clone()
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

fn proxy_bearer() -> &'static str {
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TOKEN.get_or_init(|| common::REALM.workload(common::AGENT_PROXY_CLIENT))
}

async fn listener() -> (AppState, axum::Router) {
    let state = AppState::new(config(), None).with_mirror(mirror());
    state
        .bearer
        .as_ref()
        .expect("a realm")
        .refresh()
        .await
        .expect("jwks");
    (state.clone(), server::internal_app(state))
}

// -------------------------------------------------------------------------------------------------
// Who may call them at all
// -------------------------------------------------------------------------------------------------

/// AG-52, PF-46: the three routes answer the proxy's own ServiceAccount token and nothing else. No
/// token, another workload's token, a token for another audience, a bearer that is not a token, and a
/// session cookie: each is refused, and none of them learns whether the run exists.
#[tokio::test]
async fn the_internal_routes_answer_the_proxys_own_token_and_nothing_else() {
    let (state, app) = listener().await;
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(&id, PROJECT, AgentRunStatus::Building))
        .await
        .expect("a run");

    let uris = [
        format!("/internal/agent-runs/{id}"),
        format!("/internal/agent-runs/{id}/inbox?wait=0"),
        format!("/internal/agent-runs/{id}/diagnostics/pipeline/{PROJECT}-ingest"),
    ];
    let refused = [
        None,
        Some(common::REALM.workload("context-gateway")),
        Some(common::REALM.workload("helsinki-pipelines")),
        Some(common::REALM.token(common::AGENT_PROXY_CLIENT, "portal-api")),
        Some("not-a-token".to_owned()),
        Some(String::new()),
    ];

    for uri in &uris {
        for credential in &refused {
            let (status, body) = internal_get(&app, credential.as_deref(), uri).await;
            assert!(
                status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
                "{uri} answered {status} to {credential:?}: {body}",
            );
            let text = body.to_string();
            for leaked in [
                "ticketHash",
                "$argon2",
                "a-group-nobody-else-reads",
                "city-bikes",
            ] {
                assert!(
                    !text.contains(leaked),
                    "{uri} answered {credential:?} with {leaked:?}: {text}",
                );
            }
        }
        // The proxy's own token gets past the gate. The diagnostics door then answers 503 on this
        // installation, which has no pipeline runner configured (`src/api/pipelines.rs:248`) — past
        // the gate is what this case is about; what it answers is the next one's.
        let (status, _) = internal_get(&app, Some(proxy_bearer()), uri).await;
        assert!(
            status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE,
            "{uri} refused the proxy's own token with {status}",
        );
    }
}

// -------------------------------------------------------------------------------------------------
// T-1980 `internal_get_run`
// -------------------------------------------------------------------------------------------------

/// AG-52: what the proxy is handed is the run's own terms and the profile's limits — including the
/// ticket **hash**, which is what lets it check a workspace's ticket — and never the ticket itself, the
/// person's groups or the prompt. A run nobody minted is 404, whatever the id looks like.
#[tokio::test]
async fn the_run_the_proxy_reads_is_its_terms_and_its_hash_and_never_the_ticket_or_the_person() {
    let (state, app) = listener().await;
    let id = mint_run_id();
    let (ticket, _) = mint_ticket();
    state
        .agents
        .create_run(&a_run(&id, PROJECT, AgentRunStatus::Building))
        .await
        .expect("a run");

    let (status, context) = internal_get(
        &app,
        Some(proxy_bearer()),
        &format!("/internal/agent-runs/{id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{context}");
    assert_eq!(context["id"], json!(id));
    assert_eq!(context["project"], json!(PROJECT));
    // The profile's own limits travel with it: the proxy enforces them, so it has to be told.
    assert_eq!(context["maxTokens"], json!(400_000));
    assert_eq!(context["requestsPerMinute"], json!(60));
    assert_eq!(context["stepsPerRun"], json!(120));
    assert_eq!(context["allowedHosts"], json!(["registry.npmjs.org"]));
    // The hash is there on purpose; the ticket it hashes is not, and neither is the person.
    assert!(
        context["ticketHash"]
            .as_str()
            .is_some_and(|hash| !hash.is_empty()),
        "the proxy cannot check a ticket without the hash: {context}",
    );
    let text = context.to_string();
    assert!(
        !text.contains(&ticket),
        "the answer carried a ticket: {text}"
    );
    for never in ["a-group-nobody-else-reads", "nobody else may read"] {
        assert!(
            !text.contains(never),
            "the answer carried {never:?}: {text}"
        );
    }

    // An id nobody minted, and ids of other shapes: one answer each, and never an error that says
    // more than that.
    for probe in [
        "run-nobody-minted",
        "..",
        "%2e%2e",
        "",
        "a".repeat(300).as_str(),
    ] {
        let (status, body) = internal_get(
            &app,
            Some(proxy_bearer()),
            &format!("/internal/agent-runs/{probe}"),
        )
        .await;
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
            "{probe:?} answered {status}: {body}",
        );
    }
}

// -------------------------------------------------------------------------------------------------
// T-1981 `internal_diagnostics`
// -------------------------------------------------------------------------------------------------

/// AG-57, PF-59: a run reads a diagnostic of **its own project** and of nothing else. The run's project
/// is the only one asked, so another project's pipeline of the same name is not found through this
/// door, and a component the door does not know is refused by name.
#[tokio::test]
async fn a_run_reads_a_diagnostic_of_its_own_project_and_of_no_other() {
    let (state, app) = listener().await;
    let here = mint_run_id();
    let there = mint_run_id();
    state
        .agents
        .create_run(&a_run(&here, PROJECT, AgentRunStatus::Building))
        .await
        .expect("a run");
    state
        .agents
        .create_run(&a_run(&there, ELSEWHERE, AgentRunStatus::Building))
        .await
        .expect("a run");

    // Its own project's pipeline is found — the mirror is asked before the runner
    // (`src/api/pipelines.rs:239-248`), so this installation, which has no runner, answers 503 and
    // not the 404 of a pipeline that is not there.
    let (status, body) = internal_get(
        &app,
        Some(proxy_bearer()),
        &format!("/internal/agent-runs/{here}/diagnostics/pipeline/{PROJECT}-ingest"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "its own project's pipeline: {body}",
    );

    // The other project's pipeline of the same shape, asked for by a run of this project: not found.
    // The run does not learn that it exists — the 404, not the 503 its own project's name gets.
    let (status, body) = internal_get(
        &app,
        Some(proxy_bearer()),
        &format!("/internal/agent-runs/{here}/diagnostics/pipeline/{ELSEWHERE}-ingest"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another project's pipeline: {body}",
    );
    // The answer is about the run's own project — "not found in 'helsinki'" — and says nothing about
    // the project the pipeline really lives in, though the name the caller sent is echoed back.
    let text = body.to_string();
    assert!(text.contains(PROJECT), "{text}");
    for disclosure in ["exists", "another project", "lives in"] {
        assert!(
            !text.contains(disclosure),
            "the answer hinted the pipeline is somewhere: {text}",
        );
    }
    // And the run of that other project reads its own, which is the same 503: the rule is the run's
    // project, not a list of allowed names.
    let (status, body) = internal_get(
        &app,
        Some(proxy_bearer()),
        &format!("/internal/agent-runs/{there}/diagnostics/pipeline/{ELSEWHERE}-ingest"),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");

    // A component the door knows nothing about, and names that try to leave the project.
    for (component, name) in [
        ("pipelines", format!("{PROJECT}-ingest")),
        ("secret", "keycloak-admin".to_owned()),
        ("Pipeline", format!("{PROJECT}-ingest")),
        ("", format!("{PROJECT}-ingest")),
        ("pipeline", "../../espoo/pipelines/espoo-ingest".to_owned()),
        ("pipeline", "%2e%2e%2fespoo-ingest".to_owned()),
    ] {
        let (status, body) = internal_get(
            &app,
            Some(proxy_bearer()),
            &format!("/internal/agent-runs/{here}/diagnostics/{component}/{name}"),
        )
        .await;
        assert_ne!(
            status,
            StatusCode::OK,
            "{component}/{name} was answered: {body}",
        );
        let text = body.to_string();
        for internal in ["panicked", "sqlx", "SELECT", "keycloak-admin-password"] {
            assert!(
                !text.contains(internal),
                "{component}/{name} answered with {internal:?}: {text}",
            );
        }
    }
}

// -------------------------------------------------------------------------------------------------
// T-1982 `internal_inbox`
// -------------------------------------------------------------------------------------------------

/// AG-45, AG-52: the inbox is one run's own. It answers what is after `after`, oldest first, answers at
/// once when the caller asks for no wait, answers at once for a run that is over, and never holds a
/// call longer than the ceiling however long a caller asks for.
#[tokio::test]
async fn the_inbox_is_one_runs_own_and_waits_no_longer_than_its_ceiling() {
    let (state, app) = listener().await;
    let id = mint_run_id();
    let other = mint_run_id();
    state
        .agents
        .create_run(&a_run(&id, PROJECT, AgentRunStatus::Building))
        .await
        .expect("a run");
    state
        .agents
        .create_run(&a_run(&other, PROJECT, AgentRunStatus::Building))
        .await
        .expect("a run");

    // An answer and a message on one run, and a message on the other.
    state
        .agents
        .append_event(
            &id,
            "answer",
            json!({ "questionId": "q1", "answers": { "answer": "yes" } }),
        )
        .await
        .expect("an event");
    state
        .agents
        .append_event(&id, "message", json!({ "text": "make the map bigger" }))
        .await
        .expect("an event");
    state
        .agents
        .append_event(
            &other,
            "message",
            json!({ "text": "not for the first run" }),
        )
        .await
        .expect("an event");

    let (status, inbox) = internal_get(
        &app,
        Some(proxy_bearer()),
        &format!("/internal/agent-runs/{id}/inbox?wait=0"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{inbox}");
    let text = inbox.to_string();
    assert!(text.contains("make the map bigger"), "{inbox}");
    assert!(
        !text.contains("not for the first run"),
        "another run's message reached this inbox: {inbox}",
    );

    // `after` is a position: everything after the second event is nothing yet.
    let (status, empty) = internal_get(
        &app,
        Some(proxy_bearer()),
        &format!("/internal/agent-runs/{id}/inbox?after=99&wait=0"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["items"].as_array().map(Vec::len), Some(0), "{empty}");

    // A wait longer than the ceiling is the ceiling, not the number asked for: the call comes back
    // because this run is over, and the point is that no value a caller sends holds a thread for
    // minutes. A terminal run answers at once whatever the wait says.
    let over = mint_run_id();
    state
        .agents
        .create_run(&a_run(&over, PROJECT, AgentRunStatus::Cancelled))
        .await
        .expect("a run");
    let started = std::time::Instant::now();
    for wait in ["0", "1", "600", "18446744073709551615", "not-a-number", ""] {
        let (status, inbox) = internal_get(
            &app,
            Some(proxy_bearer()),
            &format!("/internal/agent-runs/{over}/inbox?wait={wait}"),
        )
        .await;
        assert!(
            status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
            "wait={wait} answered {status}: {inbox}",
        );
    }
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "a run that is over held the call open",
    );

    // And an inbox of a run nobody minted is not an empty inbox.
    let (status, _) = internal_get(
        &app,
        Some(proxy_bearer()),
        "/internal/agent-runs/run-nobody-minted/inbox?wait=0",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
