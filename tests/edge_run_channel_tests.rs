//! Edge cases of the run's relay, its preview and its MCP door (T-1983, T-1984, T-1985; AG-45, AG-52,
//! AG-70, PF-59, UI-45).
//!
//! **The contract, in one sentence:** the relay and the MCP door are the proxy's own — one
//! ServiceAccount token, one live run, one payload that was checked before it was recorded — and the
//! preview is the run's own person's.
//!
//! These three are where a workspace's output turns into something a browser acts on: an event becomes
//! a line in a log the UI renders and, for `navigate`, a route it follows; an MCP call runs the
//! registry as the person who started the run. So what they refuse matters as much as what they do —
//! a route that is not a path inside the Portal, a status nobody defined, a run that is over.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
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

const PROJECT: &str = "helsinki";
const OWNER: &str = "demo.builder";
const OTHER: &str = "demo.other";
const CSRF: &str = "csrf-token-edge-channel";
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
    mirror.upsert(envelope(
        "Endpoint",
        &format!("{PROJECT}-bikes"),
        PROJECT,
        json!({ "slug": SLUG, "contextSpaceRef": { "kind": "ContextSpace", "name": PROJECT },
                "audience": "project" }),
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

fn a_run(id: &str, status: AgentRunStatus, starter: Value) -> AgentRun {
    let (_, ticket_hash) = mint_ticket();
    AgentRun {
        id: id.to_owned(),
        project: PROJECT.to_owned(),
        app_name: "city-bikes".to_owned(),
        title: None,
        endpoint_name: format!("{PROJECT}-bikes"),
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
        path_prefix: format!("projects/{PROJECT}/apps/city-bikes/"),
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
        starter,
        created_at: "2026-09-19T10:00:00Z".to_owned(),
        started_at: None,
        finished_at: None,
        expires_at: "2099-09-19T10:00:00Z".to_owned(),
        error: None,
    }
}

fn starter_identity() -> Value {
    json!({
        "subject": format!("f:1:{OWNER}"),
        "username": OWNER,
        "email": format!("{OWNER}@hel.fi"),
        "name": Value::Null,
        "roles": [],
        "groups": ["devs"],
    })
}

fn cookie(config: &Config, username: &str) -> String {
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
            groups: vec!["devs".into()],
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

fn proxy_bearer() -> &'static str {
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TOKEN.get_or_init(|| common::REALM.workload(common::AGENT_PROXY_CLIENT))
}

/// The public app and the internal listener over one state, so a run the proxy writes to is the run a
/// person reads.
async fn world() -> (AppState, axum::Router, axum::Router, Config) {
    let config = config();
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    state
        .bearer
        .as_ref()
        .expect("a realm")
        .refresh()
        .await
        .expect("jwks");
    (
        state.clone(),
        server::app(state.clone()),
        server::internal_app(state),
        config,
    )
}

async fn relay(app: &axum::Router, bearer: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/internal/agent-runs/events")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .clone()
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

// -------------------------------------------------------------------------------------------------
// T-1983 `internal_post_event`
// -------------------------------------------------------------------------------------------------

/// AG-52, PF-46: the relay is the proxy's own. Another workload's token, a token for the wrong
/// audience, no token at all and a bearer that is not one are each refused — and none of them learns
/// whether the run exists or puts a line on its log.
#[tokio::test]
async fn an_event_is_relayed_by_the_proxys_own_token_and_by_nothing_else() {
    let (state, _, internal, _) = world().await;
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(&id, AgentRunStatus::Building, starter_identity()))
        .await
        .expect("a run");
    let event = json!({ "runId": id, "kind": "thought", "payload": { "text": "thinking" } });

    for credential in [
        None,
        Some(common::REALM.workload("context-gateway")),
        Some(common::REALM.token(common::AGENT_PROXY_CLIENT, "portal-api")),
        Some("not-a-token".to_owned()),
    ] {
        let (status, body) = relay(&internal, credential.as_deref(), event.clone()).await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
            "{credential:?} answered {status}: {body}",
        );
        assert!(
            !body.to_string().contains("not found"),
            "{credential:?} learned whether the run exists: {body}",
        );
    }
    assert!(
        state
            .agents
            .events_since(&id, 0)
            .await
            .expect("the log")
            .is_empty(),
        "a refused relay wrote to the log",
    );

    // The proxy's own: recorded, and the receipt says where in the log it landed.
    let (status, receipt) = relay(&internal, Some(proxy_bearer()), event).await;
    assert_eq!(status, StatusCode::CREATED, "{receipt}");
    let log = state.agents.events_since(&id, 0).await.expect("the log");
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].kind, "thought");
}

/// AG-45: a relayed event names a live run of this installation. A run nobody minted, a body that is
/// not an event, and a run that is over are each refused, and nothing is recorded anywhere.
#[tokio::test]
async fn a_relayed_event_needs_a_live_run_and_a_body_that_is_an_event() {
    let (state, _, internal, _) = world().await;
    let over = mint_run_id();
    state
        .agents
        .create_run(&a_run(&over, AgentRunStatus::Cancelled, starter_identity()))
        .await
        .expect("a run");

    // A run nobody minted.
    let (status, body) = relay(
        &internal,
        Some(proxy_bearer()),
        json!({ "runId": "run-nobody-minted", "kind": "thought", "payload": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // A body that is not a relayed event: refused as a body, and the message says so rather than
    // naming the shape to whoever reached the port.
    for shapeless in [
        json!({}),
        json!({ "runId": over.clone() }),
        json!({ "kind": "thought", "payload": {} }),
        json!({ "runId": 7, "kind": "thought", "payload": {} }),
        // `deny_unknown_fields`: a field this route does not read is a mistake somewhere, not
        // something to drop silently.
        json!({ "runId": over.clone(), "kind": "thought", "payload": {}, "asUser": "portal-approver" }),
        json!("an event"),
        Value::Null,
    ] {
        let (status, body) = relay(&internal, Some(proxy_bearer()), shapeless.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{shapeless}: {body}");
    }

    // A run that is over takes no more events.
    let (status, body) = relay(
        &internal,
        Some(proxy_bearer()),
        json!({ "runId": over.clone(), "kind": "thought", "payload": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(state
        .agents
        .events_since(&over, 0)
        .await
        .expect("the log")
        .is_empty());
}

/// UI-45: a `navigate` event is a route the browser follows, so it is checked before it is recorded.
/// Anything that is not a path inside the Portal never reaches the log, let alone a browser.
#[tokio::test]
async fn a_navigate_route_that_is_not_a_path_inside_the_portal_never_reaches_the_log() {
    let (state, _, internal, _) = world().await;
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(&id, AgentRunStatus::Building, starter_identity()))
        .await
        .expect("a run");
    let navigate = |payload: Value| json!({ "runId": id, "kind": "navigate", "payload": payload });

    for payload in [
        json!({ "route": "https://evil.example/steal" }),
        json!({ "route": "//evil.example/steal" }),
        json!({ "route": "javascript:alert(1)" }),
        json!({ "route": "/projects/helsinki#/somewhere" }),
        json!({ "route": "/projects/helsinki:8080" }),
        json!({ "route": "/projects/\u{0}helsinki" }),
        json!({ "route": "/projects/helsinki\n" }),
        json!({ "route": format!("/{}", "a".repeat(600)) }),
        json!({ "route": "projects/helsinki" }),
        json!({ "route": "" }),
        json!({ "route": 7 }),
        json!({}),
        // A prefill is an object the form reads; anything else is refused with the route.
        json!({ "route": "/projects/helsinki/endpoints", "prefill": "name=air" }),
        json!({ "route": "/projects/helsinki/endpoints", "prefill": ["air"] }),
        json!({ "route": "/projects/helsinki/endpoints", "prefill": 7 }),
    ] {
        let (status, body) =
            relay(&internal, Some(proxy_bearer()), navigate(payload.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{payload}: {body}");
    }
    assert!(
        state
            .agents
            .events_since(&id, 0)
            .await
            .expect("the log")
            .is_empty(),
        "a refused navigate reached the log",
    );

    // A path inside the Portal, with and without a prefill object: recorded. A backslash is a plain
    // character of a path here and passes: the dock hands the route to the router
    // (`ui/src/assistant/AssistantDock.tsx:178`, `navigate({ href })`), which resolves it inside the
    // application, so `/\evil.example` is a page of this Portal that does not exist and never a
    // location the browser reads as an origin.
    for payload in [
        json!({ "route": "/projects/helsinki/endpoints" }),
        json!({ "route": "/projects/helsinki/endpoints", "prefill": { "name": "air" } }),
        json!({ "route": "/projects/helsinki/endpoints", "prefill": Value::Null }),
        json!({ "route": "/" }),
    ] {
        let (status, body) =
            relay(&internal, Some(proxy_bearer()), navigate(payload.clone())).await;
        assert_eq!(status, StatusCode::CREATED, "{payload}: {body}");
    }
    assert_eq!(
        state
            .agents
            .events_since(&id, 0)
            .await
            .expect("the log")
            .len(),
        4,
    );
}

/// AG-45, AG-62: the three kinds that move something are applied after the event is recorded, so a
/// stream never shows a state the log does not explain — and a `status` event that names no state of a
/// run is refused before either happens.
#[tokio::test]
async fn a_status_event_names_a_state_of_a_run_and_usage_is_counted_on_the_run() {
    let (state, _, internal, _) = world().await;
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(&id, AgentRunStatus::Building, starter_identity()))
        .await
        .expect("a run");

    for status_value in [
        json!("invented"),
        json!("STARTING"),
        json!(""),
        json!(7),
        Value::Null,
    ] {
        let (status, body) = relay(
            &internal,
            Some(proxy_bearer()),
            json!({ "runId": id, "kind": "status", "payload": { "status": status_value } }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{status_value}: {body}");
    }
    assert_eq!(
        state
            .agents
            .get_run(&id)
            .await
            .expect("the store")
            .map(|run| run.status),
        Some(AgentRunStatus::Building.as_str().to_owned()),
        "a status event nobody could read moved the run",
    );

    // Usage is counted on the run, and a step with it.
    let (status, _) = relay(
        &internal,
        Some(proxy_bearer()),
        json!({ "runId": id, "kind": "usage", "payload": { "tokensThisStep": 1200 } }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let run = state
        .agents
        .get_run(&id)
        .await
        .expect("the store")
        .expect("the run");
    assert_eq!(run.tokens_used, 1200);
    assert_eq!(run.steps, 1);

    // A usage event that carries no number counts nothing rather than failing the step.
    let (status, _) = relay(
        &internal,
        Some(proxy_bearer()),
        json!({ "runId": id, "kind": "usage", "payload": { "tokensThisStep": "many" } }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let run = state
        .agents
        .get_run(&id)
        .await
        .expect("the store")
        .expect("the run");
    assert_eq!(run.tokens_used, 1200, "a word was counted as tokens");
    assert_eq!(run.steps, 2);
}

// -------------------------------------------------------------------------------------------------
// T-1984 `preview`
// -------------------------------------------------------------------------------------------------

/// PF-59: the preview is the run's own person's, like everything else about a run. Another person's,
/// another project's and one nobody minted are the same 404, and a run that has written nothing has no
/// preview rather than an empty page.
#[tokio::test]
async fn the_preview_of_a_run_is_read_by_the_person_whose_run_it_is() {
    let (state, app, _, config) = world().await;
    let id = mint_run_id();
    state
        .agents
        .create_run(&a_run(&id, AgentRunStatus::Previewing, starter_identity()))
        .await
        .expect("a run");

    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview");
    let get = |cookie: String, uri: String| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .method("GET")
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
            (status, String::from_utf8_lossy(&bytes).into_owned())
        }
    };

    // Somebody else, who is a member of the project but did not start this run.
    let (status, body) = get(cookie(&config, OTHER), uri.clone()).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        !body.contains("nobody else may read"),
        "the 404 carried the run's prompt: {body}",
    );

    // Its own person: the run has written no files, so there is no preview yet — not an empty page
    // that looks like an application with nothing in it.
    let (status, body) = get(cookie(&config, OWNER), uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(
        body.contains("no preview yet"),
        "the answer does not say what is missing: {body}",
    );

    // A run nobody minted, and a run of a project this path does not name.
    for probe in ["run-nobody-minted", ".."] {
        let (status, _) = get(
            cookie(&config, OWNER),
            format!("/api/v1/projects/{PROJECT}/agent-runs/{probe}/preview"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{probe}");
    }
    // Struck as belonging elsewhere: what a rendered preview carries — its Content Security Policy,
    // the script hash and the origin — is `tests/kit_pass_tests.rs`, which builds a real spec and a
    // real bundle. This case is about who may ask for it.
}

// -------------------------------------------------------------------------------------------------
// T-1985 `internal_mcp`
// -------------------------------------------------------------------------------------------------

/// AG-52, AG-70: the run's MCP door is the proxy's, it runs as the person who started the run, and a
/// run that is over calls nothing. A run whose starter was never recorded has nobody to act as, and is
/// refused rather than falling back to anybody.
#[tokio::test]
async fn the_runs_mcp_door_is_the_proxys_and_acts_as_the_person_who_started_it() {
    let (state, _, internal, _) = world().await;
    let live = mint_run_id();
    let over = mint_run_id();
    let nameless = mint_run_id();
    state
        .agents
        .create_run(&a_run(&live, AgentRunStatus::Building, starter_identity()))
        .await
        .expect("a run");
    state
        .agents
        .create_run(&a_run(&over, AgentRunStatus::Published, starter_identity()))
        .await
        .expect("a run");
    state
        .agents
        .create_run(&a_run(&nameless, AgentRunStatus::Building, Value::Null))
        .await
        .expect("a run");

    let call = |bearer: Option<String>, id: String, body: Value| {
        let internal = internal.clone();
        async move {
            let mut request = Request::builder()
                .method("POST")
                .uri(format!("/internal/agent-runs/{id}/mcp"))
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(token) = bearer {
                request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
            }
            let response = internal
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
            (status, String::from_utf8_lossy(&bytes).into_owned())
        }
    };
    let list_tools = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });

    // Not the proxy: refused, and never told whether the run is there.
    for credential in [
        None,
        Some(common::REALM.workload("context-gateway")),
        Some(common::REALM.token(common::AGENT_PROXY_CLIENT, "portal-api")),
    ] {
        let (status, body) = call(credential.clone(), live.clone(), list_tools.clone()).await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
            "{credential:?} answered {status}: {body}",
        );
    }

    // A run that is over, and one nobody minted.
    let (status, body) = call(Some(proxy_bearer().to_owned()), over, list_tools.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = call(
        Some(proxy_bearer().to_owned()),
        "run-nobody-minted".to_owned(),
        list_tools.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // A run with no starter recorded: there is nobody to run its calls as, and that is a refusal
    // rather than a call that runs as nobody.
    let (status, body) = call(
        Some(proxy_bearer().to_owned()),
        nameless,
        list_tools.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body.contains("starter") || body.contains("nobody"),
        "the refusal does not say what is missing: {body}",
    );

    // The live run: answered, and what it is offered is the registry as its profile and its person
    // allow — a tools list, not an error.
    let (status, body) = call(Some(proxy_bearer().to_owned()), live, list_tools).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let answer: Value = serde_json::from_str(&body).expect("a JSON-RPC answer");
    assert!(
        answer["result"]["tools"].is_array(),
        "the door answered no tools: {answer}",
    );
    // Nothing of the person behind the run rides out on it.
    for never in ["f:1:", "@hel.fi", "devs"] {
        assert!(
            !body.contains(never),
            "the tools list carried {never:?}: {body}",
        );
    }
}
