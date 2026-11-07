//! Builder runs through the real router (T-0538, AG-43…AG-46, AG-52, AP-42, AP-44, AP-46).
//!
//! Every case drives the routes a person or the credential proxy actually calls. Four
//! properties are the reason the file exists: a public application is refused before anything
//! is scheduled, a run may not ask for more than its endpoint publishes, a cancelled run's
//! ticket stops working, and the two internal routes are reachable with the proxy's token and
//! with nothing else.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::agents::run::{
    digest_prompt, mint_run_id, mint_ticket, AgentRun, AgentRunStatus,
};
use joinedcontext_portal::agents::{preview, reaper};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;

mod common;

const CSRF: &str = "csrf-token-value";
const PROJECT: &str = "helsinki";
const STEWARD: &str = "demo.steward";
const SLUG: &str = "si6epqkx364lprho5uaigutk274r5grb";

/// A Portal with an agent runner configured and no cluster to schedule in: the shape every case
/// here runs against, because the workspace is not what these routes are about.
fn config() -> Config {
    Config::from_vars(|key| {
        match key {
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some("http://jc-agent-proxy.agents.svc.cluster.local:8080"),
            // The callbacks take the proxy's own ServiceAccount token (T-2271), so this Portal
            // knows the realm that signs it and the client it belongs to.
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            "JC_PORTAL_AGENT_PROXY_CLIENT_ID" => Some(common::AGENT_PROXY_CLIENT),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            // The live journeys sign in as the steward (AG-93, T-2816).
            "JC_PORTAL_JOURNEY_USERS" => Some("demo.other, demo.steward"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("the agent runner block is complete")
}

/// The token `jc-agent-proxy` presents on the internal listener: minted by the process's realm for
/// the client this Portal is told about, and the only credential those routes answer.
fn proxy_bearer() -> &'static str {
    static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TOKEN.get_or_init(|| common::REALM.workload(common::AGENT_PROXY_CLIENT))
}

fn session_cookie(config: &Config, username: &str, roles: &[&str]) -> String {
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
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &session).expect("store session");
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
        "egress": { "allowedHosts": ["registry.npmjs.org", "static.crates.io"] },
        "tools": ["shell", "npm", "git"],
        "workspace": { "cpu": "1", "memory": "2Gi", "ephemeralStorage": "4Gi" }
    })
}

/// The endpoint the runs read through: two attributes published, one hidden.
fn mirror(profile: Option<Value>) -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(envelope(
        "Endpoint",
        "helsinki-bikes",
        PROJECT,
        json!({
            "slug": SLUG,
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
            "audience": "project",
            "projection": { "hiddenAttributes": ["maintenanceInternalCode"] }
        }),
    ));
    if let Some(spec) = profile {
        mirror.upsert(envelope("AgentProfile", "app-builder", "org", spec));
    }
    mirror
}

fn router(mirror: Arc<Mirror>, config: &Config) -> axum::Router {
    server::app(AppState::new(config.clone(), None).with_mirror(mirror))
}

fn internal_router(mirror: Arc<Mirror>, config: &Config) -> axum::Router {
    server::internal_app(AppState::new(config.clone(), None).with_mirror(mirror))
}

fn with_state(mirror: Arc<Mirror>, config: &Config) -> (AppState, axum::Router, axum::Router) {
    let state = AppState::new(config.clone(), None).with_mirror(mirror);
    (
        state.clone(),
        server::app(state.clone()),
        server::internal_app(state),
    )
}

/// A Portal, its router and its internal router over one shared state, so a run created through
/// the public API is the run the proxy reads.
fn both(mirror: Arc<Mirror>, config: &Config) -> (axum::Router, axum::Router) {
    let (_, app, internal) = with_state(mirror, config);
    (app, internal)
}

fn create_body() -> Value {
    json!({
        "appName": "city-bikes-overview",
        "endpointName": "helsinki-bikes",
        // The workspace shape: a `static` run is the kit pass and never hands out its ticket
        // (tests/kit_pass_tests.rs).
        "appClass": "ui-rust",
        "visibility": "project",
        "prompt": "Create a live bike availability dashboard with station filtering",
        "dataNeeds": [{
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
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

async fn internal_call(
    app: &axum::Router,
    bearer: Option<&str>,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
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

/// Reads the first `frames` frames of an event stream. The stream never ends on its own, so a
/// test that collected the whole body would wait for the keep-alive for ever.
async fn read_stream(
    app: &axum::Router,
    cookie: &str,
    uri: &str,
    last: Option<i64>,
    frames: usize,
) -> String {
    let mut request = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .header(header::ACCEPT, "text/event-stream");
    if let Some(seq) = last {
        request = request.header("last-event-id", seq.to_string());
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).expect("a request"))
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    assert_eq!(
        response
            .headers()
            .get("x-accel-buffering")
            .and_then(|value| value.to_str().ok()),
        Some("no"),
        "the edge would hold every event until the run ended (AG-45)"
    );
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-cache"),
        "the API's own no-store must not replace the stream's header"
    );

    let mut body = response.into_body();
    let mut text = String::new();
    for _ in 0..frames {
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("a frame arrives")
            .expect("the stream is open")
            .expect("the frame is data");
        if let Some(chunk) = frame.data_ref() {
            text.push_str(&String::from_utf8_lossy(chunk));
        }
    }
    text
}

async fn create_run(app: &axum::Router, cookie: &str) -> Value {
    let (status, body) = call(
        app,
        cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    body
}

#[tokio::test]
async fn a_run_is_created_queued_and_readable() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let created = create_run(&app, &cookie).await;
    assert_eq!(created["status"], json!("queued"));
    assert_eq!(created["project"], json!(PROJECT));
    assert_eq!(created["endpointSlug"], json!(SLUG));
    assert_eq!(created["allowsWrite"], json!(false));
    assert_eq!(
        created["pathPrefix"],
        json!("projects/helsinki/apps/city-bikes-overview/")
    );
    assert!(
        created["branch"]
            .as_str()
            .is_some_and(|branch| branch.starts_with("agent/app-city-bikes-overview/")),
        "the run owns one branch of the organization repository"
    );
    assert!(
        created["promptDigest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:")),
        "a reviewer has to be able to see the prompt has not been edited"
    );
    assert!(
        created.get("ticketHash").is_none(),
        "AG-46: the ticket hash is never in a public answer"
    );
    assert!(
        created["ticket"].as_str().is_some(),
        "with no cluster the caller drives the workspace, so it needs the ticket"
    );

    let id = created["id"].as_str().expect("an id");
    let (status, run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(run["id"], json!(id));

    let (status, list) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().map(Vec::len), Some(1));

    let (status, _) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/bb/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another project's run is not found, not forbidden"
    );
}

#[tokio::test]
async fn a_second_person_neither_reads_nor_steers_a_run_that_is_not_theirs() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let mine = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &mine).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    // A signed-in person with no role in the project: the run keeps the creator's grants, so its
    // conversation, its preview and its inbox are none of their business (AG-43, AG-45, PF-50).
    let intruder = session_cookie(&config, "intruder", &[]);
    for (method, path, body) in [
        (Method::GET, format!("/agent-runs/{id}"), None),
        (Method::GET, format!("/agent-runs/{id}/events"), None),
        (Method::GET, format!("/agent-runs/{id}/preview"), None),
        (
            Method::POST,
            format!("/agent-runs/{id}/messages"),
            Some(json!({ "text": "delete everything" })),
        ),
        (Method::POST, format!("/agent-runs/{id}/cancel"), None),
    ] {
        let (status, _) = call(
            &app,
            &intruder,
            method.clone(),
            &format!("/api/v1/projects/{PROJECT}{path}"),
            body,
        )
        .await;
        // A read is `404` (PF-59); a write is `403` before the id is looked up, as the ops door
        // answers it, so neither tells the intruder whether the run exists (PF-50, T-2486).
        let refused = if method == Method::GET {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::FORBIDDEN
        };
        assert_eq!(
            status, refused,
            "{method} {path} answered a person who did not start the run"
        );
    }

    // Its own person still reads it, and so does whoever may approve in the project.
    let (status, run) = call(
        &app,
        &mine,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{run}");
    let approver = session_cookie(&config, "demo.approver", &["portal-approver"]);
    let (status, run) = call(
        &app,
        &approver,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{run}");
}

#[tokio::test]
async fn a_public_application_is_refused_before_anything_is_scheduled() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["visibility"] = json!("public");
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("AP-42")),
        "the refusal names the requirement: {problem}"
    );

    let (status, list) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        list["items"].as_array().map(Vec::len),
        Some(0),
        "a refused request leaves no run behind"
    );
}

#[tokio::test]
async fn every_data_need_the_endpoint_hides_is_named_at_once() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["dataNeeds"][0]["attrs"] = json!(["name", "maintenanceInternalCode"]);
    body["dataNeeds"]
        .as_array_mut()
        .expect("needs")
        .push(json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
            "types": [],
            "attrs": ["maintenanceInternalCode"],
            "operations": ["queryEntity"]
        }));

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let errors = problem["errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        errors.len(),
        3,
        "AP-44: both hidden attributes and the empty type list, in one answer: {problem}"
    );
}

/// The indicators of the same city, a second endpoint an application may read beside the first.
const KPI_SLUG: &str = "q3mzkq2v7w5ayxcbn4ltdj6hofkpis";

fn mirror_with_kpis(profile: Option<Value>) -> Arc<Mirror> {
    let mirror = mirror(profile);
    mirror.upsert(envelope(
        "Endpoint",
        "helsinki-kpi",
        PROJECT,
        json!({
            "slug": KPI_SLUG,
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki-kpi" },
            "audience": "project",
            "projection": { "hiddenAttributes": ["formulaInternal"] }
        }),
    ));
    mirror
}

fn two_endpoint_body() -> Value {
    let mut body = create_body();
    body.as_object_mut()
        .expect("an object")
        .remove("endpointName");
    body["endpointNames"] = json!(["helsinki-bikes", "helsinki-kpi"]);
    body["dataNeeds"]
        .as_array_mut()
        .expect("needs")
        .push(json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki-kpi" },
            "types": ["KeyPerformanceIndicator"],
            "attrs": ["name", "kpiValue"],
            "operations": ["queryEntity"]
        }));
    body
}

/// AP-44, SDK-02, SDK-18: an application of the bikes and their indicators. Both endpoints are on
/// the run, the proxy may read both, and the preview's SDK configuration lists both.
#[tokio::test]
async fn an_application_reads_several_endpoints_each_need_on_its_own_space() {
    let config = config();
    let (state, app, internal) =
        with_state(mirror_with_kpis(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(two_endpoint_body()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let run = &created;
    let run = if run.is_null() { &created } else { run };
    assert_eq!(run["endpointName"], json!("helsinki-bikes"));
    assert_eq!(run["endpointSlug"], json!(SLUG));
    assert_eq!(
        run["endpoints"],
        json!([
            { "name": "helsinki-bikes", "slug": SLUG, "space": "helsinki" },
            { "name": "helsinki-kpi", "slug": KPI_SLUG, "space": "helsinki-kpi" }
        ])
    );
    let id = run["id"].as_str().expect("an id").to_owned();

    let (status, context) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::GET,
        &format!("/internal/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{context}");
    assert_eq!(context["endpointSlug"], json!(SLUG));
    assert_eq!(context["endpointSlugs"], json!([SLUG, KPI_SLUG]));
    // The proxy counts one model call per step against this ceiling (AG-25, AG-51, T-0841).
    assert_eq!(context["stepsPerRun"], json!(120));
    assert_eq!(context["requestsPerMinute"], json!(60));
    // The profile names hosts and no budget, so the run gets the default rather than nothing:
    // the fetch route counts every byte against this number (AG-65, T-0557).
    assert_eq!(
        context["allowedHosts"],
        json!(["registry.npmjs.org", "static.crates.io"])
    );
    assert_eq!(
        context["maxEgressBytesPerRun"],
        json!(jc_core::DEFAULT_EGRESS_BYTES_PER_RUN)
    );

    let mut files = preview::template_files();
    files.retain(|path, _| path.starts_with("src/") || path.starts_with("functions/"));
    state.agents.set_files(&id, json!(files)).await.unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{PROJECT}/agent-runs/{id}/preview"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes);
    if status == StatusCode::OK {
        assert!(body.contains(&format!("\"slug\":\"{KPI_SLUG}\"")), "{body}");
        assert!(
            body.contains("\"types\":[\"KeyPerformanceIndicator\"]"),
            "each endpoint with the types its needs read there"
        );
    } else {
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    }
}

/// Operations beside the public stations, both on the space `helsinki`: the operations endpoint
/// shows the maintenance code and hides where a station stands.
const OPS_SLUG: &str = "p8vx2kq7w5ayxcbn4ltdj6hofkops";

fn mirror_with_operations() -> Arc<Mirror> {
    let mirror = mirror(Some(builder_profile_spec()));
    mirror.upsert(envelope(
        "Endpoint",
        "helsinki-bikes-ops",
        PROJECT,
        json!({
            "slug": OPS_SLUG,
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
            "audience": "project",
            "projection": { "hiddenAttributes": ["location", "maintenanceInternalCode"] }
        }),
    ));
    mirror
}

/// AP-44, SDK-02: two endpoints of one space each publish their part of the same stations. An
/// attribute one of them publishes is readable, one both hide is refused naming both, and the
/// preview's configuration lists the type under each, so the application names the endpoint and
/// joins the rows by id.
#[tokio::test]
async fn two_endpoints_of_one_space_each_serve_the_type_and_bound_only_what_both_hide() {
    let config = config();
    let (state, app, _internal) = with_state(mirror_with_operations(), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs");

    let mut body = create_body();
    body.as_object_mut()
        .expect("an object")
        .remove("endpointName");
    body["endpointNames"] = json!(["helsinki-bikes-ops", "helsinki-bikes"]);

    let mut hidden_by_both = body.clone();
    hidden_by_both["dataNeeds"][0]["attrs"] = json!(["maintenanceInternalCode"]);
    let (status, problem) = call(&app, &cookie, Method::POST, &uri, Some(hidden_by_both)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert!(
        problem["errors"][0]["message"].as_str().is_some_and(
            |e| e.contains("hidden by endpoint 'helsinki-bikes-ops' and 'helsinki-bikes'")
        ),
        "{problem}"
    );

    // `location` is hidden by operations alone: the public endpoint publishes it.
    let (status, created) = call(&app, &cookie, Method::POST, &uri, Some(body)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let run = if created["run"].is_null() {
        &created
    } else {
        &created["run"]
    };
    let id = run["id"].as_str().expect("an id").to_owned();

    let mut files = preview::template_files();
    files.retain(|path, _| path.starts_with("src/") || path.starts_with("functions/"));
    state.agents.set_files(&id, json!(files)).await.unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/projects/{PROJECT}/agent-runs/{id}/preview"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let page = String::from_utf8_lossy(&bytes);
    if status == StatusCode::OK {
        assert_eq!(
            page.matches("\"types\":[\"BikeHireDockingStation\"]")
                .count(),
            2,
            "the type under both endpoints: {page}"
        );
    } else {
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{page}");
    }
}

/// AP-44: the second endpoint's projection bounds the need of its space, and the names are one
/// to five endpoints of the project, never beside `endpointName`.
#[tokio::test]
async fn endpoint_names_are_checked_and_each_endpoint_bounds_its_own_needs() {
    let config = config();
    let app = router(mirror_with_kpis(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs");

    let mut hidden = two_endpoint_body();
    hidden["dataNeeds"][1]["attrs"] = json!(["formulaInternal"]);
    let (status, problem) = call(&app, &cookie, Method::POST, &uri, Some(hidden)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert!(
        problem["errors"][0]["message"]
            .as_str()
            .is_some_and(|e| e.contains("hidden by endpoint 'helsinki-kpi'")),
        "{problem}"
    );

    let mut both = two_endpoint_body();
    both["endpointName"] = json!("helsinki-bikes");
    assert_eq!(
        call(&app, &cookie, Method::POST, &uri, Some(both)).await.0,
        StatusCode::BAD_REQUEST
    );

    let mut twice = two_endpoint_body();
    twice["endpointNames"] = json!(["helsinki-bikes", "helsinki-bikes"]);
    assert_eq!(
        call(&app, &cookie, Method::POST, &uri, Some(twice)).await.0,
        StatusCode::BAD_REQUEST
    );

    let mut neither = two_endpoint_body();
    neither.as_object_mut().unwrap().remove("endpointNames");
    assert_eq!(
        call(&app, &cookie, Method::POST, &uri, Some(neither))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );

    let mut unknown = two_endpoint_body();
    unknown["endpointNames"] = json!(["helsinki-bikes", "helsinki-air"]);
    assert_eq!(
        call(&app, &cookie, Method::POST, &uri, Some(unknown))
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    let mut profile = builder_profile_spec();
    profile["access"] =
        json!({ "operations": [], "endpoints": [{ "name": "helsinki-bikes", "verbs": ["read"] }] });
    let app = router(mirror_with_kpis(Some(profile)), &config);
    let (status, problem) =
        call(&app, &cookie, Method::POST, &uri, Some(two_endpoint_body())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("helsinki-kpi")),
        "{problem}"
    );
}

#[tokio::test]
async fn a_write_operation_marks_the_run_as_writing() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["dataNeeds"][0]["operations"] = json!(["queryEntity", "updateAttrs"]);
    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    assert_eq!(
        created["allowsWrite"],
        json!(true),
        "the proxy refuses a write for a run that did not declare one"
    );
}

#[tokio::test]
async fn a_portal_with_no_builder_profile_answers_503() {
    let config = config();
    let app = router(mirror(None), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{problem}");
}

#[tokio::test]
async fn a_steward_profile_builds_no_application() {
    let config = config();
    let mut spec = builder_profile_spec();
    spec["role"] = json!("steward");
    let app = router(mirror(Some(spec)), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("AG-26")),
        "{problem}"
    );
}

#[tokio::test]
async fn a_portal_with_no_agent_runner_configured_answers_503() {
    let config = Config::for_tests();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{problem}");
}

#[tokio::test]
async fn the_stream_replays_what_a_reconnecting_browser_missed() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let created = create_run(&app, &cookie).await;
    let id = created["id"].as_str().expect("an id").to_owned();

    // Two more events from the workspace, through the proxy.
    for text in ["reading the model", "scaffolding the view"] {
        let (status, receipt) = internal_call(
            &internal,
            Some(proxy_bearer()),
            Method::POST,
            "/internal/agent-runs/events",
            Some(json!({ "runId": id, "kind": "thought", "payload": { "text": text } })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{receipt}");
    }

    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/events");
    let whole = read_stream(&app, &cookie, &uri, None, 3).await;
    assert!(whole.contains("event: status"), "{whole}");
    assert!(whole.contains("id: 1"), "{whole}");
    assert!(whole.contains("scaffolding the view"), "{whole}");
    assert!(
        whole.contains("\"seq\":3"),
        "the sequence number is in the payload as well as on the frame: {whole}"
    );

    let resumed = read_stream(&app, &cookie, &uri, Some(2), 1).await;
    assert!(
        resumed.contains("scaffolding the view") && !resumed.contains("reading the model"),
        "Last-Event-ID replays what came after it and nothing before: {resumed}"
    );
}

/// The workspace asks the person a question through the proxy, as a live run does.
async fn ask(internal: &axum::Router, id: &str, question_id: &str) {
    let (status, receipt) = internal_call(
        internal,
        Some(proxy_bearer()),
        Method::POST,
        "/internal/agent-runs/events",
        Some(json!({ "runId": id, "kind": "question", "payload": { "questionId": question_id, "question": "Which centre?" } })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{receipt}");
}

#[tokio::test]
async fn an_answer_lands_on_the_runs_log() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    ask(&internal, &id, "q-center").await;

    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/answers"),
        Some(json!({ "questionId": "q-center", "answers": { "center": "Kallio" } })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let stream = read_stream(
        &app,
        &cookie,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/events"),
        Some(2),
        1,
    )
    .await;
    assert!(stream.contains("event: answer"), "{stream}");
    assert!(stream.contains("Kallio"), "{stream}");
    assert!(
        stream.contains(STEWARD),
        "who answered is part of the record: {stream}"
    );
}

/// The project's activity says a question was answered; what was typed stays on the run's
/// timeline, because a person answering a question can type a password into it (AG-80, OPS-48).
#[tokio::test]
async fn an_answer_is_on_the_project_s_activity_without_the_words() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    ask(&internal, &id, "q-center").await;

    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/answers"),
        Some(json!({ "questionId": "q-center", "answers": { "token": "hunter2" } })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, list) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/activity?kind=agent.answer"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items = list["items"].as_array().expect("a list");
    assert_eq!(items.len(), 1, "{list}");
    assert_eq!(items[0]["details"]["questionId"], json!("q-center"));
    assert_eq!(items[0]["details"]["runId"], json!(id));
    assert_eq!(
        items[0]["details"]["object"],
        json!(format!("agent-runs/{id}")),
        "the run's page filters its own activity on this"
    );
    assert!(
        items[0]["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains(STEWARD)),
        "who answered is part of the record: {list}"
    );
    assert!(
        !list.to_string().contains("hunter2"),
        "what the person typed never reaches the activity feed: {list}"
    );
}

/// An answer to a question the run never asked is refused before anything is published: it
/// would skip every bound the question sets on files, addresses and options (T-5701).
#[tokio::test]
async fn an_answer_to_a_question_never_asked_is_refused_and_logs_nothing() {
    let config = config();
    let (state, app, _) = with_state(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/answers"),
        Some(json!({ "questionId": "q-never", "answers": { "file": { "name": "a.csv", "format": "csv", "text": "x".repeat(200_000) } } })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{problem}");
    assert!(
        problem.to_string().contains("q-never"),
        "the refusal names the question id: {problem}"
    );
    let events = state.agents.events_since(&id, 0).await.expect("the log");
    assert!(
        !events.iter().any(|event| event.kind == "answer"),
        "nothing reaches the run's log"
    );
}

#[tokio::test]
async fn cancelling_ends_the_run_and_the_ticket_with_it() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, context) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::GET,
        &format!("/internal/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        context["ticketHash"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("$argon2id$")),
        "the proxy verifies a ticket against a hash, never against a secret: {context}"
    );

    let (status, cancelled) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/cancel"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancelled["status"], json!("cancelled"));
    assert!(cancelled["finishedAt"].as_str().is_some());

    let (_, context) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::GET,
        &format!("/internal/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(
        context["ticketHash"],
        json!(""),
        "AG-46: an empty hash is not a PHC string, so no ticket verifies against it again"
    );

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/cancel"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a run that is over is over: {problem}"
    );

    let (status, problem) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::POST,
        "/internal/agent-runs/events",
        Some(json!({ "runId": id, "kind": "thought", "payload": { "text": "still here" } })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a cancelled run takes no more events: {problem}"
    );
}

#[tokio::test]
async fn a_need_the_app_kind_cannot_parse_is_refused_before_the_run_exists() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    // `contextSpaceRef` is a jc-core `Ref`: a bare name, or `{ kind, name }`. A caller that
    // nests one inside the other builds a need the `App` kind cannot parse, and before this
    // guard the run was accepted and only failed when the person clicked publish.
    let mut body = create_body();
    body["dataNeeds"][0]["contextSpaceRef"]["name"] =
        json!({ "kind": "ContextSpace", "name": "helsinki" });
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    let errors = problem["errors"].as_array().expect("named violations");
    assert!(
        errors
            .iter()
            .any(|error| error.to_string().contains("dataNeeds[0]")),
        "the need is named: {problem}"
    );
}

#[tokio::test]
async fn the_published_manifest_is_an_app_the_platform_can_parse() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let created = create_run(&app, &cookie).await;

    // What `publish` writes, from the run as it was stored: a Change is refused without a forge,
    // so the manifest itself is what this asserts, against the kind that has to accept it.
    let manifest = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "App",
        "metadata": { "name": created["appName"], "namespace": PROJECT },
        "spec": {
            "kind": created["appClass"],
            "source": { "path": "./src" },
            "build": { "node": "22" },
            "visibility": created["visibility"],
            "lifecycle": "published",
            "dataNeeds": created["dataNeeds"],
        },
    });
    let spec: jc_core::kinds::AppSpec = serde_json::from_value(manifest["spec"].clone())
        .unwrap_or_else(|error| panic!("the App kind refuses what publish writes: {error}"));
    assert_eq!(spec.data_needs.len(), 1);
}

/// SDK-46: a message names the element the person pointed at as `scope`, a file and a line of
/// the application's interface; anything else, and a scope on a conversation, is refused.
#[tokio::test]
async fn a_message_scoped_to_a_pointed_element_carries_it_and_a_bad_scope_is_refused() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let messages = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages");

    for scope in [
        "functions/sum.ts:3",
        "src/main.tsx:1",
        "src/pages/Card.tsx",
        "src/../x.tsx:2",
    ] {
        let (status, body) = call(
            &app,
            &cookie,
            Method::POST,
            &messages,
            Some(json!({ "text": "red", "scope": scope })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{scope}: {body}");
    }
    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &messages,
        Some(json!({ "text": "Make this title red", "scope": "src/pages/Card.tsx:4" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::GET,
        &format!("/internal/agent-runs/{id}/inbox?after=0&wait=0"),
        None,
    )
    .await;
    let items = body["items"].as_array().expect("items");
    assert_eq!(
        items.len(),
        1,
        "only the valid message is on the log: {body}"
    );
    assert_eq!(items[0]["payload"]["scope"], "src/pages/Card.tsx:4");

    let (_, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({ "message": "Find datasets about bikes" })),
    )
    .await;
    let conversation = created["id"].as_str().expect("id");
    let (status, body) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{conversation}/messages"),
        Some(json!({ "text": "red", "scope": "src/pages/Card.tsx:4" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn a_person_steers_a_live_run_and_the_workspace_reads_it() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": "  Sort by free bikes, and add a district filter.  " })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The workspace reads one channel: what was answered and what was said, after `after`.
    let (status, body) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::GET,
        &format!("/internal/agent-runs/{id}/inbox?after=0"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{body}");
    assert_eq!(items[0]["kind"], "message");
    assert_eq!(
        items[0]["payload"]["text"],
        "Sort by free bikes, and add a district filter."
    );
    assert_eq!(items[0]["payload"]["sentBy"], STEWARD);

    // Read past it and the inbox is empty: `after` is how a workspace does not re-read.
    let seq = items[0]["seq"].as_i64().expect("a seq");
    let (status, body) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::GET,
        &format!("/internal/agent-runs/{id}/inbox?after={seq}&wait=0"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["items"]
            .as_array()
            .is_some_and(|items| items.is_empty()),
        "{body}"
    );
}

#[tokio::test]
async fn the_inbox_is_the_proxy_token_and_nothing_else() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, _) = internal_call(
        &internal,
        Some("not-the-proxy-token"),
        Method::GET,
        &format!("/internal/agent-runs/{id}/inbox?after=0"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_empty_instruction_is_refused_and_a_finished_run_reads_nothing() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");

    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/cancel"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": "one more thing" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
}

#[tokio::test]
async fn a_preview_error_lands_on_the_runs_log_and_nothing_malformed_does() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview-errors");

    for refused in [
        json!({ "message": "   " }),
        json!({ "message": "x".repeat(2001) }),
        json!({ "message": "boom", "file": "f".repeat(257) }),
        json!({ "message": "boom", "line": 0 }),
    ] {
        let (status, problem) = call(&app, &cookie, Method::POST, &uri, Some(refused)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    }
    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &uri,
        Some(json!({ "message": "boom", "token": "Bearer abc" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &uri,
        Some(json!({
            "message": " Cannot read properties of undefined (reading 'value') ",
            "file": "src/pages/Overview.tsx",
            "line": 42
        })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let stream = read_stream(
        &app,
        &cookie,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/events"),
        Some(1),
        1,
    )
    .await;
    assert!(stream.contains("event: preview_error"), "{stream}");
    assert!(
        stream.contains(r#""message":"Cannot read properties of undefined (reading 'value')""#)
            && stream.contains(r#""file":"src/pages/Overview.tsx""#)
            && stream.contains(r#""line":42"#)
            && stream.contains(STEWARD),
        "{stream}"
    );

    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/cancel"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &uri,
        Some(json!({ "message": "late" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
}

#[tokio::test]
async fn a_preview_observation_lands_once_per_version_within_its_bounds() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview-observations");
    let page = |text: String| json!({ "label": "Stations", "text": text, "rows": [5] });

    for refused in [
        json!({ "version": 0, "pages": [page("ok".into())] }),
        json!({ "version": 1, "pages": [] }),
        json!({ "version": 1, "pages": vec![page("ok".into()); 21] }),
        json!({ "version": 1, "pages": [page("x".repeat(20_001))] }),
        json!({ "version": 1, "pages": [{ "label": "l".repeat(121), "text": "ok" }] }),
        json!({ "version": 1, "pages": [{ "label": "Stations", "text": "ok", "rows": vec![1; 51] }] }),
        json!({ "version": 1, "pages": [{ "label": "Stations", "text": "ok", "h1": 51 }] }),
        json!({ "version": 1, "pages": [page("ok".into())], "failedRequests": [{ "path": "/functions/summary", "status": 200 }] }),
        json!({ "version": 1, "pages": [page("ok".into())], "failedRequests": [{ "path": "p".repeat(257), "status": 500 }] }),
    ] {
        let (status, problem) = call(&app, &cookie, Method::POST, &uri, Some(refused)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    }
    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &uri,
        Some(json!({ "version": 1, "pages": [page("ok".into())], "html": "<p>" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let observation = json!({
        "version": 1,
        "pages": [{ "label": "Stations", "text": "Kaivopuisto 7", "rows": [5], "h1": 0 }],
        "failedRequests": [{ "path": "/functions/summary", "status": 500 }]
    });
    let (status, _) = call(&app, &cookie, Method::POST, &uri, Some(observation.clone())).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let stream = read_stream(
        &app,
        &cookie,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/events"),
        Some(1),
        1,
    )
    .await;
    assert!(stream.contains("event: preview_observation"), "{stream}");
    assert!(
        stream.contains(r#""text":"Kaivopuisto 7""#)
            && stream.contains(r#""h1":0"#)
            && stream.contains(r#""failedRequests":[{"path":"/functions/summary","status":500}]"#)
            && stream.contains(STEWARD),
        "{stream}"
    );
    // A reload posts the same version again: the first observation is the one checked.
    let (status, problem) = call(&app, &cookie, Method::POST, &uri, Some(observation)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");

    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/cancel"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &uri,
        Some(json!({ "version": 2, "pages": [page("late".into())] })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
}

#[tokio::test]
async fn a_run_with_no_preview_publishes_nothing() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/publish"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("AP-46")),
        "{problem}"
    );
}

/// A code run's preview (SDK-12, SDK-16, T-0678): nothing before the run has files; its
/// `src/**` transpiled into one document with its own policy and the bridge configuration once it
/// has; every problem with file and line when they do not build.
#[tokio::test]
async fn a_code_run_previews_its_files_as_one_document() {
    let config = config();
    let (state, app, _) = with_state(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview");
    let get = || async {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let csp = response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .map(|v| v.to_str().unwrap().to_owned());
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, csp, String::from_utf8_lossy(&bytes).into_owned())
    };

    assert_eq!(get().await.0, StatusCode::NOT_FOUND);

    let mut files = preview::template_files();
    files.retain(|path, _| path.starts_with("src/") || path.starts_with("functions/"));
    state.agents.set_files(&id, json!(files)).await.unwrap();
    let (status, csp, body) = get().await;
    match status {
        StatusCode::OK => {
            let csp = csp.expect("the document's own policy");
            assert!(csp.starts_with("default-src 'none';"), "{csp}");
            assert!(csp.contains("script-src 'sha256-"), "{csp}");
            assert!(
                !csp.contains("'unsafe-inline' data:") && !csp.contains("script-src 'self'"),
                "{csp}"
            );
            assert!(body.contains("<script type=\"importmap\">"));
            assert!(
                body.contains(&format!("\"slug\":\"{SLUG}\"")),
                "the endpoint the bridge may reach"
            );
            assert!(body.contains("\"transport\":\"bridge\""));
        }
        // A Portal built without sdk/dist/runtime says so rather than serving half a document.
        StatusCode::SERVICE_UNAVAILABLE => assert!(body.contains("SDK runtime"), "{body}"),
        other => panic!("{other}: {body}"),
    }

    files.insert(
        "src/App.tsx".to_owned(),
        "import chart from \"chart.js\";\nexport default () => chart;\n".to_owned(),
    );
    state.agents.set_files(&id, json!(files)).await.unwrap();
    let (status, _, body) = get().await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let problem: Value = serde_json::from_str(&body).unwrap();
    let errors = problem["errors"].as_array().expect("every problem listed");
    assert_eq!(errors.len(), 1, "{problem}");
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("src/App.tsx:1:19: 'chart.js' may not be imported here"),
        "{problem}"
    );
}

/// A run's function through the Portal (SDK-18, SDK-23, T-0687): the run's transpiled functions,
/// the request and the caller's token go to `jc-functions` with the Portal's own token, fetched
/// once; the answer, a thrown error and a full runtime come back as the route documents them.
#[tokio::test]
async fn a_function_of_the_run_answers_through_jc_functions() {
    use joinedcontext_portal::agents::kit;
    use joinedcontext_portal::auth::oidc::OidcClient;
    use wiremock::matchers::{body_partial_json, header as has_header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let realm = MockServer::start().await;
    let issuer = format!("{}/realms/helsinki", realm.uri());
    Mock::given(method("GET"))
        .and(path("/realms/helsinki/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["ES256"]
        })))
        .mount(&realm)
        .await;
    Mock::given(method("GET"))
        .and(path("/realms/helsinki/protocol/openid-connect/certs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "keys": [] })))
        .mount(&realm)
        .await;
    Mock::given(method("POST"))
        .and(path("/realms/helsinki/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "the-portals-own-token", "token_type": "Bearer", "expires_in": 300
        })))
        .mount(&realm)
        .await;

    let runtime = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/invoke"))
        .and(has_header("authorization", "Bearer the-portals-own-token"))
        .and(body_partial_json(json!({
            "entry": "@app/entry.js",
            "request": {
                "method": "POST",
                "query": { "types": "Station" },
                "body": { "types": ["Station"] },
                "user": { "id": format!("f:1:{STEWARD}"), "email": format!("{STEWARD}@hel.fi") }
            },
            "config": { "slug": SLUG, "space": "helsinki" },
            "token": null
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 201, "body": { "types": [] }, "logs": ["summary 1 types"]
        })))
        .up_to_n_times(1)
        .mount(&runtime)
        .await;
    Mock::given(method("POST"))
        .and(path("/invoke"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 500,
            "error": { "message": "boom", "file": "@app/functions/summary.ts", "line": 3 },
            "logs": []
        })))
        .up_to_n_times(1)
        .mount(&runtime)
        .await;
    Mock::given(method("POST"))
        .and(path("/invoke"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 500,
            "error": { "message": "Authentication Required", "file": "@joinedcontext/sdk/server", "line": 80 },
            "logs": []
        })))
        .up_to_n_times(1)
        .mount(&runtime)
        .await;
    Mock::given(method("POST"))
        .and(path("/invoke"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&runtime)
        .await;

    let runtime_uri = runtime.uri();
    let config = Config::from_vars(|key| {
        match key {
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some("http://jc-agent-proxy.agents.svc.cluster.local:8080"),
            // This case has a realm of its own, because the Portal mints a token for jc-functions
            // from it; the proxy client is named all the same, so the callbacks stay closed.
            "JC_PORTAL_AGENT_PROXY_CLIENT_ID" => Some(common::AGENT_PROXY_CLIENT),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            "JC_OIDC_ISSUER" => Some(issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("joinedcontext-portal"),
            "JC_OIDC_CLIENT_SECRET" => Some("test-secret"),
            "JC_FUNCTIONS_URL" => Some(runtime_uri.as_str()),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    let oidc = OidcClient::discover(
        config.oidc.as_ref().expect("a realm"),
        "https://portal.test/api/v1/auth/callback",
    )
    .await
    .expect("discovery");
    let state =
        AppState::new(config.clone(), Some(oidc)).with_mirror(mirror(Some(builder_profile_spec())));
    let app = server::app(state.clone());
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let uri = |name: &str| format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/functions/{name}");
    let body = Some(json!({ "types": ["Station"] }));

    let (status, _) = call(&app, &cookie, Method::POST, &uri("summary"), body.clone()).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no files yet");

    let mut files = preview::template_files();
    files.retain(|path, _| path.starts_with("functions/") || path.starts_with("src/"));
    assert!(files.contains_key("functions/summary.ts"));
    state.agents.set_files(&id, json!(files)).await.unwrap();
    for name in ["Summary", "missing", "summary.test"] {
        let (status, _) = call(&app, &cookie, Method::POST, &uri(name), body.clone()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name}");
    }

    let summary = format!("{}?types=Station", uri("summary"));
    let (status, answer) = call(&app, &cookie, Method::POST, &summary, body.clone()).await;
    if kit::functions_server().is_none() {
        // A Portal built without sdk/dist says so rather than invoking without the SDK.
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{answer}");
        return;
    }
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    assert_eq!(answer, json!({ "types": [] }));

    let invoked = runtime.received_requests().await.expect("recording");
    let sent: Value = serde_json::from_slice(&invoked[0].body).unwrap();
    let modules: Vec<&str> = sent["files"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert!(
        modules.contains(&"@app/functions/summary.ts"),
        "{modules:?}"
    );
    assert!(
        modules.contains(&"@joinedcontext/sdk/server"),
        "{modules:?}"
    );
    assert!(
        !modules
            .iter()
            .any(|m| m.contains(".test.") || m.contains("/src/")),
        "neither tests nor interface files go to the runtime: {modules:?}"
    );

    let (status, answer) = call(&app, &cookie, Method::POST, &summary, body.clone()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{answer}");
    // T-5949: a Problem Details answer that keeps the function's error member.
    assert_eq!(
        answer["error"],
        json!({ "message": "boom", "file": "functions/summary.ts", "line": 3 })
    );
    assert_eq!(
        answer["type"],
        "https://joinedcontext.com/errors/internal-error"
    );
    assert_eq!(answer["detail"], "boom");
    // T-3134: the gateway refused the function's data call as unauthenticated: the caller's to
    // fix, a 401, never the function's 500.
    let (status, answer) = call(&app, &cookie, Method::POST, &summary, body.clone()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
    assert_eq!(answer["error"]["message"], "Authentication Required");
    let (status, _) = call(&app, &cookie, Method::POST, &summary, body.clone()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    let tokens = realm
        .received_requests()
        .await
        .expect("recording")
        .into_iter()
        .filter(|r| r.method == wiremock::http::Method::POST)
        .count();
    assert_eq!(tokens, 1, "the Portal's token is fetched once and kept");

    let stream = read_stream(
        &app,
        &cookie,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/events"),
        Some(1),
        4,
    )
    .await;
    assert!(
        stream.contains(r#""tool":"function:summary""#)
            && stream.contains(r#""output":{"logs":[],"status":401}"#)
            && stream.contains(r#""logs":["summary 1 types"]"#)
            && stream.contains(r#""status":201"#)
            && stream.contains(r#""file":"functions/summary.ts""#)
            && stream.contains(r#""error":"jc-functions is full""#),
        "{stream}"
    );

    files.insert(
        "functions/broken.ts".to_owned(),
        "export default (".to_owned(),
    );
    state.agents.set_files(&id, json!(files)).await.unwrap();
    let (status, problem) = call(&app, &cookie, Method::POST, &summary, body.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert!(
        problem["errors"][0]["message"]
            .as_str()
            .is_some_and(|e| e.starts_with("functions/broken.ts:")),
        "{problem}"
    );

    let (status, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/cancel"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, &cookie, Method::POST, &summary, body).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

/// Without a runtime address a function call is 503, not a call that goes nowhere.
#[tokio::test]
async fn a_portal_without_jc_functions_answers_503() {
    let config = config();
    let (state, app, _) = with_state(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let mut files = preview::template_files();
    files.retain(|path, _| path.starts_with("functions/"));
    state.agents.set_files(&id, json!(files)).await.unwrap();
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/functions/summary"),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("JC_FUNCTIONS_URL")),
        "{problem}"
    );
}

#[tokio::test]
async fn the_lifecycle_is_driven_by_the_workspace_through_the_proxy() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    for state in [
        "starting",
        "interviewing",
        "building",
        "testing",
        "previewing",
    ] {
        let (status, body) = internal_call(
            &internal,
            Some(proxy_bearer()),
            Method::POST,
            "/internal/agent-runs/events",
            Some(json!({ "runId": id, "kind": "status", "payload": { "status": state } })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{state}: {body}");
    }

    let (status, body) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::POST,
        "/internal/agent-runs/events",
        Some(json!({ "runId": id, "kind": "status", "payload": { "status": "published" } })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "a workspace does not publish its own application: {body}"
    );

    let (_, run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(run["status"], json!("previewing"));
    assert!(run["startedAt"].as_str().is_some());
}

#[tokio::test]
async fn usage_and_the_preview_url_are_recorded_from_the_stream() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    for tokens in [4_120, 1_880] {
        internal_call(
            &internal,
            Some(proxy_bearer()),
            Method::POST,
            "/internal/agent-runs/events",
            Some(json!({
                "runId": id,
                "kind": "usage",
                "payload": { "tokensThisStep": tokens }
            })),
        )
        .await;
    }
    internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::POST,
        "/internal/agent-runs/events",
        Some(json!({
            "runId": id,
            "kind": "preview",
            "payload": { "previewUrl": "https://portal.hel.fi/apps/city-bikes-overview/" }
        })),
    )
    .await;

    let (_, run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(run["tokensUsed"], json!(6_000));
    assert_eq!(run["steps"], json!(2));
    assert_eq!(
        run["previewUrl"],
        json!("https://portal.hel.fi/apps/city-bikes-overview/")
    );
}

#[tokio::test]
async fn the_internal_listener_answers_the_proxy_and_nobody_else() {
    let config = config();
    let internal = internal_router(mirror(Some(builder_profile_spec())), &config);

    for bearer in [None, Some("not-the-proxy-token")] {
        let (status, _) = internal_call(
            &internal,
            bearer,
            Method::GET,
            "/internal/agent-runs/whatever",
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "AG-52: the ticket hash leaves the Portal for the proxy alone"
        );
    }

    let (status, _) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::GET,
        "/internal/agent-runs/no-such-run",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_internal_routes_are_not_on_the_public_surface() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    // The routed listener answers unknown paths with the single-page application, so what is
    // asserted is that no answer is a run context: the ticket hash is what must not be there.
    for uri in [
        "/internal/agent-runs/some-run",
        "/api/v1/internal/agent-runs/some-run",
        "/api/v1/projects/helsinki/internal/agent-runs/some-run",
    ] {
        let (_, body) = call(&app, &cookie, Method::GET, uri, None).await;
        assert!(
            body.get("ticketHash").is_none(),
            "{uri} answered a run context on the routed listener (AG-52): {body}"
        );
    }
}

#[tokio::test]
async fn a_caller_who_may_not_propose_an_app_starts_no_run() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    // No bootstrap role and no RoleBinding in the mirror: nothing grants proposing an App.
    let cookie = session_cookie(&config, "curious.reader", &[]);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
}

#[tokio::test]
async fn a_request_the_app_kind_would_refuse_is_refused_here() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    for (field, value) in [
        ("appName", json!("Not A Label")),
        ("appClass", json!("not-a-class")),
        ("visibility", json!("everyone")),
        ("prompt", json!("   ")),
    ] {
        let mut body = create_body();
        body[field] = value.clone();
        let (status, problem) = call(
            &app,
            &cookie,
            Method::POST,
            &format!("/api/v1/projects/{PROJECT}/agent-runs"),
            Some(body),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{field} = {value} was accepted: {problem}"
        );
    }
}

/// AP-124: `ui-node` is refused until it is built and `service` is withdrawn, each in jc-core's
/// words naming `ui` and `ui-rust`; the previous release's `fullstack` is read, and recorded, as
/// `ui-rust`.
#[tokio::test]
async fn the_app_shape_is_ui_or_ui_rust_and_the_old_name_is_recorded_in_the_new() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    for (class, says) in [("ui-node", "not built yet"), ("service", "withdrawn")] {
        let mut body = create_body();
        body["appClass"] = json!(class);
        let (status, problem) = call(
            &app,
            &cookie,
            Method::POST,
            &format!("/api/v1/projects/{PROJECT}/agent-runs"),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{class}: {problem}");
        let detail = problem["detail"].as_str().expect("a detail");
        assert!(detail.contains(says), "{class}: {detail}");
        assert!(
            detail.contains("`ui`") && detail.contains("`ui-rust`"),
            "{class}: {detail}"
        );
    }

    let mut body = create_body();
    body["appClass"] = json!("fullstack");
    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    assert_eq!(created["appClass"], json!("ui-rust"), "{created}");
}

#[tokio::test]
async fn the_diagnostics_door_answers_the_proxy_for_the_runs_own_project_only() {
    let config = config();
    let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    // AG-57: the door opens for the proxy alone.
    let (status, _) = internal_call(
        &internal,
        None,
        Method::GET,
        &format!("/internal/agent-runs/{id}/diagnostics/pipeline/hsl-bikes"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A run that does not exist has no project to look into.
    let (status, _) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::GET,
        "/internal/agent-runs/no-such-run/diagnostics/pipeline/hsl-bikes",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Only the components the door knows.
    let (status, problem) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::GET,
        &format!("/internal/agent-runs/{id}/diagnostics/endpoint/hsl-bikes"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");

    // A pipeline the run's project does not have is not found, whatever other projects hold.
    let (status, problem) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::GET,
        &format!("/internal/agent-runs/{id}/diagnostics/pipeline/hsl-bikes"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{problem}");

    // A change needs the forge; without one the door says so instead of inventing a state.
    let (status, problem) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::GET,
        &format!("/internal/agent-runs/{id}/diagnostics/change/chg-0000000a"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{problem}");
}

#[tokio::test]
async fn a_second_create_for_the_same_app_while_live_is_conflict_and_succeeds_after_cancel() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let created = create_run(&app, &cookie).await;
    let id1 = created["id"].as_str().expect("id").to_owned();

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains(&id1)),
        "the 409 detail names the live run id: {problem}"
    );

    let (status, list) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().map(Vec::len), Some(1));

    let (status, cancelled) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id1}/cancel"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancelled["status"], json!("cancelled"));

    let (status, created2) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created2}");
    let id2 = created2["id"].as_str().expect("id2");
    assert_ne!(id1, id2);

    let (status, list2) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list2["items"].as_array().map(Vec::len), Some(2));
}

#[tokio::test]
async fn list_runs_filters_by_app_query_parameter() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let created1 = create_run(&app, &cookie).await;
    let id1 = created1["id"].as_str().expect("id").to_owned();

    let mut body2 = create_body();
    body2["appName"] = json!("other-app");
    let (status, created2) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body2),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let id2 = created2["id"].as_str().expect("id").to_owned();

    let (status, list1) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?app=city-bikes-overview"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items1 = list1["items"].as_array().expect("items");
    assert_eq!(items1.len(), 1);
    assert_eq!(items1[0]["id"], id1);
    assert_eq!(items1[0]["appName"], "city-bikes-overview");

    let (status, list2) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?app=other-app"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items2 = list2["items"].as_array().expect("items");
    assert_eq!(items2.len(), 1);
    assert_eq!(items2[0]["id"], id2);
    assert_eq!(items2[0]["appName"], "other-app");

    let (status, list_none) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?app=nonexistent"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items_none = list_none["items"].as_array().expect("items");
    assert_eq!(items_none.len(), 0);
}

#[tokio::test]
async fn expired_runs_are_reaped_and_ticket_invalidated_while_live_runs_remain() {
    let config = config();
    let (state, app, internal) = with_state(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let live_created = create_run(&app, &cookie).await;
    let live_id = live_created["id"].as_str().expect("id").to_owned();

    let (_ticket, ticket_hash) = mint_ticket();
    let expired_id = mint_run_id();
    let expired_run = AgentRun {
        id: expired_id.clone(),
        project: PROJECT.to_owned(),
        app_name: "expired-app".to_owned(),
        title: None,
        endpoint_name: "helsinki-bikes".to_owned(),
        endpoint_slug: SLUG.to_owned(),
        endpoints: serde_json::json!([]),
        profile: "app-builder".to_owned(),
        kind: "application".to_owned(),
        unattended: false,
        continues: None,
        app_class: "ui-rust".to_owned(),
        visibility: "project".to_owned(),
        prompt: "Expired prompt".to_owned(),
        prompt_digest: digest_prompt("Expired prompt"),
        data_needs: json!([]),
        allows_write: false,
        branch: format!("agent/app-expired-app/{expired_id}"),
        path_prefix: format!("projects/{PROJECT}/apps/expired-app/"),
        status: "building".to_owned(),
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
        steps: 5,
        tokens_used: 1000,
        created_by: STEWARD.to_owned(),
        origin: "person".to_owned(),
        starter: serde_json::Value::Null,
        created_at: "2020-01-01T00:00:00Z".to_owned(),
        started_at: Some("2020-01-01T00:01:00Z".to_owned()),
        finished_at: None,
        expires_at: "2020-01-01T00:20:00Z".to_owned(),
        error: None,
    };
    state
        .agents
        .create_run(&expired_run)
        .await
        .expect("create expired run");

    let (status, context) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::GET,
        &format!("/internal/agent-runs/{expired_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!context["ticketHash"].as_str().unwrap_or("").is_empty());

    let reaped = reaper::reap_expired(&state).await;
    assert_eq!(reaped, 1);

    let (status, run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{expired_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(run["status"], json!("expired"));
    assert_eq!(
        run["error"],
        json!("the run's time ran out before it finished; start it again from the Apps page")
    );
    assert!(run["finishedAt"].as_str().is_some());

    let (_, context) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::GET,
        &format!("/internal/agent-runs/{expired_id}"),
        None,
    )
    .await;
    assert_eq!(
        context["ticketHash"],
        json!(""),
        "the ticket hash is cleared after reaping"
    );

    let (status, problem) = internal_call(
        &internal,
        Some(proxy_bearer()),
        Method::POST,
        "/internal/agent-runs/events",
        Some(
            json!({ "runId": expired_id, "kind": "thought", "payload": { "text": "still here" } }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "an expired run takes no more events: {problem}"
    );

    let (status, live_run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{live_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(live_run["status"], json!("queued"));
    assert!(live_run.get("error").is_none() || live_run["error"].is_null());

    let reaped_again = reaper::reap_expired(&state).await;
    assert_eq!(reaped_again, 0);
}

/// T-2772: a run that reached `awaiting_approval` waits on the approval's lease, days rather than
/// the build's twenty minutes, so the reaper leaves it; once that lease runs out too, the run
/// says what was lost and that its change stays open, on the record and on the stream.
#[tokio::test]
async fn a_run_waiting_for_approval_keeps_days_and_says_what_its_expiry_lost() {
    let config = config();
    let (state, app, internal) = with_state(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let created = create_run(&app, &cookie).await;
    let id = created["id"].as_str().expect("id").to_owned();

    for status in [
        "starting",
        "building",
        "testing",
        "previewing",
        "awaiting_approval",
    ] {
        let (code, answer) = internal_call(
            &internal,
            Some(proxy_bearer()),
            Method::POST,
            "/internal/agent-runs/events",
            Some(json!({ "runId": id, "kind": "status", "payload": { "status": status } })),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "{status}: {answer}");
    }
    let run = state
        .agents
        .get_run(&id)
        .await
        .expect("store")
        .expect("run");
    assert_eq!(run.status, "awaiting_approval");
    let lease = chrono::DateTime::parse_from_rfc3339(&run.expires_at).expect("an RFC 3339 lease");
    assert!(
        lease.with_timezone(&chrono::Utc) - chrono::Utc::now() > chrono::Duration::days(6),
        "the approval lease is days: {}",
        run.expires_at
    );
    assert_eq!(
        reaper::reap_expired(&state).await,
        0,
        "a run inside its lease stays"
    );

    state
        .agents
        .set_expiry(&id, "2000-01-01T00:00:00Z")
        .await
        .expect("an old lease");
    assert_eq!(reaper::reap_expired(&state).await, 1);
    let (_, ended) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(ended["status"], json!("expired"));
    let error = ended["error"].as_str().unwrap_or_default();
    assert!(error.contains("the change stays open"), "{error}");
    let events = state.agents.events_since(&id, 0).await.expect("events");
    let last = events
        .iter()
        .rev()
        .find(|event| event.kind == "status")
        .expect("a status event");
    assert_eq!(last.payload["status"], "expired");
    assert_eq!(last.payload["reason"], json!(error), "the stream says why");
}

#[tokio::test]
async fn a_run_that_never_renders_records_neither_first_frame_nor_first_version() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let created = create_run(&app, &cookie).await;
    let id = created["id"].as_str().expect("an id");

    assert!(
        created.get("firstFrameMs").is_none(),
        "firstFrameMs must be omitted when None: {created}"
    );
    assert!(
        created.get("firstVersionMs").is_none(),
        "firstVersionMs must be omitted when None: {created}"
    );

    let (status, run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        run.get("firstFrameMs").is_none(),
        "firstFrameMs must be omitted when None: {run}"
    );
    assert!(
        run.get("firstVersionMs").is_none(),
        "firstVersionMs must be omitted when None: {run}"
    );
}

#[tokio::test]
async fn conversation_starts_with_only_a_message_and_has_kind_conversation() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({ "message": "Find datasets about bikes" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    assert_eq!(created["kind"], json!("conversation"));
    assert_eq!(created["appName"], json!(""));
    assert_eq!(created["endpointName"], json!(""));
    assert_eq!(created["endpointSlug"], json!(""));
    assert_eq!(created["appClass"], json!("ui"));
    assert_eq!(created["visibility"], json!("private"));
    assert_eq!(created["unattended"], json!(false));
    assert!(created["continues"].is_null());
    assert_eq!(created["prompt"], json!("Find datasets about bikes"));
    assert_eq!(created["status"], json!("queued"));

    let id = created["id"].as_str().expect("id");

    let (status, filtered) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?app=city-bikes-overview"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = filtered["items"].as_array().expect("items");
    assert!(!items.iter().any(|item| item["id"] == id));

    let (status, single) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(single["kind"], json!("conversation"));
}

#[tokio::test]
async fn conversation_needs_propose_permission_on_app() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, "curious.reader", &[]);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({ "message": "Hello" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
}

#[tokio::test]
async fn continues_validations_reject_invalid_runs() {
    let config = config();
    let (state, app, _) = with_state(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    // 1. Missing run id -> 400
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({
            "message": "Continue please",
            "continues": "00000000-0000-0000-0000-000000000000"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");

    // 2. Application run (kind is not conversation) -> 400
    let app_run = create_run(&app, &cookie).await;
    let app_run_id = app_run["id"].as_str().expect("id");
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({
            "message": "Continue please",
            "continues": app_run_id
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");

    // 3. Conversation run of another project -> 400
    let other_run_id = mint_run_id();
    let (_, ticket_hash) = mint_ticket();
    let other_project_run = AgentRun {
        id: other_run_id.clone(),
        project: "espoo".to_owned(),
        app_name: "".to_owned(),
        title: None,
        endpoint_name: "".to_owned(),
        endpoint_slug: "".to_owned(),
        endpoints: serde_json::json!([]),
        profile: "app-builder".to_owned(),
        kind: "conversation".to_owned(),
        unattended: false,
        continues: None,
        app_class: "ui".to_owned(),
        visibility: "private".to_owned(),
        prompt: "Earlier conversation".to_owned(),
        prompt_digest: digest_prompt("Earlier conversation"),
        data_needs: json!([]),
        allows_write: false,
        branch: "".to_owned(),
        path_prefix: "".to_owned(),
        status: "interviewing".to_owned(),
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
        created_by: STEWARD.to_owned(),
        origin: "person".to_owned(),
        starter: serde_json::Value::Null,
        created_at: "2026-09-12T08:00:00Z".to_owned(),
        started_at: None,
        finished_at: None,
        expires_at: "2026-09-12T08:30:00Z".to_owned(),
        error: None,
    };
    state
        .agents
        .create_run(&other_project_run)
        .await
        .expect("create run");

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({
            "message": "Continue please",
            "continues": other_run_id
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
}

#[tokio::test]
async fn kind_filter_in_list_runs() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let app_run = create_run(&app, &cookie).await;
    let app_id = app_run["id"].as_str().expect("id");

    let (status, conv_run) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({ "message": "List test conversation" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let conv_id = conv_run["id"].as_str().expect("id");

    let (status, list) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?kind=conversation"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().expect("items");
    assert!(items.iter().any(|item| item["id"] == conv_id));
    assert!(!items.iter().any(|item| item["id"] == app_id));
    assert!(items.iter().all(|item| item["kind"] == "conversation"));

    let (status, list_app) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?kind=application"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items_app = list_app["items"].as_array().expect("items");
    assert!(items_app.iter().any(|item| item["id"] == app_id));
    assert!(!items_app.iter().any(|item| item["id"] == conv_id));
    assert!(items_app.iter().all(|item| item["kind"] == "application"));
}

#[tokio::test]
async fn caller_without_portal_approver_sees_only_own_runs_while_approver_sees_both() {
    let config = config();
    let (state, app, _) = with_state(mirror(Some(builder_profile_spec())), &config);
    let approver_cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let viewer_cookie = session_cookie(&config, "viewer.user", &["viewer"]);

    let approver_run = create_run(&app, &approver_cookie).await;
    let approver_run_id = approver_run["id"].as_str().expect("id");

    let viewer_run_id = mint_run_id();
    let (_, ticket_hash) = mint_ticket();
    let viewer_run = AgentRun {
        id: viewer_run_id.clone(),
        project: PROJECT.to_owned(),
        app_name: "viewer-app".to_owned(),
        title: None,
        endpoint_name: "helsinki-bikes".to_owned(),
        endpoint_slug: SLUG.to_owned(),
        endpoints: serde_json::json!([]),
        profile: "app-builder".to_owned(),
        kind: "application".to_owned(),
        unattended: false,
        continues: None,
        app_class: "ui".to_owned(),
        visibility: "project".to_owned(),
        prompt: "Viewer app".to_owned(),
        prompt_digest: digest_prompt("Viewer app"),
        data_needs: json!([]),
        allows_write: false,
        branch: format!("agent/app-viewer-app/{viewer_run_id}"),
        path_prefix: format!("projects/{PROJECT}/apps/viewer-app/"),
        status: "building".to_owned(),
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
        created_by: "viewer.user".to_owned(),
        origin: "person".to_owned(),
        starter: serde_json::Value::Null,
        created_at: "2026-09-12T09:00:00Z".to_owned(),
        started_at: None,
        finished_at: None,
        expires_at: "2026-09-12T09:30:00Z".to_owned(),
        error: None,
    };
    state
        .agents
        .create_run(&viewer_run)
        .await
        .expect("create viewer run");

    let (status, list) = call(
        &app,
        &approver_cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().expect("items");
    assert!(items.iter().any(|item| item["id"] == approver_run_id));
    assert!(items.iter().any(|item| item["id"] == viewer_run_id));

    let (status, list_mine) = call(
        &app,
        &approver_cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs?mine=true"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items_mine = list_mine["items"].as_array().expect("items");
    assert!(items_mine.iter().any(|item| item["id"] == approver_run_id));
    assert!(!items_mine.iter().any(|item| item["id"] == viewer_run_id));

    // The viewer reads the project, as someone who has run an agent there does; a caller with
    // no binding is answered as if the project were not there (PF-59, T-1368).
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "RoleBinding".to_owned(),
        metadata: ObjectMeta::new("viewer-reads", "org"),
        spec: json!({
            "subjects": [{ "user": "viewer.user" }],
            "role": "viewer",
            "scope": { "project": PROJECT },
        }),
        status: None,
    });
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: "Role".to_owned(),
        metadata: ObjectMeta::new("viewer", "org"),
        spec: json!({ "rules": [{ "kinds": ["Endpoint"], "verbs": ["read"] }] }),
        status: None,
    });
    let (status, list_viewer) = call(
        &app,
        &viewer_cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items_viewer = list_viewer["items"].as_array().expect("items");
    assert!(items_viewer.iter().any(|item| item["id"] == viewer_run_id));
    assert!(!items_viewer
        .iter()
        .any(|item| item["id"] == approver_run_id));
}

#[tokio::test]
async fn create_run_refuses_kind_conversation() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["kind"] = json!("conversation");
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("/assistant/conversations")),
        "{problem}"
    );
}

#[tokio::test]
async fn analysis_run_is_unattended_and_publish_answers_409() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["kind"] = json!("analysis");
    body["unattended"] = json!(false);
    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    assert_eq!(created["kind"], json!("analysis"));
    assert_eq!(created["unattended"], json!(true));

    let id = created["id"].as_str().expect("id");
    let (status, run) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(run["kind"], json!("analysis"));
    assert_eq!(run["unattended"], json!(true));

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/publish"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("an analysis is never published")),
        "{problem}"
    );
}

/// T-3159: a dashboard only reads; data needs that write are refused before anything is recorded.
#[tokio::test]
async fn a_dashboard_whose_data_needs_write_is_refused() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["kind"] = json!("dashboard");
    body["dataNeeds"][0]["operations"] = json!(["queryEntity", "updateAttrs"]);
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("a dashboard only reads")),
        "{problem}"
    );
}

/// T-3160: an analysis only reads; data needs that write are refused before anything is recorded.
#[tokio::test]
async fn an_analysis_whose_data_needs_write_is_refused() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["kind"] = json!("analysis");
    body["dataNeeds"][0]["operations"] = json!(["queryEntity", "updateAttrs"]);
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("an analysis only reads")),
        "{problem}"
    );
}

#[tokio::test]
async fn dashboard_run_is_unattended_and_publish_answers_409() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let mut body = create_body();
    body["kind"] = json!("dashboard");
    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    assert_eq!(created["kind"], json!("dashboard"));
    assert_eq!(created["unattended"], json!(true));

    let id = created["id"].as_str().expect("id");
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/publish"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .is_some_and(|d| d.contains("publishing a dashboard run is not available yet")),
        "{problem}"
    );
}

#[tokio::test]
async fn a_profile_that_lists_endpoints_refuses_a_run_on_one_it_does_not_grant() {
    let config = config();
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let with_endpoints = |endpoints: Value| {
        let mut spec = builder_profile_spec();
        spec["access"] = json!({ "operations": ["jc_catalog_search"], "endpoints": endpoints });
        router(mirror(Some(spec)), &config)
    };

    let app = with_endpoints(json!([{ "name": "helsinki-air", "verbs": ["read"] }]));
    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    assert!(problem["detail"]
        .as_str()
        .is_some_and(|d| d.contains("helsinki-bikes") && d.contains("AG-70")));

    let app = with_endpoints(json!([{ "name": "helsinki-bikes", "verbs": ["read"] }]));
    create_run(&app, &cookie).await;
}

/// PF-73, PF-74: a project starts as many runs a day as its quota allows, and the next one is
/// refused with the count and the limit. Yesterday's runs are not today's.
#[tokio::test]
async fn the_run_above_the_daily_quota_is_refused_with_the_count_and_the_limit() {
    let config = config();
    let mirror = mirror(Some(builder_profile_spec()));
    mirror.upsert(envelope(
        "Organization",
        "bb",
        "org",
        json!({ "domain": "hel.fi", "projects": { "quota": { "agentRunsPerDay": 2 } } }),
    ));
    let app = router(mirror, &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    // One live run per application, so each run is its own app.
    let named = |app_name: &str| {
        let mut body = create_body();
        body["appName"] = json!(app_name);
        body
    };
    for app_name in ["first-app", "second-app"] {
        let (status, body) = call(
            &app,
            &cookie,
            Method::POST,
            &format!("/api/v1/projects/{PROJECT}/agent-runs"),
            Some(named(app_name)),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    }

    let (status, body) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(named("third-app")),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body.to_string().contains("agentRunsPerDay 3 of 2"),
        "the refusal names the count and the limit: {body}"
    );
}

/// PF-74, T-1403: a conversation is a run and counts against the same day. Two runs of any kind
/// fill a quota of two, and the next conversation is refused like the next application run,
/// before a run is stored; before, a conversation was never refused.
#[tokio::test]
async fn a_conversation_above_the_daily_quota_is_refused_like_any_run() {
    let config = config();
    let mirror = mirror(Some(builder_profile_spec()));
    mirror.upsert(envelope(
        "Organization",
        "bb",
        "org",
        json!({ "domain": "hel.fi", "projects": { "quota": { "agentRunsPerDay": 2 } } }),
    ));
    let app = router(mirror, &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let converse = |message: &str| {
        let (app, cookie) = (app.clone(), cookie.clone());
        let body = json!({ "message": message });
        async move {
            call(
                &app,
                &cookie,
                Method::POST,
                &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
                Some(body),
            )
            .await
        }
    };

    let mut body = create_body();
    body["appName"] = json!("first-app");
    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let (status, created) = converse("Find datasets about bikes").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");

    let (status, refused) = converse("And about air quality?").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert!(
        refused.to_string().contains("agentRunsPerDay 3 of 2"),
        "the refusal names the count and the limit: {refused}"
    );
    let (_, listed) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        None,
    )
    .await;
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(2),
        "{listed}"
    );
}

/// A run reaches the operations registry through one door and arrives narrowed twice: the person
/// who started it, and the run's own profile (AG-64, AG-70, T-0837).
mod the_registry_a_run_reaches {
    use super::*;

    /// The JSON-RPC message the proxy relays, and what came back.
    async fn mcp(internal: &axum::Router, run: &str, message: Value) -> (StatusCode, Value) {
        internal_call(
            internal,
            Some(proxy_bearer()),
            Method::POST,
            &format!("/internal/agent-runs/{run}/mcp"),
            Some(message),
        )
        .await
    }

    /// A profile that names an operation and the verb it needs: what an author writes to let a
    /// run propose a change (AG-70).
    fn profile_naming(operations: &[&str], kind_verbs: &[(&str, &[&str])]) -> Value {
        let mut spec = builder_profile_spec();
        spec["access"] = json!({
            "operations": operations,
            "kinds": kind_verbs.iter().map(|(kind, verbs)| json!({ "kind": kind, "verbs": verbs })).collect::<Vec<_>>(),
        });
        spec
    }

    async fn run_for(app: &axum::Router, config: &Config) -> String {
        let cookie = session_cookie(config, STEWARD, &["portal-approver"]);
        create_run(app, &cookie).await["id"]
            .as_str()
            .expect("an id")
            .to_owned()
    }

    /// A profile without an access block grants the read-only operations and nothing else, so
    /// the catalogue a run reads never offers what its call would refuse.
    #[tokio::test]
    async fn tools_list_offers_only_what_the_profile_names() {
        let config = config();
        let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
        let run = run_for(&app, &config).await;

        let (status, answer) = mcp(
            &internal,
            &run,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                    "params": { "project": PROJECT } }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        let names: Vec<&str> = answer["result"]["tools"]
            .as_array()
            .expect("a tool list")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(!names.is_empty(), "the run reads nothing at all: {answer}");
        assert!(
            !names.contains(&"jc_resource_delete"),
            "a profile that grants no deletion offered one: {names:?}"
        );
        assert!(
            !names.contains(&"jc_change_approve"),
            "a profile that grants no approval offered one: {names:?}"
        );
        assert!(
            names.contains(&"jc_resource_list"),
            "reading is what every profile grants: {names:?}"
        );
    }

    /// The same list from the person's own door, with no run: the profile is what took the rest
    /// away, and a steward at a keyboard still has it.
    #[tokio::test]
    async fn the_person_who_started_the_run_keeps_what_the_profile_took() {
        let config = config();
        let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
        let run = run_for(&app, &config).await;

        let (_, narrowed) = mcp(
            &internal,
            &run,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                    "params": { "project": PROJECT } }),
        )
        .await;
        let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
        let (_, whole) = call(
            &app,
            &cookie,
            Method::GET,
            &format!("/api/v1/projects/{PROJECT}/ops"),
            None,
        )
        .await;
        let offered = narrowed["result"]["tools"].as_array().map_or(0, Vec::len);
        let person = whole.as_array().map_or(0, Vec::len);
        assert!(
            offered < person,
            "the run was offered {offered} of the person's {person} operations"
        );
    }

    /// AG-11 holds on this door too: a run never decides a change, whatever its profile says.
    #[tokio::test]
    async fn a_run_never_approves_or_rejects_a_change() {
        let config = config();
        let profile = profile_naming(
            &["jc_change_approve", "jc_change_reject"],
            &[("Change", &["approve"])],
        );
        let (app, internal) = both(mirror(Some(profile)), &config);
        let run = run_for(&app, &config).await;

        for tool in ["jc_change_approve", "jc_change_reject"] {
            let (status, answer) = mcp(
                &internal,
                &run,
                json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                    "name": tool,
                    "arguments": { "project": PROJECT, "id": "chg-00000001", "reason": "no" }
                }}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{answer}");
            assert_eq!(answer["result"]["isError"], json!(true), "{answer}");
            let said = serde_json::to_string(&answer).unwrap_or_default();
            assert!(
                said.contains("an agent never approves or rejects a change"),
                "{tool}: {said}"
            );
        }
    }

    /// A run that has ended calls nothing, and a message larger than the door reads is refused
    /// before it is parsed.
    #[tokio::test]
    async fn a_finished_run_and_an_oversized_message_are_refused() {
        let config = config();
        let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
        let run = run_for(&app, &config).await;

        let (status, _) = mcp(
            &internal,
            &run,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                "name": "jc_resource_list",
                "arguments": { "project": PROJECT, "kind": "Endpoint", "pad": "x".repeat(2 * 1024 * 1024) }
            }}),
        )
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

        let (status, _) = internal_call(
            &internal,
            Some(proxy_bearer()),
            Method::POST,
            "/internal/agent-runs/no-such-run/mcp",
            Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// The door is the proxy's alone: a workspace that reached it with any other token is nobody.
    #[tokio::test]
    async fn the_door_is_the_proxy_token_and_nothing_else() {
        let config = config();
        let (app, internal) = both(mirror(Some(builder_profile_spec())), &config);
        let run = run_for(&app, &config).await;

        let (status, _) = internal_call(
            &internal,
            Some("not-the-proxy-token"),
            Method::POST,
            &format!("/internal/agent-runs/{run}/mcp"),
            Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

// ---------------------------------------------------------------------------------------------
// One repository per static application (AP-75…AP-78, T-2468).
// ---------------------------------------------------------------------------------------------

/// A forge whose public address is https, as a clone address has to be (AP-02).
fn forge(server: &wiremock::MockServer) -> Arc<joinedcontext_portal::git::GiteaClient> {
    let mut gitea = joinedcontext_portal::git::GiteaClient::new(
        server.uri().parse().expect("a url"),
        "joinedcontext",
        "configuration",
        "forge-token",
    )
    .expect("a client");
    gitea.public_base = "https://forge.example/".parse().expect("a url");
    Arc::new(gitea)
}

fn static_body(app: &str) -> Value {
    let mut body = create_body();
    body["appName"] = json!(app);
    body["appClass"] = json!("ui");
    body
}

/// A static application's run as the store holds it once it has a preview, owned by the steward.
fn a_static_run(status: AgentRunStatus) -> AgentRun {
    let id = mint_run_id();
    serde_json::from_value(json!({
        "id": id,
        "project": PROJECT,
        "appName": "city-bikes",
        "title": "City bikes",
        "endpointName": "helsinki-bikes",
        "endpointSlug": SLUG,
        "profile": "app-builder",
        "kind": "application",
        "unattended": false,
        "appClass": "ui",
        "visibility": "project",
        "prompt": "p",
        "promptDigest": digest_prompt("p"),
        "dataNeeds": [],
        "allowsWrite": false,
        "branch": format!("agent/app-city-bikes/{id}"),
        "pathPrefix": "",
        "status": status.as_str(),
        "ticketHash": mint_ticket().1,
        "steps": 0,
        "tokensUsed": 0,
        "createdBy": STEWARD,
        "createdAt": "2026-09-21T06:00:00Z",
        "expiresAt": "2099-09-21T06:00:00Z"
    }))
    .expect("a run")
}

/// AP-75, AP-78: a static application's run writes at the root of its own repository, links that
/// repository (the name the forge would refuse is `agents::repository`'s own case).
#[tokio::test]
async fn a_static_application_run_lives_in_its_own_repository() {
    let server = wiremock::MockServer::start().await;
    let config = config();
    let state = AppState::new(config.clone(), None)
        .with_mirror(mirror(Some(builder_profile_spec())))
        .with_gitea(forge(&server));
    let app = server::app(state);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(static_body("city-bikes")),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let run = &created;
    assert_eq!(run["pathPrefix"], json!(""), "{created}");
    let source = run["sourceUrl"].as_str().unwrap_or_default().to_owned();
    let (_, read) = call(
        &app,
        &cookie,
        Method::GET,
        &format!(
            "/api/v1/projects/{PROJECT}/agent-runs/{}",
            run["id"].as_str().unwrap_or_default()
        ),
        None,
    )
    .await;
    let source = read["sourceUrl"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or(source);
    assert!(
        source.starts_with("https://forge.example/") && source.contains("helsinki_city-bikes"),
        "the run links its application's repository: {source}"
    );
    assert!(!source.contains("configuration"), "{source}");
    assert!(
        read.get("mirrorUrl").is_none(),
        "no GitHub copy is linked where the installation keeps none: {read}"
    );
}

/// AP-79: where the installation keeps application repositories on GitHub as well, the run links
/// the copy on its branch beside the forge's repository of record.
#[tokio::test]
async fn a_static_run_links_its_github_copy_beside_the_forge() {
    let server = wiremock::MockServer::start().await;
    let config = config();
    let github =
        joinedcontext_portal::git::github_mirror::GithubMirror::from_env(|name| match name {
            "JC_APP_MIRROR_GITHUB_OWNER" => Some("hel-apps".to_owned()),
            "JC_APP_MIRROR_GITHUB_TOKEN" => Some("ghp_never_shown".to_owned()),
            _ => None,
        })
        .expect("valid")
        .expect("on");
    let state = AppState::new(config.clone(), None)
        .with_mirror(mirror(Some(builder_profile_spec())))
        .with_gitea(forge(&server))
        .with_github_mirror(Arc::new(github));
    let app = server::app(state);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(static_body("city-bikes")),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let id = created["id"].as_str().unwrap_or_default().to_owned();
    let (_, read) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}"),
        None,
    )
    .await;
    assert_eq!(
        read["mirrorUrl"],
        json!(format!(
            "https://github.com/hel-apps/helsinki_city-bikes/tree/agent/app-city-bikes/{id}"
        )),
        "{read}"
    );
    assert!(read["sourceUrl"]
        .as_str()
        .unwrap_or_default()
        .starts_with("https://forge.example/"));
    assert!(!read.to_string().contains("ghp_never_shown"), "{read}");
}

/// AP-77: a static run with nothing committed to its branch is not published, and the refusal
/// says what to wait for; the run does not move.
#[tokio::test]
async fn a_static_run_with_no_commit_is_not_published() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(
            "^/api/v1/repos/joinedcontext/helsinki_city-bikes/branches/.*$",
        ))
        .respond_with(wiremock::ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let config = config();
    let state = AppState::new(config.clone(), None)
        .with_mirror(mirror(Some(builder_profile_spec())))
        .with_gitea(forge(&server));
    let run = a_static_run(AgentRunStatus::Previewing);
    state.agents.create_run(&run).await.expect("a run");
    let app = server::app(state.clone());
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{}/publish", run.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert!(
        problem
            .to_string()
            .contains("no version in the application's repository"),
        "{problem}"
    );
    let stored = state
        .agents
        .get_run(&run.id)
        .await
        .expect("store")
        .expect("run");
    assert_eq!(stored.status, "previewing");
}

/// AP-77, T-2603: a run whose branch holds nothing but the README and the build workflow is not
/// published, because the approval would merge it and leave `main` README-only; no merge request
/// is opened and the run does not move.
#[tokio::test]
async fn a_static_run_whose_branch_holds_no_application_is_not_published() {
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(
            "^/api/v1/repos/joinedcontext/helsinki_city-bikes/branches/.*$",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": "readmeonly" } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/joinedcontext/helsinki_city-bikes/git/trees/readmeonly",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "readmeonly",
            "truncated": false,
            "tree": [
                { "path": "README.md", "type": "blob", "sha": "r" },
                { "path": ".gitea/workflows/build.yml", "type": "blob", "sha": "w" },
            ],
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/repos/joinedcontext/helsinki_city-bikes/pulls",
        ))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&server)
        .await;
    let config = config();
    let state = AppState::new(config.clone(), None)
        .with_mirror(mirror(Some(builder_profile_spec())))
        .with_gitea(forge(&server));
    let run = a_static_run(AgentRunStatus::Previewing);
    state.agents.create_run(&run).await.expect("a run");
    let app = server::app(state.clone());
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{}/publish", run.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert!(problem.to_string().contains("README alone"), "{problem}");
    let stored = state
        .agents
        .get_run(&run.id)
        .await
        .expect("store")
        .expect("run");
    assert_eq!(stored.status, "previewing");
}

fn published_app(run: &AgentRun, sha: &str) -> ResourceEnvelope {
    let mut manifest = envelope(
        "App",
        &run.app_name,
        PROJECT,
        json!({
            "kind": "static",
            "source": { "git": { "url": "https://forge.example/joinedcontext/helsinki_city-bikes.git", "ref": sha } },
            "visibility": "project",
            "lifecycle": "published",
        }),
    );
    manifest
        .metadata
        .annotations
        .insert("joinedcontext.com/agent-run".into(), run.id.clone());
    manifest
}

/// AP-77, PF-57: approving the application's Change merges its repository's merge request at the
/// commit the manifest names and marks the run published; a merge the forge refuses leaves the
/// run where it was and says so on it.
#[tokio::test]
async fn approving_the_change_merges_the_applications_repository_at_the_published_commit() {
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let config = config();
    let state = AppState::new(config.clone(), None)
        .with_mirror(mirror(Some(builder_profile_spec())))
        .with_gitea(forge(&server));
    let merged = a_static_run(AgentRunStatus::AwaitingApproval);
    let refused = a_static_run(AgentRunStatus::AwaitingApproval);
    for run in [&merged, &refused] {
        state.agents.create_run(run).await.expect("a run");
    }
    let pull = |number: u64, run: &AgentRun| {
        json!({
            "number": number, "html_url": "h", "state": "open", "title": "t",
            "head": { "ref": run.branch, "sha": "abc" }, "base": { "ref": "main", "sha": "b" }
        })
    };
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/joinedcontext/helsinki_city-bikes/pulls",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([pull(1, &merged), pull(2, &refused)])),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/repos/joinedcontext/helsinki_city-bikes/pulls/1/merge",
        ))
        .and(body_partial_json(
            json!({ "Do": "merge", "head_commit_id": "abc" }),
        ))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/repos/joinedcontext/helsinki_city-bikes/pulls/2/merge",
        ))
        .respond_with(ResponseTemplate::new(409).set_body_string("head out of date"))
        .mount(&server)
        .await;

    joinedcontext_portal::api::agent_runs::merge_published_application(
        &state,
        PROJECT,
        Some(&published_app(&merged, "abc")),
        "approver@hel.fi",
    )
    .await;
    let stored = state
        .agents
        .get_run(&merged.id)
        .await
        .expect("store")
        .expect("run");
    assert_eq!(stored.status, "published");

    joinedcontext_portal::api::agent_runs::merge_published_application(
        &state,
        PROJECT,
        Some(&published_app(&refused, "abc")),
        "approver@hel.fi",
    )
    .await;
    let stored = state
        .agents
        .get_run(&refused.id)
        .await
        .expect("store")
        .expect("run");
    assert_eq!(
        stored.status, "awaiting_approval",
        "a refused merge published nothing"
    );
    let events = state
        .agents
        .events_since(&refused.id, 0)
        .await
        .expect("events");
    assert!(
        events.iter().any(|event| event.kind == "thought"
            && event.payload["text"]
                .as_str()
                .is_some_and(|text| text.contains("did not merge"))),
        "the refusal is said on the run: {events:?}"
    );

    // Another project's Change never reaches this project's run.
    let other = a_static_run(AgentRunStatus::AwaitingApproval);
    state.agents.create_run(&other).await.expect("a run");
    joinedcontext_portal::api::agent_runs::merge_published_application(
        &state,
        "espoo",
        Some(&published_app(&other, "abc")),
        "approver@hel.fi",
    )
    .await;
    let stored = state
        .agents
        .get_run(&other.id)
        .await
        .expect("store")
        .expect("run");
    assert_eq!(stored.status, "awaiting_approval");
}

/// A run that published and waits for approval, its Change recorded as `number`.
async fn waiting_on(state: &AppState, number: i32) -> AgentRun {
    let run = a_static_run(AgentRunStatus::AwaitingApproval);
    state.agents.create_run(&run).await.expect("a run");
    state
        .agents
        .set_merge_request(&run.id, number)
        .await
        .expect("its change");
    run
}

async fn status_of(state: &AppState, run: &AgentRun) -> AgentRun {
    state
        .agents
        .get_run(&run.id)
        .await
        .expect("store")
        .expect("run")
}

/// T-3015: a rejected publish Change ends the run that proposed it, with the reason on its
/// status event; a Change never ends a run of another number, project or state.
#[tokio::test]
async fn a_rejected_change_ends_only_its_own_waiting_run() {
    let state = AppState::new(config(), None);
    let rejected = waiting_on(&state, 7).await;
    let other_change = waiting_on(&state, 8).await;
    let never_published = a_static_run(AgentRunStatus::AwaitingApproval);
    state
        .agents
        .create_run(&never_published)
        .await
        .expect("a run");
    let previewing = a_static_run(AgentRunStatus::Previewing);
    state.agents.create_run(&previewing).await.expect("a run");
    state
        .agents
        .set_merge_request(&previewing.id, 7)
        .await
        .expect("its change");

    use joinedcontext_portal::api::agent_runs::end_rejected_publication;
    let why = "its change chg-00000007 was rejected by jana@hel.fi: not this one";
    assert!(!end_rejected_publication(&state, "espoo", &rejected.id, 7, why).await);
    assert!(!end_rejected_publication(&state, PROJECT, &other_change.id, 7, why).await);
    assert!(!end_rejected_publication(&state, PROJECT, &never_published.id, 7, why).await);
    assert!(!end_rejected_publication(&state, PROJECT, &previewing.id, 7, why).await);
    assert!(!end_rejected_publication(&state, PROJECT, "run-unknown", 7, why).await);
    for run in [&rejected, &other_change, &never_published] {
        assert_eq!(status_of(&state, run).await.status, "awaiting_approval");
    }
    assert_eq!(status_of(&state, &previewing).await.status, "previewing");

    assert!(end_rejected_publication(&state, PROJECT, &rejected.id, 7, why).await);
    let ended = status_of(&state, &rejected).await;
    assert_eq!(ended.status, "cancelled");
    assert_eq!(ended.error.as_deref(), Some(why));
    let events = state
        .agents
        .events_since(&rejected.id, 0)
        .await
        .expect("events");
    assert!(
        events.iter().any(|event| event.kind == "status"
            && event.payload["status"] == "cancelled"
            && event.payload["reason"] == why),
        "the stream says why: {events:?}"
    );
    // Once is enough: a second reject of the same Change finds nothing waiting.
    assert!(!end_rejected_publication(&state, PROJECT, &rejected.id, 7, why).await);
}

/// T-3015: a publish Change closed in the forge without a merge ends its waiting run on the
/// reaper's next pass; an open, a merged, another application's and an unreadable pull request
/// leave theirs waiting.
#[tokio::test]
async fn a_change_closed_on_the_forge_ends_its_waiting_run() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let state = AppState::new(config(), None).with_gitea(forge(&server));
    let closed = waiting_on(&state, 5).await;
    let merged = waiting_on(&state, 6).await;
    let open = waiting_on(&state, 7).await;
    let another_app = waiting_on(&state, 8).await;
    let unreadable = waiting_on(&state, 9).await;
    let pull = |number: u64, state: &str, merged: bool, app: &str| {
        json!({
            "number": number, "html_url": "h", "state": state, "merged": merged, "title": "t",
            "head": { "ref": format!("portal/create-app-{app}-0000000{number}"), "sha": "abc" },
            "base": { "ref": "main", "sha": "b" }
        })
    };
    for (number, body) in [
        (5, pull(5, "closed", false, "city-bikes")),
        (6, pull(6, "closed", true, "city-bikes")),
        (7, pull(7, "open", false, "city-bikes")),
        (8, pull(8, "closed", false, "air-quality")),
    ] {
        Mock::given(method("GET"))
            .and(path(format!(
                "/api/v1/repos/joinedcontext/configuration/pulls/{number}"
            )))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/joinedcontext/configuration/pulls/9"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let ended = joinedcontext_portal::agents::reaper::end_closed_publications(&state).await;
    assert_eq!(ended, 1);
    let run = status_of(&state, &closed).await;
    assert_eq!(run.status, "cancelled");
    assert!(
        run.error
            .as_deref()
            .is_some_and(|why| why.contains("change #5 was closed on the forge")),
        "{:?}",
        run.error
    );
    for run in [&merged, &open, &another_app, &unreadable] {
        assert_eq!(status_of(&state, run).await.status, "awaiting_approval");
    }
    // The next pass has nothing left to end.
    assert_eq!(
        joinedcontext_portal::agents::reaper::end_closed_publications(&state).await,
        0
    );
}

// ---- T-2516: `end_run` under failure (AG-46) ----------------------------------------------

/// What the kube API saw at each workspace delete: the run's ticket hash and whether its
/// ending status event was already published, read from the store at that moment.
#[derive(Clone, Default)]
struct Deletes(Arc<std::sync::Mutex<Vec<(String, bool, String)>>>);

/// A Portal whose runs schedule a workspace on a kube API mock that answers every delete with
/// `delete_status`, recording the run's state at that moment.
async fn ending_world(
    delete_status: u16,
    db: Option<sqlx::PgPool>,
) -> (
    AppState,
    axum::Router,
    String,
    wiremock::MockServer,
    Deletes,
) {
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let api = MockServer::start().await;
    let config = config();
    let (mut state, _, _) = with_state(mirror(Some(builder_profile_spec())), &config);
    if let Some(pool) = db {
        state = state.with_db(pool);
    }
    state.kube = Some(Arc::new(
        joinedcontext_portal::apps::kube::KubeClient::with_token(&api.uri(), "kube-token")
            .expect("a kube client"),
    ));
    let deletes = Deletes::default();
    let (seen, reader) = (deletes.clone(), state.clone());
    Mock::given(method("DELETE"))
        .and(path_regex("/jobs/agent-run-"))
        .respond_with(move |request: &wiremock::Request| {
            let id = request
                .url
                .path()
                .rsplit("agent-run-")
                .next()
                .unwrap_or_default()
                .to_owned();
            let store = reader.clone();
            let run_id = id.clone();
            // The store is read from a runtime of its own: the responder runs on the mock's.
            let (hash, ended) = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("a runtime");
                runtime.block_on(async {
                    let hash = store
                        .agents
                        .get_run(&run_id)
                        .await
                        .ok()
                        .flatten()
                        .map(|run| run.ticket_hash)
                        .unwrap_or_default();
                    let ended = store
                        .agents
                        .events_since(&run_id, 0)
                        .await
                        .unwrap_or_default()
                        .iter()
                        .any(|e| e.kind == "status" && e.payload.to_string().contains("cancelled"));
                    (hash, ended)
                })
            })
            .join()
            .expect("the store read");
            if let Ok(mut log) = seen.0.lock() {
                log.push((
                    hash,
                    ended,
                    String::from_utf8_lossy(&request.body).into_owned(),
                ));
            }
            ResponseTemplate::new(delete_status).set_body_json(json!({ "kind": "Status" }))
        })
        .mount(&api)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(delete_status).set_body_json(json!({})))
        .mount(&api)
        .await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&api)
        .await;
    let app = server::app(state.clone());
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    (state, app, cookie, api, deletes)
}

async fn cancel(app: &axum::Router, cookie: &str, id: &str) -> (StatusCode, Value) {
    call(
        app,
        cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/cancel"),
        None,
    )
    .await
}

async fn ticket_of(state: &AppState, id: &str) -> String {
    state
        .agents
        .get_run(id)
        .await
        .expect("the store")
        .expect("the run")
        .ticket_hash
}

/// The deletes of this run's Job. A reaper in another test of a shared database may end other
/// runs through the same client, so the count is by the run's own Job name.
async fn job_deletes(api: &wiremock::MockServer, id: &str) -> usize {
    let job = format!("/jobs/agent-run-{id}");
    api.received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "DELETE" && r.url.path().ends_with(&job))
        .count()
}

/// T-2516, AG-46: when the pod's delete reaches the kube API, the run's ticket is already
/// dead, so the workspace is refused by the proxy for every moment it still runs.
#[tokio::test]
async fn the_ticket_is_invalidated_before_the_pod_delete_call_is_made() {
    let (state, app, cookie, _api, deletes) = ending_world(200, None).await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    assert!(ticket_of(&state, &id).await.starts_with("$argon2id$"));

    let (status, body) = cancel(&app, &cookie, &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let seen = deletes.0.lock().expect("the log").clone();
    assert_eq!(seen.len(), 1, "one Job delete");
    assert_eq!(
        seen[0].0, "",
        "the ticket was alive when the pod delete was sent"
    );
}

/// T-2516, AG-46: the ending `status` event is published after the pod's delete, never before.
#[tokio::test]
async fn the_status_event_is_published_after_the_pod_is_gone() {
    let (state, app, cookie, _api, deletes) = ending_world(200, None).await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    cancel(&app, &cookie, &id).await;
    let seen = deletes.0.lock().expect("the log").clone();
    assert!(
        !seen[0].1,
        "the status event went out before the pod delete"
    );
    let events = state.agents.events_since(&id, 0).await.expect("events");
    assert!(events
        .iter()
        .any(|e| e.kind == "status" && e.payload.to_string().contains("cancelled")));
}

/// T-2516, AG-46: a pod that is already gone (404) or a kube API that fails (500) still ends
/// the run with its reason, and the ticket stays dead.
#[tokio::test]
async fn a_pod_delete_that_answers_404_still_ends_the_run() {
    for delete in [404, 500] {
        let (state, app, cookie, api, _) = ending_world(delete, None).await;
        let id = create_run(&app, &cookie).await["id"]
            .as_str()
            .expect("an id")
            .to_owned();
        let (status, run) = cancel(&app, &cookie, &id).await;
        assert_eq!(status, StatusCode::OK, "{delete}: {run}");
        assert_eq!(run["status"], json!("cancelled"), "{delete}");
        assert_eq!(
            run["error"],
            json!(format!("cancelled by {STEWARD}")),
            "{delete}: {run}"
        );
        assert_eq!(ticket_of(&state, &id).await, "", "{delete}");
        assert_eq!(job_deletes(&api, &id).await, 1, "{delete}");
    }
}

#[tokio::test]
async fn a_pod_delete_that_answers_500_leaves_the_ticket_dead_and_reports_the_run_ended_with_the_reason(
) {
    let (state, app, cookie, _api, _) = ending_world(500, None).await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let (status, run) = cancel(&app, &cookie, &id).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    assert_eq!(run["error"], json!(format!("cancelled by {STEWARD}")));
    assert!(run["finishedAt"].as_str().is_some(), "{run}");
    assert_eq!(ticket_of(&state, &id).await, "");
    let read = state
        .agents
        .get_run(&id)
        .await
        .expect("store")
        .expect("run");
    assert_eq!(read.status, "cancelled");
}

/// T-2516, AG-46: a Portal that never scheduled the run's pod (no kube API, or no agent
/// settings at all) still kills the ticket when the run ends.
#[tokio::test]
async fn cancelling_a_run_with_no_scheduled_pod_still_invalidates_the_ticket() {
    let config = config();
    let (state, app, _) = with_state(mirror(Some(builder_profile_spec())), &config);
    assert!(state.kube.is_none());
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    assert!(ticket_of(&state, &id).await.starts_with("$argon2id$"));
    let (status, body) = cancel(&app, &cookie, &id).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ticket_of(&state, &id).await, "");
}

#[tokio::test]
async fn no_agent_settings_configured_still_invalidates_the_ticket() {
    // The run is recorded while the runner block is configured; the Portal that ends it has none
    // (its settings were removed and it restarted), and the reaper ends what expired.
    let config = config();
    let (state, app, _) = with_state(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let mut run = state
        .agents
        .get_run(&id)
        .await
        .expect("store")
        .expect("run");
    run.expires_at = "2000-01-01T00:00:00Z".to_owned();

    let bare =
        AppState::new(Config::for_tests(), None).with_mirror(mirror(Some(builder_profile_spec())));
    assert!(bare.config.agent_settings.is_none());
    bare.agents.create_run(&run).await.expect("recorded");
    assert!(ticket_of(&bare, &id).await.starts_with("$argon2id$"));
    let reaped = reaper::reap_expired(&bare).await;
    assert_eq!(reaped, 1);
    assert_eq!(ticket_of(&bare, &id).await, "");
    let read = bare.agents.get_run(&id).await.expect("store").expect("run");
    assert_eq!(read.status, "expired");
}

/// T-2516, AG-46: a second end of an ended run is a conflict that changes nothing: the hash
/// stays empty and nothing more is deleted or published.
#[tokio::test]
async fn ending_an_already_ended_run_is_idempotent_on_the_ticket_hash() {
    let (state, app, cookie, api, _) = ending_world(200, None).await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    assert_eq!(cancel(&app, &cookie, &id).await.0, StatusCode::OK);
    let events = state
        .agents
        .events_since(&id, 0)
        .await
        .expect("events")
        .len();
    for _ in 0..2 {
        assert_eq!(cancel(&app, &cookie, &id).await.0, StatusCode::CONFLICT);
    }
    assert_eq!(ticket_of(&state, &id).await, "");
    assert_eq!(job_deletes(&api, &id).await, 1);
    assert_eq!(
        state
            .agents
            .events_since(&id, 0)
            .await
            .expect("events")
            .len(),
        events
    );
}

/// T-2516: the reason is the run's `error` and nothing else: no delete to the kube API
/// carries it, and none carries the run's ticket either.
#[tokio::test]
async fn the_reason_string_never_reaches_the_pod_spec_or_a_log_with_a_credential() {
    let (state, app, cookie, api, _) = ending_world(200, None).await;
    let created = create_run(&app, &cookie).await;
    let id = created["id"].as_str().expect("an id").to_owned();
    let hash = ticket_of(&state, &id).await;
    cancel(&app, &cookie, &id).await;
    let reason = format!("cancelled by {STEWARD}");
    for request in api.received_requests().await.unwrap_or_default() {
        let body = String::from_utf8_lossy(&request.body);
        if request.method.as_str() == "DELETE" {
            assert!(!body.contains(&reason), "{} {body}", request.url);
        }
        assert!(
            !body.contains(&hash),
            "the ticket hash reached the kube API: {}",
            request.url
        );
    }
}

fn database_url() -> Option<String> {
    std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
}

/// A trigger of this test alone that makes one kind of update of one run fail. The names are
/// built from a run id the Portal minted (hex and dashes), never from input.
async fn refuse_update(pool: &sqlx::PgPool, name: &str, id: &str, when: &str) {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE OR REPLACE FUNCTION {name}() RETURNS trigger AS $$ BEGIN \
         IF NEW.id = '{id}' AND {when} THEN RAISE EXCEPTION 'refused by the test'; END IF; \
         RETURN NEW; END $$ LANGUAGE plpgsql"
    )))
    .execute(pool)
    .await
    .expect("the function");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE TRIGGER {name} BEFORE UPDATE ON agent_runs FOR EACH ROW EXECUTE FUNCTION {name}()"
    )))
    .execute(pool)
    .await
    .expect("the trigger");
}

async fn drop_refusal(pool: &sqlx::PgPool, name: &str) {
    let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP TRIGGER IF EXISTS {name} ON agent_runs"
    )))
    .execute(pool)
    .await;
    let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP FUNCTION IF EXISTS {name}()"
    )))
    .execute(pool)
    .await;
}

/// A run of an application of its own, so tests sharing one database never meet a live run of
/// the same application from another test.
async fn create_run_of_own_app(app: &axum::Router, cookie: &str) -> String {
    let mut body = create_body();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    body["appName"] = json!(format!("ending-{nanos}"));
    let (status, run) = call(
        app,
        cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{run}");
    run["id"].as_str().expect("an id").to_owned()
}

/// T-2516, AG-46: a store that cannot clear the ticket fails the end before the pod is touched,
/// so a pod never outlives a ticket that still verifies without somebody being told.
#[tokio::test]
async fn a_failing_ticket_invalidation_stops_before_the_pod_is_deleted() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let (state, app, cookie, api, _) = ending_world(200, Some(pool.clone())).await;
    let id = create_run_of_own_app(&app, &cookie).await;
    let name = format!("t2516_ticket_{}", id.replace('-', "_"));
    refuse_update(
        &pool,
        &name,
        &id,
        "NEW.ticket_hash = '' AND OLD.ticket_hash <> ''",
    )
    .await;

    let (status, body) = cancel(&app, &cookie, &id).await;
    drop_refusal(&pool, &name).await;
    assert!(status.is_server_error(), "{status}: {body}");
    assert_eq!(job_deletes(&api, &id).await, 0);
    assert!(ticket_of(&state, &id).await.starts_with("$argon2id$"));
}

/// T-2516, restated by T-2560 (AG-46): a status that cannot be written still leaves the ticket
/// dead and the pod gone, the caller is told, and the retry moves the status.
#[tokio::test]
async fn a_failing_set_status_still_leaves_the_ticket_dead_and_the_pod_gone() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let (state, app, cookie, api, _) = ending_world(200, Some(pool.clone())).await;
    let id = create_run_of_own_app(&app, &cookie).await;
    let name = format!("t2516_status_{}", id.replace('-', "_"));
    refuse_update(&pool, &name, &id, "NEW.status = 'cancelled'").await;

    let (status, body) = cancel(&app, &cookie, &id).await;
    drop_refusal(&pool, &name).await;
    assert!(status.is_server_error(), "{status}: {body}");
    assert_eq!(ticket_of(&state, &id).await, "");
    assert_eq!(job_deletes(&api, &id).await, 1);
    let run = state
        .agents
        .get_run(&id)
        .await
        .expect("store")
        .expect("run");
    assert_ne!(run.status, "cancelled");
    // The retry the caller was told to make ends it.
    let retry = cancel(&app, &cookie, &id).await;
    assert_eq!(retry.0, StatusCode::OK, "{}", retry.1);
    assert_eq!(retry.1["status"], json!("cancelled"));
}

/// T-2560, AG-46: an end that failed after the ticket could not be cleared is finished by the
/// next attempt: the ticket dies and the pod goes.
#[tokio::test]
async fn an_end_that_failed_after_the_status_is_finished_by_a_retry() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let (state, app, cookie, api, _) = ending_world(200, Some(pool.clone())).await;
    let id = create_run_of_own_app(&app, &cookie).await;
    let name = format!("t2560_{}", id.replace('-', "_"));
    refuse_update(
        &pool,
        &name,
        &id,
        "NEW.ticket_hash = '' AND OLD.ticket_hash <> ''",
    )
    .await;
    let first = cancel(&app, &cookie, &id).await;
    drop_refusal(&pool, &name).await;
    assert!(first.0.is_server_error(), "{}", first.1);

    cancel(&app, &cookie, &id).await;
    reaper::reap_expired(&state).await;
    assert_eq!(
        ticket_of(&state, &id).await,
        "",
        "the ticket still verifies"
    );
    assert_eq!(job_deletes(&api, &id).await, 1, "the pod was never deleted");
}

/// Edge cases of the filtered run list against PostgreSQL (T-2502, MF-12, PF-59). They run when
/// `JC_PORTAL_TEST_DATABASE_URL` names a database the test may write to, and skip otherwise, like
/// the other database tests. Each test writes into projects of its own.
mod filtered_list_edges {
    use joinedcontext_portal::agents::run::AgentRun;
    use joinedcontext_portal::agents::store::RunFilter;
    use joinedcontext_portal::db;
    use serde_json::json;
    use sqlx::PgPool;

    async fn pool() -> Option<PgPool> {
        let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL").ok()?;
        if url.trim().is_empty() {
            return None;
        }
        Some(db::connect(&url).await.expect("connect and migrate"))
    }

    /// A project name no other run of any test has used.
    fn fresh(tag: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        format!("t2502-{tag}-{nanos}")
    }

    /// `(id, app, kind, status, created_by, created_at)`
    type Row<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str, &'a str);

    async fn insert(pool: &PgPool, project: &str, rows: &[Row<'_>]) {
        for (id, app, kind, status, created_by, created_at) in rows {
            let run: AgentRun = serde_json::from_value(json!({
                "id": format!("{project}-{id}"), "project": project, "appName": app,
                "endpointName": "air", "endpointSlug": "air", "profile": "builder",
                "kind": kind, "unattended": false, "appClass": "dashboard",
                "visibility": "internal", "prompt": "p", "promptDigest": "d",
                "dataNeeds": [], "allowsWrite": false, "branch": "b", "pathPrefix": "apps/x",
                "status": status, "ticketHash": "h", "steps": 0, "tokensUsed": 0,
                "createdBy": created_by, "createdAt": created_at,
                "expiresAt": "2099-01-01T00:00:00Z",
            }))
            .expect("a run");
            db::insert_agent_run(pool, &run).await.expect("insert");
        }
    }

    async fn ids(pool: &PgPool, project: &str, filter: RunFilter, limit: i64) -> Vec<String> {
        db::list_agent_runs_filtered(pool, project, &filter, limit)
            .await
            .expect("the list")
            .into_iter()
            .map(|run| run.id.trim_start_matches(&format!("{project}-")).to_owned())
            .collect()
    }

    fn filter(
        app: Option<&str>,
        kind: Option<&str>,
        status: Option<&str>,
        by: Option<&str>,
    ) -> RunFilter {
        RunFilter {
            app: app.map(str::to_owned),
            kind: kind.map(str::to_owned),
            status: status.map(str::to_owned),
            created_by: by.map(str::to_owned),
            origin: None,
        }
    }

    /// Four runs that differ in one field each from `a`, oldest first.
    const ROWS: [Row<'static>; 5] = [
        ("a", "kpi", "app", "running", "anna", "2026-09-21T10:00:00Z"),
        ("b", "map", "app", "running", "anna", "2026-09-21T10:01:00Z"),
        (
            "c",
            "kpi",
            "edit",
            "running",
            "anna",
            "2026-09-21T10:02:00Z",
        ),
        ("d", "kpi", "app", "failed", "anna", "2026-09-21T10:03:00Z"),
        (
            "e",
            "kpi",
            "app",
            "running",
            "boris",
            "2026-09-21T10:04:00Z",
        ),
    ];

    /// Cases 2 to 5 and 11, table-driven: every combination of the four filters binds each value
    /// to its own column, and no filter at all lists every run once, newest first.
    #[tokio::test]
    async fn every_filter_alone_and_together_binds_its_own_column() {
        let Some(pool) = pool().await else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let project = fresh("filters");
        insert(&pool, &project, &ROWS).await;
        let cases: [(RunFilter, &[&str]); 7] = [
            (filter(None, None, None, None), &["e", "d", "c", "b", "a"]),
            (filter(Some("map"), None, None, None), &["b"]),
            (filter(None, Some("edit"), None, None), &["c"]),
            (filter(None, None, Some("failed"), None), &["d"]),
            (filter(None, None, None, Some("boris")), &["e"]),
            (
                filter(Some("kpi"), Some("app"), Some("running"), Some("anna")),
                &["a"],
            ),
            // A value in the wrong column matches nothing: `kpi` is an app, not a kind.
            (filter(None, Some("kpi"), None, None), &[]),
        ];
        for (filter, expected) in cases {
            let said = format!("{filter:?}");
            assert_eq!(ids(&pool, &project, filter, 50).await, expected, "{said}");
        }
    }

    /// AG-93: `origin` is written and read back, a run recorded before the column is a person's,
    /// and the origin filter binds after the others without moving the `LIMIT` placeholder.
    #[tokio::test]
    async fn the_origin_is_stored_and_filters_after_the_other_columns() {
        let Some(pool) = pool().await else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let project = fresh("origin");
        insert(&pool, &project, &ROWS[..2]).await;
        let mut journey: AgentRun = db::load_agent_run(&pool, &format!("{project}-a"))
            .await
            .expect("a read")
            .expect("the run");
        assert_eq!(journey.origin, "person", "the column's default");
        journey.id = format!("{project}-j");
        journey.origin = "journey".to_owned();
        journey.created_at = "2026-09-21T10:09:00Z".to_owned();
        db::insert_agent_run(&pool, &journey).await.expect("insert");
        let by_origin = |origin: Option<&str>| RunFilter {
            app: Some("kpi".to_owned()),
            created_by: Some("anna".to_owned()),
            origin: origin.map(str::to_owned),
            ..RunFilter::default()
        };
        assert_eq!(
            ids(&pool, &project, by_origin(Some("person")), 50).await,
            ["a"]
        );
        assert_eq!(
            ids(&pool, &project, by_origin(Some("journey")), 1).await,
            ["j"]
        );
        assert_eq!(ids(&pool, &project, by_origin(None), 50).await, ["j", "a"]);
        assert!(
            ids(&pool, &project, by_origin(Some("journey' OR '1'='1")), 50)
                .await
                .is_empty()
        );
    }

    /// Case 3: two filters before it do not move the `LIMIT` placeholder onto a filter value.
    #[tokio::test]
    async fn app_and_status_together_do_not_shift_the_limit_placeholder() {
        let Some(pool) = pool().await else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let project = fresh("limit");
        insert(&pool, &project, &ROWS).await;
        let page = ids(
            &pool,
            &project,
            filter(Some("kpi"), None, Some("running"), None),
            2,
        )
        .await;
        assert_eq!(page, ["e", "c"]);
    }

    /// Case 1 and 8: a run of another project never reaches this one's page, whatever the
    /// filters, and a project with no runs is an empty page, not an error.
    #[tokio::test]
    async fn two_projects_never_leak_into_each_others_filtered_page() {
        let Some(pool) = pool().await else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let (mine, theirs) = (fresh("mine"), fresh("theirs"));
        insert(&pool, &mine, &ROWS[..1]).await;
        insert(&pool, &theirs, &ROWS).await;
        for filter in [
            filter(None, None, None, None),
            filter(Some("map"), None, None, None),
            filter(None, None, None, Some("boris")),
        ] {
            let page = db::list_agent_runs_filtered(&pool, &mine, &filter, 50)
                .await
                .expect("the list");
            assert!(page.iter().all(|run| run.project == mine), "{filter:?}");
        }
        let nobody = fresh("nobody");
        let all = filter(Some("kpi"), Some("app"), Some("running"), Some("anna"));
        assert!(ids(&pool, &nobody, all, 50).await.is_empty());
    }

    /// Cases 6 and 7: a value is bound, never spliced into the SQL, so `%`, `_` and a quote are
    /// the characters they are.
    #[tokio::test]
    async fn a_filter_value_is_bound_so_wildcards_and_quotes_are_literal() {
        let Some(pool) = pool().await else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let project = fresh("literal");
        insert(&pool, &project, &ROWS).await;
        for value in [
            "%",
            "kp_",
            "k%",
            "kpi' OR '1'='1",
            "kpi\"; DROP TABLE agent_runs; --",
        ] {
            for filter in [
                filter(Some(value), None, None, None),
                filter(None, None, None, Some(value)),
            ] {
                let said = format!("{filter:?}");
                assert!(ids(&pool, &project, filter, 50).await.is_empty(), "{said}");
            }
        }
        assert_eq!(
            ids(&pool, &project, filter(None, None, None, None), 50)
                .await
                .len(),
            5
        );
    }

    /// Cases 9 and 10: `LIMIT 0` is an empty page, and a limit above the rows is every row once.
    /// The route refuses a limit below one before it reaches here.
    #[tokio::test]
    async fn the_limit_bounds_the_page_at_both_ends() {
        let Some(pool) = pool().await else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let project = fresh("bounds");
        insert(&pool, &project, &ROWS).await;
        let none = filter(None, None, None, None);
        assert!(ids(&pool, &project, none.clone(), 0).await.is_empty());
        assert_eq!(ids(&pool, &project, none.clone(), 5).await.len(), 5);
        assert_eq!(ids(&pool, &project, none.clone(), 6).await.len(), 5);
        assert_eq!(ids(&pool, &project, none, 4).await, ["e", "d", "c", "b"]);
    }

    /// Runs created in the same instant come in the order the memory store gives them (by id),
    /// so a page is the same between two calls whichever store serves it.
    #[tokio::test]
    async fn runs_created_at_one_instant_come_in_id_order() {
        let Some(pool) = pool().await else {
            eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
            return;
        };
        let project = fresh("ties");
        let at = "2026-09-21T10:00:00Z";
        insert(
            &pool,
            &project,
            &[
                ("c", "kpi", "app", "running", "anna", at),
                ("a", "kpi", "app", "running", "anna", at),
                ("b", "kpi", "app", "running", "anna", at),
            ],
        )
        .await;
        let none = filter(None, None, None, None);
        assert_eq!(
            ids(&pool, &project, none.clone(), 50).await,
            ["a", "b", "c"]
        );
        assert_eq!(ids(&pool, &project, none, 2).await, ["a", "b"]);
    }
}

/// CC-87, AG-86: in layout 2 the proxy is told the run's one repository, the project's own, and
/// the run's folder as that repository spells it; before the project is registered it is the
/// configuration repository's folder and no repository is named.
#[tokio::test]
async fn a_workspace_run_reaches_its_project_repository_alone() {
    let config = config();
    let (state, app, internal) = with_state(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let (status, created) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(create_body()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let id = created["id"].as_str().expect("an id").to_owned();
    let context = |internal: axum::Router| {
        let id = id.clone();
        async move {
            internal_call(
                &internal,
                Some(proxy_bearer()),
                Method::GET,
                &format!("/internal/agent-runs/{id}"),
                None,
            )
            .await
        }
    };

    let (status, before) = context(internal.clone()).await;
    assert_eq!(status, StatusCode::OK, "{before}");
    assert!(before.get("repository").is_none(), "{before}");
    assert_eq!(
        before["pathPrefix"],
        json!("projects/helsinki/apps/city-bikes-overview/")
    );

    state
        .mirror
        .set_repositories(std::collections::BTreeMap::from([(
            PROJECT.to_owned(),
            "helsinki".to_owned(),
        )]));
    let (status, after) = context(internal).await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(after["repository"], json!("helsinki"));
    assert_eq!(after["pathPrefix"], json!("apps/city-bikes-overview/"));
}

/// A run as the live journeys start it: the browser session plus `X-JC-Run-Origin` (AG-93).
async fn call_as_journey(
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
        .header("x-csrf-token", CSRF)
        .header("x-jc-run-origin", "journey");
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

fn ids(list: &Value) -> Vec<String> {
    list["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|run| run["id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

// T-2816, AG-93: the journeys' test runs crowded the history people read; they are marked and
// left out of a list unless it asks, and nothing is deleted.
#[tokio::test]
async fn a_journey_s_run_is_marked_and_a_list_leaves_it_out_unless_asked() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
    let runs = format!("/api/v1/projects/{PROJECT}/agent-runs");

    let person = create_run(&app, &cookie).await;
    assert_eq!(person["origin"], "person");
    // Another application: one live run per application (AP-68).
    let mut journey_body = create_body();
    journey_body["appName"] = json!("t2816-journey-bikes");
    let (status, journey) =
        call_as_journey(&app, &cookie, Method::POST, &runs, Some(journey_body)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{journey}");
    assert_eq!(journey["origin"], "journey");
    let (person_id, journey_id) = (
        person["id"].as_str().unwrap_or_default(),
        journey["id"].as_str().unwrap_or_default(),
    );

    let (status, listed) = call(&app, &cookie, Method::GET, &runs, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(
        ids(&listed),
        vec![person_id.to_owned()],
        "people's runs by default"
    );

    let (_, all) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("{runs}?origin=all"),
        None,
    )
    .await;
    let mut both = ids(&all);
    both.sort();
    let mut expected = vec![person_id.to_owned(), journey_id.to_owned()];
    expected.sort();
    assert_eq!(
        both, expected,
        "asked for, every run is there: nothing was deleted"
    );

    let (_, only) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("{runs}?origin=journey"),
        None,
    )
    .await;
    assert_eq!(ids(&only), vec![journey_id.to_owned()]);

    // The journey itself finds the run it started.
    let (_, own) = call_as_journey(&app, &cookie, Method::GET, &runs, None).await;
    assert_eq!(ids(&own).len(), 2);

    let (status, refused) = call(
        &app,
        &cookie,
        Method::GET,
        &format!("{runs}?origin=tests"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
}

// T-2816, AG-93: only the journeys' own sessions carry the mark. Anyone else, an administrator
// included, would hide their activity from the default lists with it, so they are refused.
#[tokio::test]
async fn a_person_who_is_not_a_journey_cannot_mark_a_run() {
    let config = config();
    let app = router(mirror(Some(builder_profile_spec())), &config);
    let admin = session_cookie(&config, "demo.approver", &["portal-approver"]);
    let runs = format!("/api/v1/projects/{PROJECT}/agent-runs");

    let (status, refused) =
        call_as_journey(&app, &admin, Method::POST, &runs, Some(create_body())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert!(
        refused["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("live journeys")),
        "{refused}"
    );
    let (status, refused) = call_as_journey(&app, &admin, Method::GET, &runs, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");

    // Nothing was started by the refused request.
    let (status, listed) = call(
        &app,
        &admin,
        Method::GET,
        &format!("{runs}?origin=all"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(ids(&listed).is_empty(), "{listed}");
}

/// AP-132, PF-70: a run's data needs are read against the caller's own `/access` document, which
/// the context gateway answers for the caller's token; a need wider than it is refused, naming the
/// operation, and a session with no token asks for no write.
mod held {
    use super::*;
    use wiremock::matchers::{header as has_header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn gateway(actions: &[&str]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/api/endpoint/{SLUG}/access")))
            .and(has_header(
                "authorization",
                format!("Bearer {}", token()).as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "permissions": [{
                    "resource": { "type": "BikeHireDockingStation" },
                    "actions": actions,
                    "attributes": "*"
                }],
                "prohibitions": []
            })))
            .mount(&server)
            .await;
        server
    }

    /// One token for the whole module: an ES256 signature differs every time it is made.
    fn token() -> String {
        static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        TOKEN
            .get_or_init(|| {
                common::REALM.person_token_of(
                    "portal-api",
                    "portal-api",
                    STEWARD,
                    &["portal-approver"],
                )
            })
            .clone()
    }

    fn config_with(gateway: &MockServer) -> Config {
        let uri = gateway.uri();
        Config::from_vars(|key| match key {
            "JC_PORTAL_GATEWAY_URL" => Some(uri.clone()),
            "JC_AGENTS_NAMESPACE" => Some("agents".into()),
            "JC_AGENT_PROXY_BASE" => {
                Some("http://jc-agent-proxy.agents.svc.cluster.local:8080".into())
            }
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.clone()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api".into()),
            "JC_OIDC_CLIENT_SECRET" => Some("secret".into()),
            "JC_PORTAL_AGENT_PROXY_CLIENT_ID" => Some(common::AGENT_PROXY_CLIENT.into()),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver".into()),
            _ => None,
        })
        .expect("a Portal with a gateway")
    }

    async fn portal(config: &Config) -> axum::Router {
        let state =
            AppState::new(config.clone(), None).with_mirror(mirror(Some(builder_profile_spec())));
        state
            .bearer
            .as_ref()
            .expect("a realm")
            .refresh()
            .await
            .expect("jwks");
        server::app(state)
    }

    async fn create_as_bearer(app: &axum::Router, body: Value) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/projects/{PROJECT}/agent-runs"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", token()))
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

    fn writing() -> Value {
        let mut body = create_body();
        body["dataNeeds"][0]["operations"] =
            json!(["queryEntity", "retrieveEntity", "updateAttrs"]);
        body
    }

    #[tokio::test]
    async fn a_need_within_the_callers_grant_starts_and_a_wider_one_is_named() {
        let reader = gateway(&["queryEntity", "retrieveEntity"]).await;
        let app = portal(&config_with(&reader)).await;

        let (status, body) = create_as_bearer(&app, create_body()).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");

        let (status, body) = create_as_bearer(&app, writing()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let errors = body["errors"].to_string();
        assert!(
            errors.contains("updateAttrs BikeHireDockingStation on endpoint 'helsinki-bikes'"),
            "{body}"
        );

        let editor = gateway(&["queryEntity", "retrieveEntity", "updateAttrs"]).await;
        let app = portal(&config_with(&editor)).await;
        let (status, body) = create_as_bearer(&app, writing()).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        assert_eq!(body["allowsWrite"], json!(true));
    }

    #[tokio::test]
    async fn a_cookie_session_asks_for_no_write_and_a_silent_gateway_is_503() {
        let reader = gateway(&["queryEntity", "retrieveEntity"]).await;
        let config = config_with(&reader);
        let app = portal(&config).await;
        let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);
        let (status, body) = call(
            &app,
            &cookie,
            Method::POST,
            &format!("/api/v1/projects/{PROJECT}/agent-runs"),
            Some(writing()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body["errors"].to_string().contains("no token"), "{body}");

        let silent = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&silent)
            .await;
        let app = portal(&config_with(&silent)).await;
        let (status, body) = create_as_bearer(&app, create_body()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    }
}

/// F-POR1-018: two creates of one application's run at once on Postgres: the store's unique
/// index lets one start, the other is 409.
#[tokio::test]
async fn two_concurrent_creates_of_one_apps_run_start_one() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let (_state, app, cookie, _api, _) = ending_world(200, Some(pool)).await;
    let mut body = create_body();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    body["appName"] = json!(format!("twice-{nanos}"));
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs");
    let (one, two) = tokio::join!(
        call(&app, &cookie, Method::POST, &uri, Some(body.clone())),
        call(&app, &cookie, Method::POST, &uri, Some(body.clone()))
    );
    let mut statuses = [one.0, two.0];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::ACCEPTED, StatusCode::CONFLICT],
        "{} / {}",
        one.1,
        two.1
    );
}

/// JC1-24, AG-70: a continuation that names no endpoints keeps the ones of the run it continues,
/// held to the profile it starts with like named ones: a profile that does not list the inherited
/// endpoint refuses the continuation instead of reading it.
#[tokio::test]
async fn a_continuation_holds_inherited_endpoints_to_its_profile() {
    let config = config();
    let mirror = mirror(Some(builder_profile_spec()));
    let mut narrow = builder_profile_spec();
    narrow["access"] = json!({
        "operations": ["jc_catalog_search"],
        "endpoints": [{ "name": "another-endpoint", "verbs": ["read"] }]
    });
    mirror.upsert(envelope("AgentProfile", "narrow", "org", narrow));
    let (state, app, _) = with_state(mirror, &config);
    let cookie = session_cookie(&config, STEWARD, &["portal-approver"]);

    let parent_id = mint_run_id();
    let (_, ticket_hash) = mint_ticket();
    let parent = AgentRun {
        id: parent_id.clone(),
        project: PROJECT.to_owned(),
        app_name: "".to_owned(),
        title: None,
        endpoint_name: "helsinki-bikes".to_owned(),
        endpoint_slug: SLUG.to_owned(),
        endpoints: json!([{ "name": "helsinki-bikes", "slug": SLUG, "space": "helsinki" }]),
        profile: "app-builder".to_owned(),
        kind: "conversation".to_owned(),
        unattended: false,
        continues: None,
        app_class: "ui".to_owned(),
        visibility: "private".to_owned(),
        prompt: "Earlier conversation".to_owned(),
        prompt_digest: digest_prompt("Earlier conversation"),
        data_needs: json!([]),
        allows_write: false,
        branch: "".to_owned(),
        path_prefix: "".to_owned(),
        status: "succeeded".to_owned(),
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
        created_by: STEWARD.to_owned(),
        origin: "person".to_owned(),
        starter: serde_json::Value::Null,
        created_at: "2026-09-12T08:00:00Z".to_owned(),
        started_at: None,
        finished_at: Some("2026-09-12T08:05:00Z".to_owned()),
        expires_at: "2026-09-12T08:30:00Z".to_owned(),
        error: None,
    };
    state.agents.create_run(&parent).await.expect("create run");

    let (status, problem) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({
            "message": "Continue please",
            "continues": parent_id,
            "profile": "narrow"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{problem}");
    assert!(problem.to_string().contains("helsinki-bikes"), "{problem}");
}
