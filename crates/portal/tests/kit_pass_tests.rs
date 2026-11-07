//! The builder's runs through the real router (T-0571, T-0680, AP-56…AP-60, AG-53, AG-54, UI-41).
//!
//! Every run that builds writes code on the App SDK: an application (T-0680), a dashboard
//! (T-3159) and an analysis (T-3160). The spec.json kit this file was named for is retired
//! (T-0681). The proxy is a wiremock that answers what the driver asks. The conversation cases
//! are the assistant's own path.
//!
//! The application cases drive `src/agents/oneshot/code_pass.rs` end to end (T-1662): the first
//! version, a refused path, a repair, a second failure, an answer without blocks, an answer cut at
//! the budget, the verification passes and their limit, and the tool loop after the first version.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{header_regex, method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The LinkML source of the `bikes` model, served by the forge (DM-56).
const BIKES_LINKML: &str = "id: https://hel.fi/models/bikes\nname: bikes\nenums:\n  StationStatus:\n    permissible_values:\n      working: {}\n      closed: {}\nclasses:\n  BikeHireDockingStation:\n    slots: [id, name, location, availableBikeNumber, status, stewardNote]\nslots:\n  id: {}\n  name: { range: string, required: true }\n  location: { range: string }\n  availableBikeNumber: { range: integer, minimum_value: 0 }\n  status: { range: StationStatus }\n  stewardNote: { range: string }\n";

const CSRF: &str = "csrf-token-value";
const PROJECT: &str = "helsinki";
const STEWARD: &str = "demo.steward";
const SLUG: &str = "si6epqkx364lprho5uaigutk274r5grb";

fn answer(prose: &str, blocks: &[(&str, &str, &str)]) -> String {
    let mut text = format!("{prose}\n\n```text\n");
    for (path, search, replace) in blocks {
        text.push_str(&format!(
            "{path}\n<<<<<<< SEARCH\n{search}=======\n{replace}\n>>>>>>> REPLACE\n"
        ));
    }
    text.push_str("```\n");
    text
}

fn anthropic(text: &str) -> Value {
    json!({
        "id": "msg_1", "type": "message", "role": "assistant",
        "content": [{ "type": "text", "text": text }],
        "usage": { "input_tokens": 1200, "output_tokens": 300 }
    })
}

fn openai(text: &str) -> Value {
    json!({
        "id": "chatcmpl-1", "object": "chat.completion",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": text }, "finish_reason": "stop" }],
        "usage": { "total_tokens": 1500 }
    })
}

/// An answer the provider cut at the output budget.
fn openai_cut(text: &str) -> Value {
    let mut body = openai(text);
    body["choices"][0]["finish_reason"] = json!("length");
    body
}

fn config(proxy_base: &str) -> Config {
    // Model Tools answers on the proxy's mock too, under its own prefix (SDK-10).
    let model_tools = format!("{proxy_base}/model-tools");
    Config::from_vars(|key| {
        match key {
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some(proxy_base),
            "JC_PORTAL_MODEL_TOOLS_URL" => Some(model_tools.as_str()),
            "JC_AGENT_PROXY_TOKEN" => Some("the-token-only-jc-agent-proxy-has"),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            "JC_PORTAL_PUBLIC_URL" => Some("https://portal.example.com"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("the agent runner block is complete")
}

fn session_cookie(config: &Config, username: &str) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{username}"),
            username: username.into(),
            email: Some(format!("{username}@hel.fi")),
            name: None,
            roles: vec!["portal-approver".into()],
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

fn mirror(provider: &str) -> Arc<Mirror> {
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
    mirror.upsert(envelope(
        "Organization",
        "hel",
        "org",
        json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en" }),
    ));
    mirror.upsert(envelope(
        "ContextSpace",
        "helsinki",
        PROJECT,
        json!({ "dataModelRef": { "kind": "DataModel", "name": "bikes" } }),
    ));
    mirror.upsert(envelope(
        "DataModel",
        "bikes",
        PROJECT,
        json!({
            "version": "1.0.0",
            "contextSpaceRef": "helsinki",
            "linkml": "./bikes.linkml.yaml",
            "classes": ["BikeHireDockingStation"]
        }),
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
            "model": { "provider": provider, "name": "claude-sonnet-5", "maxTokensPerRun": 400000, "reasoningEffort": "medium" },
            "limits": { "stepsPerRun": 120, "wallClock": "PT20M", "concurrentRunsPerOrganization": 2, "requestsPerMinute": 60, "maxResponseBytes": 2097152 },
            "egress": { "allowedHosts": [] },
            // A builder that shares what it built proposes an Endpoint, so the profile grants
            // that operation (AG-70: what the block does not name, the agent does not get).
            // Nothing approving is here, and nothing can be: an agent never approves (PF-58).
            "access": {
                "operations": [
                    "jc_catalog_search",
                    "jc_kpi_compute",
                    "jc_space_complete",
                    "jc_endpoint_propose",
                    "jc_resource_propose"
                ],
                "kinds": [
                    { "kind": "Endpoint", "verbs": ["read", "propose"] },
                    { "kind": "ContextSpace", "verbs": ["read", "propose"] },
                    { "kind": "Pipeline", "verbs": ["read", "propose"] }
                ]
            },
            "tools": ["shell"],
            "workspace": { "cpu": "1", "memory": "2Gi", "ephemeralStorage": "4Gi" }
        }),
    ));
    mirror
}

/// A Portal and its stub proxy. The proxy answers the sample read with five stations and the
/// model route with the canned answers, in order.
async fn portal(provider: &str, answers: &[String]) -> (axum::Router, String, MockServer) {
    let (_, app, cookie, proxy) = portal_state(provider, answers).await;
    (app, cookie, proxy)
}

async fn portal_state(
    provider: &str,
    answers: &[String],
) -> (AppState, axum::Router, String, MockServer) {
    portal_state_with(provider, answers, None).await
}

/// A forge that has no branch and no file for this run yet, and takes the commit; it serves
/// the `bikes` model's LinkML source, the file the field schema is read from (DM-56).
async fn forge() -> MockServer {
    let forge = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/org/manifests/contents/projects/helsinki/spaces/helsinki/datamodels/bikes.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-bikes-linkml",
            "content": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, BIKES_LINKML)
        })))
        .with_priority(1)
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/org/manifests"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(
            "^/api/v1/repos/org/manifests/(branches|contents)/",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&forge)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/org/manifests/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&forge)
        .await;
    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/org/manifests/contents/projects/helsinki/apps/city-bikes-overview/src/spec.json",
        ))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "abc123def456" } })),
        )
        .mount(&forge)
        .await;
    forge
}

async fn portal_state_with(
    provider: &str,
    answers: &[String],
    forge: Option<&MockServer>,
) -> (AppState, axum::Router, String, MockServer) {
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/data/ngsi-ld/v1/entities"))
        .and(query_param("type", "BikeHireDockingStation"))
        .and(query_param("limit", "5"))
        .and(header_regex("authorization", "^Bearer jcr_[0-9a-f-]+\\.[A-Za-z0-9_-]+$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "urn:ngsi-ld:BikeHireDockingStation:001", "type": "BikeHireDockingStation", "name": "Kaivopuisto", "availableBikeNumber": 7, "location": { "type": "Point", "coordinates": [24.95, 60.155] } },
            { "id": "urn:ngsi-ld:BikeHireDockingStation:002", "type": "BikeHireDockingStation", "name": "Laivasillankatu", "availableBikeNumber": 2, "location": { "type": "Point", "coordinates": [24.956, 60.161] } },
            { "id": "urn:ngsi-ld:BikeHireDockingStation:003", "type": "BikeHireDockingStation", "name": "Kapteeninpuistikko", "availableBikeNumber": 0, "location": { "type": "Point", "coordinates": [24.945, 60.158] } },
            { "id": "urn:ngsi-ld:BikeHireDockingStation:004", "type": "BikeHireDockingStation", "name": "Viiskulma", "availableBikeNumber": 11, "location": { "type": "Point", "coordinates": [24.941, 60.161] } },
            { "id": "urn:ngsi-ld:BikeHireDockingStation:005", "type": "BikeHireDockingStation", "name": "Sepänkatu", "availableBikeNumber": 4, "location": { "type": "Point", "coordinates": [24.937, 60.159] } }
        ])))
        .mount(&proxy)
        .await;
    // The rows of the preview: every entities read that is not the five-entity sample.
    Mock::given(method("GET"))
        .and(path("/v1/data/ngsi-ld/v1/entities"))
        .and(query_param("options", "keyValues"))
        .and(header_regex("authorization", "^Bearer jcr_"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "urn:ngsi-ld:BikeHireDockingStation:001", "type": "BikeHireDockingStation", "name": "Kaivopuisto", "availableBikeNumber": 7 },
            { "id": "urn:ngsi-ld:BikeHireDockingStation:002", "type": "BikeHireDockingStation", "name": "Laivasillankatu", "availableBikeNumber": 2 }
        ])))
        .mount(&proxy)
        .await;
    let route = if provider == "anthropic" {
        "/v1/llm/messages"
    } else {
        "/v1/llm/chat/completions"
    };
    // A conversation that starts without a path is routed first (AG-88, T-2693): to no path
    // here, so the answers below are the conversation's own turns.
    let unrouted = "{\"path\": null, \"reason\": \"a script\"}";
    Mock::given(method("POST"))
        .and(path(route))
        .and(wiremock::matchers::body_string_contains(
            "route a person's message to one of the assistant's paths",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(if provider == "anthropic" {
                anthropic(unrouted)
            } else {
                openai(unrouted)
            }),
        )
        .with_priority(1)
        .mount(&proxy)
        .await;
    for text in answers {
        let body = if provider == "anthropic" {
            anthropic(text)
        } else {
            openai(text)
        };
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .up_to_n_times(1)
            .mount(&proxy)
            .await;
    }
    let config = config(&proxy.uri());
    let mut state = AppState::new(config.clone(), None).with_mirror(mirror(provider));
    if let Some(forge) = forge {
        let client = GiteaClient::new(forge.uri().parse().expect("url"), "org", "manifests", "t")
            .expect("a forge client");
        state = state.with_gitea(Arc::new(client));
    }
    let app = server::app(state.clone());
    let cookie = session_cookie(&config, STEWARD);
    (state, app, cookie, proxy)
}

async fn call(
    app: &axum::Router,
    cookie: &str,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
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
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (status, headers, bytes.to_vec())
}

async fn json(
    app: &axum::Router,
    cookie: &str,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, _, bytes) = call(app, cookie, method, uri, body).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn create_run(app: &axum::Router, cookie: &str) -> Value {
    create_run_with(
        app,
        cookie,
        "Create a live bike availability dashboard with station filtering",
        &["queryEntity"],
    )
    .await
}

async fn create_run_with(
    app: &axum::Router,
    cookie: &str,
    prompt: &str,
    operations: &[&str],
) -> Value {
    let (status, body) = json(
        app,
        cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(json!({
            "appName": "city-bikes-overview",
            "endpointName": "helsinki-bikes",
            "appClass": "ui",
            // An analysis is the run kind still on the kit (T-3159 moved dashboards, T-3160
            // moves analyses, T-0681 then removes these tests with the kit).
            "kind": "analysis",
            "visibility": "project",
            "prompt": prompt,
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                "types": ["BikeHireDockingStation"],
                "attrs": ["name", "location", "availableBikeNumber"],
                "operations": operations
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    body
}

/// The run as read back once its status is one of `statuses`, or a panic after ten seconds
/// with the run as it stands.
async fn wait_for(app: &axum::Router, cookie: &str, id: &str, statuses: &[&str]) -> Value {
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}");
    let mut last = Value::Null;
    for _ in 0..200 {
        let (status, run) = json(app, cookie, Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{run}");
        if statuses.iter().any(|s| run["status"] == json!(s)) {
            return run;
        }
        last = run;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the run never reached {statuses:?}: {last}");
}

/// Every recorded event of a run, in order, read from the store rather than the live stream:
/// what a browser that reconnects after the pass is shown.
async fn events(app: &axum::Router, cookie: &str, id: &str) -> Vec<(String, Value)> {
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/events");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(&uri)
                .header(header::COOKIE, cookie)
                .header(header::ACCEPT, "text/event-stream")
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let mut text = String::new();
    // The replay is one frame per event; the stream then waits for the next live one, which
    // is the timeout below.
    while let Ok(Some(Ok(frame))) =
        tokio::time::timeout(Duration::from_millis(300), body.frame()).await
    {
        if let Some(chunk) = frame.data_ref() {
            text.push_str(&String::from_utf8_lossy(chunk));
        }
    }
    text.split("\n\n")
        .filter_map(|frame| {
            let kind = frame
                .lines()
                .find_map(|line| line.strip_prefix("event: "))?;
            let data = frame.lines().find_map(|line| line.strip_prefix("data: "))?;
            Some((kind.to_owned(), serde_json::from_str(data).ok()?))
        })
        .collect()
}

async fn model_requests(proxy: &MockServer) -> Vec<Value> {
    proxy
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.url.path().starts_with("/v1/llm/"))
        .map(|request| serde_json::from_slice(&request.body).unwrap_or(Value::Null))
        .collect()
}

#[tokio::test]
async fn the_preview_is_gated_on_the_run_and_its_project() {
    let (app, cookie, _proxy) = portal("anthropic", &[]).await;
    let (status, body) = json(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/no-such-run/preview"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // A run whose pass has not happened yet has no document (API/04 "Preview Document").
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, _) = json(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json(
        &app,
        &cookie,
        Method::GET,
        &format!("/api/v1/projects/tampere/agent-runs/{id}/preview"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_proxy_that_does_not_answer_fails_the_run_with_the_reason() {
    // No canned answer mounted: the model route answers 404, which the driver reports. What it
    // reports is a sentence about the run, not the refusal document: the status and the upstream's
    // own words go to the log line beside it, where an operator reads them (T-2419).
    let (app, cookie, _proxy) = portal("anthropic", &[]).await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["failed", "awaiting_approval"]).await;
    assert_eq!(run["status"], json!("failed"), "{run}");
    let reason = run["error"].as_str().unwrap_or_default();
    assert!(
        reason.contains("the model service refused this call")
            && reason.contains("nothing was changed"),
        "{run}"
    );
    assert!(
        !reason.contains("proxy") && !reason.contains("404"),
        "{run}"
    );
    assert!(run["previewUrl"].is_null());
}

#[tokio::test]
async fn a_key_out_of_credit_fails_the_pass_without_the_providers_links() {
    let (app, cookie, proxy) = portal("anthropic", &[]).await;
    Mock::given(method("POST"))
        .and(path("/v1/llm/messages"))
        .respond_with(ResponseTemplate::new(402).set_body_json(json!({ "error": {
            "code": 402,
            "message": "You requested up to 24000 tokens, but can only afford 1200. To increase, visit https://openrouter.ai/workspaces/default/keys/abc"
        } })))
        .mount(&proxy)
        .await;
    let id = create_run(&app, &cookie).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = wait_for(&app, &cookie, &id, &["failed", "awaiting_approval"]).await;
    assert_eq!(run["status"], json!("failed"), "{run}");
    let error = run["error"].as_str().unwrap_or_default();
    assert!(error.contains("credit") && error.contains("1200"), "{run}");
    assert!(!error.contains("openrouter.ai"), "{run}");
    assert_eq!(model_requests(&proxy).await.len(), 1);
}

#[tokio::test]
async fn a_conversation_answers_prose_stays_interviewing_and_answers_second_message() {
    let (app, cookie, _proxy) = portal(
        "anthropic",
        &[
            "You can find bike stations using the catalog search or explore view.".to_owned(),
            "Station Kaivopuisto currently has 7 available bikes.".to_owned(),
        ],
    )
    .await;

    let (status, created) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({
            "message": "Where are the bike stations?"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    assert_eq!(created["kind"], json!("conversation"));
    assert_eq!(created["status"], json!("queued"));
    let id = created["id"].as_str().expect("an id").to_owned();

    let run = wait_for(&app, &cookie, &id, &["interviewing", "failed"]).await;
    assert_eq!(run["status"], json!("interviewing"), "{run}");
    assert!(
        run["previewUrl"].is_null(),
        "conversation runs have no preview"
    );

    let log = events(&app, &cookie, &id).await;
    assert!(
        log.iter().any(|(kind, payload)| kind == "thought"
            && payload["text"]
                .as_str()
                .is_some_and(|t| t.contains("You can find bike stations"))),
        "the model answer was published as a thought: {log:?}"
    );

    let (status, _) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": "How many bikes at Kaivopuisto?" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let mut said_second = false;
    for _ in 0..100 {
        let log = events(&app, &cookie, &id).await;
        said_second = log.iter().any(|(kind, payload)| {
            kind == "thought"
                && payload["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("Station Kaivopuisto currently has 7"))
        });
        if said_second {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(said_second, "second message received second thought answer");

    let run_after = wait_for(&app, &cookie, &id, &["interviewing"]).await;
    assert_eq!(run_after["status"], json!("interviewing"));
    assert!(run_after["previewUrl"].is_null());
}

#[tokio::test]
async fn a_question_reaches_the_registry_and_its_call_is_a_tool_step() {
    // The registry's own operations are tools of the conversation (AG-64, ADR-N-021): the model
    // calls one by name and the person sees the step.
    let (app, cookie, _proxy) = portal(
        "anthropic",
        &[
            concat!(
                "Looking for what the project publishes.\n\n",
                "```json\n",
                "{\"tool\": \"jc_catalog_search\", \"arguments\": {\"q\": \"bikes\"}}\n",
                "```\n"
            )
            .to_owned(),
            "The project publishes the city bike stations.".to_owned(),
        ],
    )
    .await;

    let (status, created) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({ "message": "What does this project publish about bikes?" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let id = created["id"].as_str().expect("an id").to_owned();

    let mut all = Vec::new();
    for _ in 0..200 {
        all = events(&app, &cookie, &id).await;
        if all
            .iter()
            .any(|(kind, payload)| kind == "tool" && payload["tool"] == json!("jc_catalog_search"))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (_, tool) = all
        .iter()
        .find(|(kind, payload)| kind == "tool" && payload["tool"] == json!("jc_catalog_search"))
        .expect("the registry call is a tool step (AG-56)");
    assert_eq!(tool["status"], json!("ok"), "{tool}");
    assert_eq!(tool["input"], json!({ "q": "bikes" }), "{tool}");
    assert!(
        tool["output"].is_object() || tool["output"].is_array(),
        "{tool}"
    );
}

#[tokio::test]
async fn an_agent_never_approves_a_change_however_it_is_asked() {
    // The profile's half refuses every approving verb (AG-70): the operation is not offered and
    // the call is refused before it runs, whatever the person's own grants are.
    let (app, cookie, _proxy) = portal(
        "anthropic",
        &[
            concat!(
                "Approving it.\n\n",
                "```json\n",
                "{\"tool\": \"jc_change_approve\", \"arguments\": {\"change\": \"chg-1a2b3c4d\"}}\n",
                "```\n"
            )
            .to_owned(),
            "I cannot approve a change; a person approves it in the Portal.".to_owned(),
        ],
    )
    .await;

    let (status, created) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/assistant/conversations"),
        Some(json!({ "message": "Approve the open change for me" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let id = created["id"].as_str().expect("an id").to_owned();

    let mut all = Vec::new();
    for _ in 0..200 {
        all = events(&app, &cookie, &id).await;
        if all
            .iter()
            .any(|(kind, payload)| kind == "tool" && payload["tool"] == json!("jc_change_approve"))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (_, tool) = all
        .iter()
        .find(|(kind, payload)| kind == "tool" && payload["tool"] == json!("jc_change_approve"))
        .expect("the refusal is a tool step too");
    assert_eq!(tool["status"], json!("failed"), "{tool}");
    assert!(
        tool["error"]
            .as_str()
            .is_some_and(|reason| reason.contains("agent profile does not grant")),
        "{tool}"
    );
}

// ---- An application: code on the App SDK (T-0680, SDK-10…SDK-17) ----

/// The row types Model Tools renders for the `bikes` model in these cases.
const JC_TYPES: &str = "export interface BikeHireDockingStation { id: string; type: \"BikeHireDockingStation\"; name?: string; availableBikeNumber?: number }\n";

/// A page and its test that build: the SDK hook, the row type as a type-only import.
const STATIONS: &str = "import { useEntities } from \"@joinedcontext/sdk\";\nimport type { BikeHireDockingStation } from \"../jc-types\";\n\nexport function Stations() {\n  const { rows } = useEntities<BikeHireDockingStation>(\"BikeHireDockingStation\");\n  return <ul>{rows.map((row) => <li key={row.id}>{row.name}</li>)}</ul>;\n}";
const STATIONS_TEST: &str = "import { render } from \"@testing-library/react\";\nimport { describe, expect, it } from \"vitest\";\nimport { JcProvider } from \"@joinedcontext/sdk\";\nimport { stubClient } from \"@joinedcontext/sdk/testing\";\nimport { Stations } from \"./Stations\";\n\ndescribe(\"Stations\", () => {\n  it(\"lists the stations\", () => {\n    const { container } = render(<JcProvider client={stubClient()}><Stations /></JcProvider>);\n    expect(container).toBeTruthy();\n  });\n});";
/// The same page reaching for a package SDK-12 does not allow.
const STATIONS_AXIOS: &str = "import axios from \"axios\";\n\nexport function Stations() {\n  return <p>{String(axios)}</p>;\n}";
const APP_PAGES: &str = "      { id: \"overview\", label: t(\"page.overview\"), render: () => <Overview schema={schema} /> },\n";
const APP_IMPORTS: &str = "import { TypePage } from \"./pages/TypePage\";\n";

/// The blocks of an application with a Stations page wired into `App.tsx`.
fn stations_app(page: &str) -> Vec<(&'static str, &'static str, String)> {
    vec![
        ("src/pages/Stations.tsx", "", page.to_owned()),
        ("src/pages/Stations.test.tsx", "", STATIONS_TEST.to_owned()),
        (
            "src/App.tsx",
            APP_IMPORTS,
            format!(
                "{}import {{ Stations }} from \"./pages/Stations\";",
                APP_IMPORTS.trim_end()
            ),
        ),
        (
            "src/App.tsx",
            APP_PAGES,
            format!(
                "{}      {{ id: \"stations\", label: \"Stations\", render: () => <Stations /> }},",
                APP_PAGES
            ),
        ),
    ]
}

fn code_answer(prose: &str, blocks: &[(&str, &str, String)]) -> String {
    let blocks: Vec<(&str, &str, &str)> = blocks
        .iter()
        .map(|(path, search, replace)| (*path, *search, replace.as_str()))
        .collect();
    answer(prose, &blocks)
}

/// The endpoint's schema surface through the proxy and Model Tools' `/generate` (SDK-10).
async fn mount_types(proxy: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/v1/data/schema/index.json"))
        .and(header_regex("authorization", "^Bearer jcr_"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "endpoint": SLUG,
            "models": [{ "name": "bikes", "version": 1, "types": ["BikeHireDockingStation"], "artifacts": {} }]
        })))
        .mount(proxy)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/data/schema/v1/model.linkml.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(BIKES_LINKML))
        .mount(proxy)
        .await;
    Mock::given(method("POST"))
        .and(path("/model-tools/generate"))
        .and(wiremock::matchers::body_json(
            json!({ "source": BIKES_LINKML }),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "typescript": JC_TYPES, "errors": [] })),
        )
        .mount(proxy)
        .await;
}

/// A run of the default kind, `application`.
async fn create_application(app: &axum::Router, cookie: &str) -> String {
    create_application_of(app, cookie, json!(["BikeHireDockingStation"])).await
}

/// An `application` run whose data needs name `types`.
async fn create_application_of(app: &axum::Router, cookie: &str, types: Value) -> String {
    let (status, body) = json(
        app,
        cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(json!({
            "appName": "city-bikes-overview",
            "endpointName": "helsinki-bikes",
            "appClass": "ui",
            "visibility": "project",
            "prompt": "A page listing the bike stations",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                "types": types,
                "attrs": ["name", "location", "availableBikeNumber"],
                "operations": ["queryEntity"]
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    body["id"].as_str().expect("a run id").to_owned()
}

/// The run once its preview is version `v`, or a panic after ten seconds.
async fn wait_for_version(app: &axum::Router, cookie: &str, id: &str, v: u32) -> Value {
    let uri = format!("/api/v1/projects/{PROJECT}/agent-runs/{id}");
    let mut last = Value::Null;
    for _ in 0..200 {
        let (_, run) = json(app, cookie, Method::GET, &uri, None).await;
        if run["previewUrl"]
            .as_str()
            .is_some_and(|url| url.ends_with(&format!("?v={v}")))
            && run["status"] == json!("previewing")
        {
            return run;
        }
        last = run;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the run never showed version {v}: {last}");
}

async fn files_of(state: &AppState, id: &str) -> Value {
    state
        .agents
        .get_run(id)
        .await
        .expect("the store answers")
        .expect("the run")
        .files
}

fn template_file(path: &str) -> String {
    joinedcontext_portal::agents::preview::template_files()
        .remove(path)
        .unwrap_or_else(|| panic!("{path} is in the template"))
}

/// A forge that takes one commit of many files (SDK-17).
/// The application's own repository (AP-75), already created: the run's branch is cut from
/// `main` on the first pass, which then reads its tree once.
const APP_REPO: &str = "/api/v1/repos/org/helsinki_city-bikes-overview";

async fn code_forge() -> MockServer {
    let forge = forge().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/org/manifests/contents"))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({ "files": [], "commit": { "sha": "c0de" } })),
        )
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path(APP_REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{APP_REPO}/branches/")))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{APP_REPO}/branches/")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": "head" } })),
        )
        .mount(&forge)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{APP_REPO}/branches")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{APP_REPO}/git/trees/head")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "truncated": false,
            "tree": [{ "path": "README.md", "type": "blob", "sha": "r" }]
        })))
        .mount(&forge)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{APP_REPO}/contents")))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({ "files": [], "commit": { "sha": "c0de" } })),
        )
        .mount(&forge)
        .await;
    forge
}

#[tokio::test]
async fn a_type_the_endpoint_does_not_serve_never_reaches_the_chat_the_samples_or_the_model() {
    let forge = code_forge().await;
    let answer = code_answer(
        "A page listing the stations, with its test.",
        &stations_app(STATIONS),
    );
    let (_state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[answer], Some(&forge)).await;
    mount_types(&proxy).await;
    // `Entity` is the model's abstract base class: the JSON Schema defines it, the endpoint's
    // index serves only `BikeHireDockingStation`.
    let id =
        create_application_of(&app, &cookie, json!(["BikeHireDockingStation", "Entity"])).await;
    wait_for_version(&app, &cookie, &id, 1).await;

    let log = events(&app, &cookie, &id).await;
    let reading: Vec<&str> = log
        .iter()
        .filter(|(kind, _)| kind == "thought")
        .filter_map(|(_, payload)| payload["text"].as_str())
        .filter(|text| text.starts_with("Reading"))
        .collect();
    assert_eq!(
        reading,
        ["Reading 5 entities of BikeHireDockingStation through the endpoint."]
    );

    let samples: Vec<String> = proxy
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == "/v1/data/ngsi-ld/v1/entities")
        .map(|r| r.url.query().unwrap_or_default().to_owned())
        .collect();
    assert!(!samples.is_empty(), "the served type was sampled");
    assert!(
        samples.iter().all(|query| !query.contains("type=Entity")),
        "{samples:?}"
    );

    let requests = model_requests(&proxy).await;
    let user = requests[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    let needs = user
        .split("Data needs (types and attributes the person asked for):")
        .nth(1)
        .and_then(|rest| rest.split("```").nth(1))
        .expect("the data needs block");
    assert!(needs.contains("\"BikeHireDockingStation\""), "{needs}");
    assert!(!needs.contains("\"Entity\""), "{needs}");
}

/// The indicator space of the city: a second endpoint an application may read (AP-44).
const KPI_SLUG: &str = "q3mzkq2v7w5ayxcbn4ltdj6hofkpis";
const KPI_LINKML: &str = "id: https://hel.fi/models/kpis\nname: kpis\nclasses:\n  KeyPerformanceIndicator:\n    slots: [id, name, kpiValue]\nslots:\n  id: {}\n  name: { range: string }\n  kpiValue: { range: float }\n";
const KPI_TYPES: &str = "export interface KeyPerformanceIndicator { id: string; type: \"KeyPerformanceIndicator\"; name?: string; kpiValue?: number }\n";

/// AP-44, SDK-10, SDK-13: an application of the bikes and their indicators samples each type
/// through the endpoint that serves it, renders the row types of both models into one file and
/// tells the model which endpoint serves what.
#[tokio::test]
async fn an_application_of_two_endpoints_reads_each_through_its_own_and_the_pack_names_both() {
    let forge = code_forge().await;
    let answer = code_answer(
        "A page listing the stations, with its test.",
        &stations_app(STATIONS),
    );
    let (state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[answer], Some(&forge)).await;
    mount_types(&proxy).await;
    state.mirror.upsert(envelope(
        "Endpoint",
        "helsinki-kpi",
        PROJECT,
        json!({
            "slug": KPI_SLUG,
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki-kpi" },
            "audience": "project"
        }),
    ));
    let kpis = format!("/v1/data/endpoints/{KPI_SLUG}");
    Mock::given(method("GET"))
        .and(path(format!("{kpis}/schema/index.json")))
        .and(header_regex("authorization", "^Bearer jcr_"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "endpoint": KPI_SLUG,
            "models": [{ "name": "kpis", "version": 1, "types": ["KeyPerformanceIndicator"], "artifacts": {} }]
        })))
        .mount(&proxy)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{kpis}/ngsi-ld/v1/entities")))
        .and(query_param("type", "KeyPerformanceIndicator"))
        .and(header_regex("authorization", "^Bearer jcr_"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki-kpi:bikes-available-avg", "type": "KeyPerformanceIndicator", "name": "bikes-available-avg", "kpiValue": 4.2 }
        ])))
        .mount(&proxy)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{kpis}/schema/v1/model.linkml.yaml")))
        .respond_with(ResponseTemplate::new(200).set_body_string(KPI_LINKML))
        .mount(&proxy)
        .await;
    Mock::given(method("POST"))
        .and(path("/model-tools/generate"))
        .and(wiremock::matchers::body_json(
            json!({ "source": KPI_LINKML }),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "typescript": KPI_TYPES, "errors": [] })),
        )
        .mount(&proxy)
        .await;

    let (status, body) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(json!({
            "appName": "city-bikes-overview",
            "endpointNames": ["helsinki-bikes", "helsinki-kpi"],
            "appClass": "ui",
            "visibility": "project",
            "prompt": "The stations beside the availability indicator",
            "dataNeeds": [
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                    "types": ["BikeHireDockingStation"],
                    "attrs": ["name", "location", "availableBikeNumber"],
                    "operations": ["queryEntity"]
                },
                {
                    "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki-kpi" },
                    "types": ["KeyPerformanceIndicator"],
                    "attrs": ["name", "kpiValue"],
                    "operations": ["queryEntity"]
                }
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["id"].as_str().expect("a run id").to_owned();
    wait_for_version(&app, &cookie, &id, 1).await;

    let paths: Vec<String> = proxy
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| format!("{}?{}", r.url.path(), r.url.query().unwrap_or_default()))
        .collect();
    assert!(
        paths
            .iter()
            .any(|p| p.starts_with("/v1/data/ngsi-ld/v1/entities?type=BikeHireDockingStation")),
        "{paths:?}"
    );
    assert!(
        paths.iter().any(|p| p.starts_with(&format!(
            "{kpis}/ngsi-ld/v1/entities?type=KeyPerformanceIndicator"
        ))),
        "{paths:?}"
    );
    assert!(
        !paths
            .iter()
            .any(|p| p.starts_with("/v1/data/ngsi-ld/v1/entities?type=KeyPerformanceIndicator")),
        "the indicator is never read through the bikes endpoint: {paths:?}"
    );

    let requests = model_requests(&proxy).await;
    let user = requests[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(user.contains("## THE ENDPOINTS"), "{user}");
    assert!(
        user.contains(
            "`helsinki-kpi` — context space `helsinki-kpi`, types: KeyPerformanceIndicator"
        ),
        "the pack says which endpoint serves the indicator"
    );
    assert!(
        user.contains("bikes-available-avg"),
        "the indicator's sample is in the pack"
    );

    let files = files_of(&state, &id).await;
    let types = files["src/jc-types.ts"].as_str().unwrap_or_default();
    assert!(
        types.contains("interface BikeHireDockingStation"),
        "{types}"
    );
    assert!(
        types.contains("interface KeyPerformanceIndicator"),
        "{types}"
    );
}

/// Operations beside the public stations, on the same space `helsinki`.
const OPS_SLUG: &str = "p8vx2kq7w5ayxcbn4ltdj6hofkops";

/// SDK-02, SDK-13, AP-44: the stations of the public endpoint and the status of the operations
/// endpoint, one space. The run samples the type through both, asks the second for the entities
/// the first answered, joins the rows by id, says in the chat which endpoints it read and tells the
/// model which endpoint carries which attributes.
#[tokio::test]
async fn a_type_two_endpoints_of_one_space_serve_is_sampled_through_both_and_joined_by_id() {
    let forge = code_forge().await;
    let answer = code_answer(
        "A page listing the stations, with its test.",
        &stations_app(STATIONS),
    );
    let (state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[answer], Some(&forge)).await;
    mount_types(&proxy).await;
    state.mirror.upsert(envelope(
        "Endpoint",
        "helsinki-bikes-ops",
        PROJECT,
        json!({
            "slug": OPS_SLUG,
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
            "audience": "project"
        }),
    ));
    let ops = format!("/v1/data/endpoints/{OPS_SLUG}");
    Mock::given(method("GET"))
        .and(path(format!("{ops}/schema/index.json")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "endpoint": OPS_SLUG,
            "models": [{ "name": "bikes", "version": 1, "types": ["BikeHireDockingStation"], "artifacts": {} }]
        })))
        .mount(&proxy)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{ops}/schema/v1/model.linkml.yaml")))
        .respond_with(ResponseTemplate::new(200).set_body_string(BIKES_LINKML))
        .mount(&proxy)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{ops}/ngsi-ld/v1/entities")))
        .and(query_param("type", "BikeHireDockingStation"))
        .and(header_regex("authorization", "^Bearer jcr_"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "urn:ngsi-ld:BikeHireDockingStation:001", "type": "BikeHireDockingStation", "status": "outOfService" },
            { "id": "urn:ngsi-ld:BikeHireDockingStation:002", "type": "BikeHireDockingStation", "status": "working" }
        ])))
        .mount(&proxy)
        .await;

    let (status, body) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(json!({
            "appName": "city-bikes-overview",
            "endpointNames": ["helsinki-bikes", "helsinki-bikes-ops"],
            "appClass": "ui",
            "visibility": "project",
            "prompt": "The stations with their status on a map",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                "types": ["BikeHireDockingStation"],
                "attrs": ["name", "location", "status"],
                "operations": ["queryEntity"]
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["id"].as_str().expect("a run id").to_owned();
    wait_for_version(&app, &cookie, &id, 1).await;

    let asked: Vec<String> = proxy
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == format!("{ops}/ngsi-ld/v1/entities"))
        .map(|r| {
            r.url
                .query_pairs()
                .find(|(key, _)| key == "id")
                .map(|(_, ids)| ids.into_owned())
                .unwrap_or_default()
        })
        .collect();
    assert!(
        asked
            .iter()
            .any(|ids| ids.contains("urn:ngsi-ld:BikeHireDockingStation:001")),
        "the operations endpoint is asked for the stations already read: {asked:?}"
    );

    let log = events(&app, &cookie, &id).await;
    assert!(
        log.iter().any(|(kind, payload)| kind == "thought"
            && payload["text"]
                == json!("Reading 5 entities of BikeHireDockingStation through helsinki-bikes and helsinki-bikes-ops.")),
        "{log:?}"
    );

    let requests = model_requests(&proxy).await;
    let user = requests[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    let samples = user
        .split("Five entities per type, as the SDK's rows (`options=keyValues`):")
        .nth(1)
        .and_then(|rest| rest.split("```").nth(1))
        .and_then(|block| serde_json::from_str::<Value>(block.trim_start_matches("json")).ok())
        .expect("the samples block");
    let first = &samples["BikeHireDockingStation"][0];
    assert_eq!(first["name"], json!("Kaivopuisto"), "{samples}");
    assert_eq!(first["status"], json!("outOfService"), "{samples}");
    assert!(
        user.contains("Types read from several endpoints:"),
        "{user}"
    );
    assert!(
        user.contains("`helsinki-bikes-ops` carries status"),
        "{user}"
    );
    assert!(
        user.contains("join the rows by `id`: BikeHireDockingStation"),
        "{user}"
    );
}

/// AP-137: a second App built as the first one's archetype and layout (both the gallery's
/// citizen map) is told so in the conversation and in its first instruction, with a layout no App
/// of the project holds; the request itself is untouched.
#[tokio::test]
async fn an_app_like_another_of_the_project_is_asked_for_another_layout() {
    use joinedcontext_portal::agents::run::{digest_prompt, mint_run_id, mint_ticket, AgentRun};
    let forge = code_forge().await;
    let answer = code_answer(
        "A page listing the stations, with its test.",
        &stations_app(STATIONS),
    );
    let (state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[answer], Some(&forge)).await;
    mount_types(&proxy).await;
    // The project already holds a citizen map, built from its own first request.
    state.mirror.upsert(envelope(
        "App",
        "bike-map",
        PROJECT,
        json!({ "kind": "static", "source": { "path": "." }, "build": { "node": "22" },
                "visibility": "project", "lifecycle": "published", "dataNeeds": [] }),
    ));
    let earlier = "A map of the bike stations near me";
    let id = mint_run_id();
    let run: AgentRun = serde_json::from_value(json!({
        "id": id, "project": PROJECT, "appName": "bike-map", "endpointName": "helsinki-bikes",
        "endpointSlug": SLUG, "profile": "app-builder", "kind": "application",
        "unattended": false, "appClass": "ui", "visibility": "project",
        "prompt": earlier, "promptDigest": digest_prompt(earlier), "dataNeeds": [],
        "allowsWrite": false, "branch": format!("agent/app-bike-map/{id}"), "pathPrefix": "",
        "status": "published", "ticketHash": mint_ticket().1, "steps": 0, "tokensUsed": 0,
        "createdBy": STEWARD, "createdAt": "2026-09-21T06:00:00Z",
        "expiresAt": "2099-09-21T06:00:00Z"
    }))
    .expect("a run");
    state
        .agents
        .create_run(&run)
        .await
        .expect("the earlier run");

    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;

    let log = events(&app, &cookie, &id).await;
    assert!(
        log.iter().any(|(kind, payload)| kind == "thought"
            && payload["text"].as_str().is_some_and(|text| text
                .starts_with("The App bike-map is already a citizen-map laid out as hero-map"))),
        "{log:?}"
    );
    let requests = model_requests(&proxy).await;
    let user = requests[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(user.contains("A page listing the bike stations"), "{user}");
    assert!(
        user.contains("NOTE ON THE LOOK") && user.contains("`bike-map`"),
        "{user}"
    );
    assert!(!user.contains("gallery layout `hero-map`"), "{user}");
}

/// T-3159, AP-56: a dashboard is written as code on the App SDK like an application, its request
/// framed as read-only; nothing of the kit is written: no `spec.json` reaches a commit.
#[tokio::test]
async fn a_dashboard_is_code_on_the_sdk_and_only_reads() {
    let forge = code_forge().await;
    let answer = code_answer(
        "A page listing the stations, with its test.",
        &stations_app(STATIONS),
    );
    let (_state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[answer], Some(&forge)).await;
    mount_types(&proxy).await;
    let (status, body) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(json!({
            "appName": "city-bikes-overview",
            "endpointName": "helsinki-bikes",
            "appClass": "ui",
            "kind": "dashboard",
            "visibility": "project",
            "prompt": "A dashboard of the bike stations",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                "types": ["BikeHireDockingStation"],
                "attrs": ["name", "location", "availableBikeNumber"],
                "operations": ["queryEntity"]
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["id"].as_str().expect("a run id").to_owned();
    // A dashboard is unattended: its first version ends waiting for approval (AG-69).
    let run = wait_for(&app, &cookie, &id, &["awaiting_approval"]).await;
    assert!(
        run["previewUrl"]
            .as_str()
            .is_some_and(|url| url.ends_with("?v=1")),
        "{run}"
    );

    let requests = model_requests(&proxy).await;
    let system = requests[0]["messages"][0]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(system.contains("AN APPLICATION ON THE JOINEDCONTEXT APP SDK"));
    let user = requests[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(
        user.contains("THIS IS A DASHBOARD: a read-only application"),
        "{user}"
    );
    assert!(user.contains("A dashboard of the bike stations"), "{user}");

    let commits: Vec<String> = forge
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .filter(|request| {
            request.method.as_str() == "POST" && request.url.path().ends_with("/contents")
        })
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    assert!(!commits.is_empty(), "the version was committed");
    assert!(
        commits.iter().all(|body| !body.contains("spec.json")),
        "a dashboard run wrote the kit's spec.json"
    );
    assert!(
        commits.iter().any(|body| body.contains("src/App.tsx")),
        "{commits:?}"
    );
}

/// T-3160, AP-56: an analysis is code on the App SDK like a dashboard: the request is framed as
/// one read-only page with a written finding, the run writes code and no `spec.json`, ends
/// waiting for approval with its preview, and is never published.
#[tokio::test]
async fn an_analysis_is_code_on_the_sdk_and_is_never_published() {
    let forge = code_forge().await;
    let answer = code_answer(
        "A page with the finding and the stations, with its test.",
        &stations_app(STATIONS),
    );
    let (state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[answer], Some(&forge)).await;
    mount_types(&proxy).await;
    let (status, body) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(json!({
            "appName": "city-bikes-analysis",
            "endpointName": "helsinki-bikes",
            "appClass": "ui",
            "kind": "analysis",
            "visibility": "project",
            "prompt": "How many bikes are free across the stations?",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                "types": ["BikeHireDockingStation"],
                "attrs": ["name", "location", "availableBikeNumber"],
                "operations": ["queryEntity"]
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["unattended"], json!(true));
    let id = body["id"].as_str().expect("a run id").to_owned();
    let run = wait_for(&app, &cookie, &id, &["awaiting_approval"]).await;
    assert!(
        run["previewUrl"]
            .as_str()
            .is_some_and(|url| url.ends_with("?v=1")),
        "{run}"
    );

    let requests = model_requests(&proxy).await;
    let user = requests[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(
        user.contains("THIS IS AN ANALYSIS: one read-only page"),
        "{user}"
    );
    assert!(user.contains("How many bikes are free"), "{user}");

    let files = files_of(&state, &id).await;
    let paths: Vec<&String> = files.as_object().expect("files").keys().collect();
    assert!(paths.iter().any(|path| *path == "src/App.tsx"), "{paths:?}");
    assert!(
        paths
            .iter()
            .all(|path| *path != "spec.json" && *path != "data.json"),
        "an analysis run wrote a kit file: {paths:?}"
    );

    let (status, problem) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/publish"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert!(
        problem
            .to_string()
            .contains("an analysis is never published"),
        "{problem}"
    );
}

#[tokio::test]
async fn an_application_is_written_in_one_call_and_the_template_never_reaches_the_frame() {
    let forge = code_forge().await;
    let answer = code_answer(
        "A page listing the stations, with its test.",
        &stations_app(STATIONS),
    );
    let (state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[answer], Some(&forge)).await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    let run = wait_for_version(&app, &cookie, &id, 1).await;
    assert!(run["firstVersionMs"].is_i64(), "{run}");
    // The first frame is the first generated version, so both mark the same moment.
    assert!(
        run["firstFrameMs"].as_i64() <= run["firstVersionMs"].as_i64()
            && run["firstFrameMs"].is_i64(),
        "{run}"
    );

    // The template is the model's context, never the preview: the first preview is the
    // generated version, after the building status (SDK-14, SDK-15).
    let log = events(&app, &cookie, &id).await;
    let previews: Vec<usize> = log
        .iter()
        .enumerate()
        .filter(|(_, (kind, _))| kind == "preview")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(previews.len(), 1, "{log:?}");
    let building = log
        .iter()
        .position(|(kind, payload)| kind == "status" && payload["status"] == json!("building"))
        .expect("the building status");
    assert!(previews[0] > building, "{log:?}");
    assert!(log[previews[0]].1["previewUrl"]
        .as_str()
        .is_some_and(|url| url.ends_with("?v=1")));
    assert!(log.iter().any(|(kind, payload)| kind == "thought"
        && payload["text"] == json!("Writing the application for your request.")));
    assert!(!log.iter().any(|(_, payload)| payload["text"]
        .as_str()
        .is_some_and(|t| t.contains("template"))));

    // A small first version on the code prompt, with the SDK, every template file and the
    // endpoint's types, then the call that completes it (SDK-13).
    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["max_tokens"], json!(20000));
    assert!(requests[0]["messages"][1]["content"]
        .as_str()
        .is_some_and(|user| user.contains("FIRST VERSION")));
    assert_eq!(requests[1]["max_tokens"], json!(64000));
    assert!(requests[1]["messages"][1]["content"]
        .as_str()
        .is_some_and(|user| user.contains("Complete the application for the request")));
    let system = requests[0]["messages"][0]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(system.contains("AN APPLICATION ON THE JOINEDCONTEXT APP SDK"));
    let user = requests[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    for part in [
        "## THE SDK",
        "function useEntities",
        // An unchanged component by its exports, an unchanged test or set-up file by name (T-3076).
        "### src/components/TypeGrid.tsx (an unchanged template component",
        "export function TypeGrid",
        "functions/summary.test.ts",
        "package.json",
        JC_TYPES.trim(),
        "Kaivopuisto",
        "may NOT write",
        "A page listing the bike stations",
    ] {
        assert!(user.contains(part), "the pack lacks {part}");
    }

    // The files: the template, the rendered types, the page and its test, the wiring.
    let files = files_of(&state, &id).await;
    assert_eq!(files["src/jc-types.ts"], json!(JC_TYPES));
    assert_eq!(
        files["src/pages/Stations.tsx"],
        json!(format!("{STATIONS}\n"))
    );
    assert!(files["src/pages/Stations.test.tsx"].is_string());
    let app_tsx = files["src/App.tsx"].as_str().unwrap_or_default();
    assert!(
        app_tsx.contains("import { Stations } from \"./pages/Stations\";"),
        "{app_tsx}"
    );
    assert!(app_tsx.contains("<Stations />"), "{app_tsx}");
    assert_eq!(files["package.json"], json!(template_file("package.json")));
    let tool = log
        .iter()
        .find(|(kind, payload)| kind == "tool" && payload["tool"] == json!("apply_patch"))
        .expect("the apply_patch event");
    assert_eq!(
        tool.1["applied"].as_array().map(Vec::len),
        Some(4),
        "{tool:?}"
    );

    // One commit on the run branch with the whole project at the root of the application's
    // own repository, and its README (AP-75, AP-76).
    let commits: Vec<Value> = forge
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == format!("{APP_REPO}/contents"))
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect();
    assert_eq!(commits.len(), 1, "{commits:?}");
    let paths: Vec<&str> = commits[0]["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["path"].as_str())
        .collect();
    for path in [
        "src/pages/Stations.tsx",
        "src/jc-types.ts",
        "package.json",
        "README.md",
    ] {
        assert!(paths.contains(&path), "{path} not in {paths:?}");
    }
    assert!(log
        .iter()
        .any(|(kind, payload)| kind == "commit" && payload["sha"] == json!("c0de")));

    // The preview is the code document, or 503 in a build without the SDK runtime.
    let (status, headers, body) = call(
        &app,
        &cookie,
        Method::GET,
        run["previewUrl"].as_str().expect("a preview url"),
        None,
    )
    .await;
    match status {
        StatusCode::OK => {
            assert!(String::from_utf8_lossy(&body).contains("importmap"));
            assert!(headers.contains_key(header::CONTENT_SECURITY_POLICY));
        }
        StatusCode::SERVICE_UNAVAILABLE => {}
        other => panic!(
            "the preview answered {other}: {}",
            String::from_utf8_lossy(&body)
        ),
    }
}

#[tokio::test]
async fn a_block_outside_the_writable_paths_is_refused_and_the_rest_lands() {
    let mut blocks = stations_app(STATIONS);
    blocks.push(("src/main.tsx", "", "console.log(\"mine\");".to_owned()));
    blocks.push(("package.json", "", "{}".to_owned()));
    let (state, app, cookie, proxy) = portal_state_with(
        "openai-compatible",
        &[code_answer("Stations.", &blocks)],
        None,
    )
    .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;

    let log = events(&app, &cookie, &id).await;
    let tool = log
        .iter()
        .find(|(kind, payload)| kind == "tool" && payload["tool"] == json!("apply_patch"))
        .expect("the apply_patch event");
    assert_eq!(tool.1["exitCode"], json!(1));
    let refused: Vec<&str> = tool.1["refused"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["path"].as_str())
        .collect();
    assert_eq!(refused, ["src/main.tsx", "package.json"]);
    assert!(tool.1["refused"][0]["reason"]
        .as_str()
        .is_some_and(|reason| reason.contains("never src/main.tsx")));
    // A refusal alone is on the log, not a repair (SDK-11).
    assert_eq!(model_requests(&proxy).await.len(), 2);
    let files = files_of(&state, &id).await;
    assert_eq!(files["src/main.tsx"], json!(template_file("src/main.tsx")));
    assert_eq!(files["package.json"], json!(template_file("package.json")));
    assert!(files["src/pages/Stations.tsx"].is_string());
}

#[tokio::test]
async fn a_refused_import_goes_back_once_with_its_file_and_line_and_the_repair_lands() {
    let (state, app, cookie, proxy) = portal_state_with(
        "openai-compatible",
        &[
            code_answer("Stations.", &stations_app(STATIONS_AXIOS)),
            code_answer(
                "Stations without axios.",
                &[("src/pages/Stations.tsx", "", STATIONS.to_owned())],
            ),
        ],
        None,
    )
    .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;

    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 3);
    let repair = requests[1]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(repair.contains("does not build"), "{repair}");
    assert!(
        repair.contains("- src/pages/Stations.tsx:1:"),
        "the repair names file and line"
    );
    assert!(repair.contains("axios"));
    let files = files_of(&state, &id).await;
    assert_eq!(
        files["src/pages/Stations.tsx"],
        json!(format!("{STATIONS}\n"))
    );
    let log = events(&app, &cookie, &id).await;
    assert!(log.iter().any(|(kind, payload)| {
        kind == "thought"
            && payload["text"]
                .as_str()
                .is_some_and(|t| t == "The application does not build; fixing it.")
    }));
}

#[tokio::test]
async fn a_second_failure_ends_the_first_run_with_no_preview_and_the_errors_said() {
    let (state, app, cookie, proxy) = portal_state_with(
        "openai-compatible",
        &[
            code_answer("Stations.", &stations_app(STATIONS_AXIOS)),
            code_answer("Still axios.", &stations_app(STATIONS_AXIOS)),
        ],
        None,
    )
    .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    let run = wait_for(&app, &cookie, &id, &["previewing", "failed"]).await;
    assert_eq!(run["status"], json!("previewing"), "{run}");
    // Neither the template nor the version that failed is put in the frame (SDK-14, AP-59).
    assert!(run["previewUrl"].is_null(), "{run}");
    assert!(run.get("firstVersionMs").is_none(), "{run}");

    assert_eq!(model_requests(&proxy).await.len(), 2);
    let files = files_of(&state, &id).await;
    assert!(files.get("src/pages/Stations.tsx").is_none());
    assert_eq!(files["src/App.tsx"], json!(template_file("src/App.tsx")));
    assert_eq!(files["src/jc-types.ts"], json!(JC_TYPES));
    let log = events(&app, &cookie, &id).await;
    assert!(log.iter().any(|(kind, payload)| kind == "thought"
        && payload["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("The application could not be built")
                && t.contains("axios")
                && t.ends_with("Send a message to try again."))));
    assert!(!log.iter().any(|(kind, _)| kind == "preview"), "{log:?}");
}

/// T-1662 (`src/agents/oneshot/code_pass.rs`): an answer that carries no block is sent back once,
/// with the reason, to the model; a second one ends the first run with no preview. The person
/// reads a plain sentence and never the patch protocol (T-0785).
#[tokio::test]
async fn an_application_answer_without_blocks_is_asked_again_once_and_the_protocol_stays_hidden() {
    let (_, app, cookie, proxy) = portal_state_with(
        "openai-compatible",
        &[
            "Here is your application.".to_owned(),
            "It is ready.".to_owned(),
        ],
        None,
    )
    .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    let run = wait_for(&app, &cookie, &id, &["previewing", "failed"]).await;
    assert!(run["previewUrl"].is_null(), "{run}");

    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 2, "one repair and no more");
    assert!(
        requests[1]
            .to_string()
            .contains("carried no SEARCH/REPLACE block"),
        "the model is told why it is asked again"
    );
    let said = wait_for_thought(&app, &cookie, &id, "The application could not be built").await;
    for protocol in ["SEARCH", "REPLACE", "block"] {
        assert!(
            !said.contains(protocol),
            "the person read the patch protocol: {said}"
        );
    }
}

/// T-1662: the largest answer the code pass takes is its output budget. An answer cut there is
/// refused whole: nothing of it lands, and the run says why, so a half-written file never
/// reaches the frame.
#[tokio::test]
async fn an_application_answer_cut_at_the_budget_lands_nothing_and_says_why() {
    let (state, app, cookie, proxy) = portal_state_with("openai-compatible", &[], None).await;
    Mock::given(method("POST"))
        .and(path("/v1/llm/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_cut(
            "Stations.\n\n```text\nsrc/pages/Stations.tsx\n<<<<<<< SEARCH\n=======\nexport function Stations() {",
        )))
        .mount(&proxy)
        .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    let run = wait_for(&app, &cookie, &id, &["previewing", "failed"]).await;

    assert_eq!(run["status"], json!("failed"), "{run}");
    let error = run["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("cut at the output budget") && error.contains("nothing was applied"),
        "{error}"
    );
    assert!(run["previewUrl"].is_null(), "{run}");
    let files = files_of(&state, &id).await;
    assert!(
        files.get("src/pages/Stations.tsx").is_none(),
        "a half-written file landed"
    );
}

/// Posts what the frame saw of version `v`, as the host page relays it (SDK-27).
async fn observe(app: &axum::Router, cookie: &str, id: &str, v: u32, pages: Value) -> StatusCode {
    let (status, _, _) = call(
        app,
        cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview-observations"),
        Some(json!({ "version": v, "pages": pages })),
    )
    .await;
    status
}

/// The run's first thought starting with `prefix`, or a panic after ten seconds.
async fn wait_for_thought(app: &axum::Router, cookie: &str, id: &str, prefix: &str) -> String {
    for _ in 0..200 {
        if let Some(text) = events(app, cookie, id)
            .await
            .into_iter()
            .filter(|(kind, _)| kind == "thought")
            .filter_map(|(_, payload)| payload["text"].as_str().map(str::to_owned))
            .find(|text| text.starts_with(prefix))
        {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no thought starting with {prefix:?}");
}

#[tokio::test]
async fn a_version_whose_pages_show_the_sampled_entities_is_checked_without_a_model_call() {
    let (_state, app, cookie, proxy) = portal_state_with(
        "openai-compatible",
        &[code_answer("Stations.", &stations_app(STATIONS))],
        None,
    )
    .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;

    let status = observe(
        &app,
        &cookie,
        &id,
        1,
        json!([
            { "label": "Overview", "text": "Stations 5 · Bikes available 24", "rows": [] },
            { "label": "Stations", "text": "Kaivopuisto 7 Laivasillankatu 2 Viiskulma 11", "rows": [5] }
        ]),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let said = wait_for_thought(&app, &cookie, &id, "Checked").await;
    assert_eq!(
        said,
        "Checked 2 pages: 3 of 5 sampled entities shown, no errors."
    );
    assert_eq!(model_requests(&proxy).await.len(), 2);
}

#[tokio::test]
async fn what_the_preview_shows_wrong_goes_back_as_a_verification_pass_and_a_new_version() {
    let repaired = STATIONS.replace("{rows.map", "{(rows ?? []).map");
    let (state, app, cookie, proxy) = portal_state_with(
        "openai-compatible",
        &[
            code_answer("Stations.", &stations_app(STATIONS)),
            code_answer("Nothing to add.", &[]),
            code_answer(
                "Rows may be absent.",
                &[("src/pages/Stations.tsx", "", repaired.clone())],
            ),
        ],
        None,
    )
    .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;

    // The runtime error the frame reported is folded into the check of the same version.
    let (status, _, _) = call(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview-errors"),
        Some(
            json!({ "message": "rows is undefined", "file": "src/pages/Stations.tsx", "line": 6 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let status = observe(
        &app,
        &cookie,
        &id,
        1,
        json!([{ "label": "Overview", "text": "Stations NaN", "rows": [] }]),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    wait_for_version(&app, &cookie, &id, 2).await;

    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 3);
    let pass = requests[2]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    for part in [
        "Checking its preview against the data found the problems below",
        "- the preview threw: rows is undefined (src/pages/Stations.tsx:6)",
        "- the page \"Overview\" shows NaN where a value belongs",
        "- no page shows any of the sampled BikeHireDockingStation entities (Kaivopuisto, Laivasillankatu, Kapteeninpuistikko)",
        "What the preview rendered, page by page:\n### Overview\nStations NaN",
    ] {
        assert!(pass.contains(part), "the pass lacks {part}: {pass}");
    }
    assert_eq!(
        files_of(&state, &id).await["src/pages/Stations.tsx"],
        json!(format!("{repaired}\n"))
    );
    let found = wait_for_thought(&app, &cookie, &id, "Checking the preview found:").await;
    assert!(found.contains("shows NaN"), "{found}");
}

#[tokio::test]
async fn verification_stops_after_three_passes_and_says_what_is_left() {
    let page = |n: u32| STATIONS.replace("<ul>", &format!("<ul data-pass=\"{n}\">"));
    let (_state, app, cookie, proxy) = portal_state_with(
        "openai-compatible",
        &[
            code_answer("Stations.", &stations_app(STATIONS)),
            code_answer("Nothing to add.", &[]),
            code_answer("One.", &[("src/pages/Stations.tsx", "", page(1))]),
            code_answer("Two.", &[("src/pages/Stations.tsx", "", page(2))]),
            code_answer("Three.", &[("src/pages/Stations.tsx", "", page(3))]),
        ],
        None,
    )
    .await;
    mount_types(&proxy).await;
    let id = create_application(&app, &cookie).await;
    let broken = json!([{ "label": "Stations", "text": "undefined", "rows": [0] }]);
    for v in 1..=3 {
        wait_for_version(&app, &cookie, &id, v).await;
        assert_eq!(
            observe(&app, &cookie, &id, v, broken.clone()).await,
            StatusCode::NO_CONTENT
        );
    }
    wait_for_version(&app, &cookie, &id, 4).await;
    assert_eq!(
        observe(&app, &cookie, &id, 4, broken).await,
        StatusCode::NO_CONTENT
    );
    let left = wait_for_thought(&app, &cookie, &id, "The preview still shows problems").await;
    assert!(left.contains("after 3 verification passes"), "{left}");
    assert!(left.contains("shows undefined"), "{left}");
    assert_eq!(model_requests(&proxy).await.len(), 5);
}

// ---- The editing agent: every message after the first version is a tool loop (T-0684, SDK-20) ----

/// One model answer that calls the tools, in the provider's body.
fn tool_answer(provider: &str, calls: &[(&str, &str, Value)]) -> Value {
    if provider == "anthropic" {
        let content: Vec<Value> = calls
            .iter()
            .map(|(id, name, input)| {
                json!({ "type": "tool_use", "id": id, "name": name, "input": input })
            })
            .collect();
        json!({
            "id": "msg_tools",
            "type": "message",
            "role": "assistant",
            "content": content,
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 300, "output_tokens": 40 }
        })
    } else {
        let tool_calls: Vec<Value> = calls
            .iter()
            .map(|(id, name, input)| {
                json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": input.to_string() }
                })
            })
            .collect();
        json!({
            "choices": [{
                "message": { "role": "assistant", "content": null, "tool_calls": tool_calls },
                "finish_reason": "tool_calls"
            }],
            "usage": { "total_tokens": 340 }
        })
    }
}

/// The model route of `provider`, answering `bodies` in order, one each.
async fn mount_tool_answers(proxy: &MockServer, provider: &str, bodies: &[Value]) {
    let route = if provider == "anthropic" {
        "/v1/llm/messages"
    } else {
        "/v1/llm/chat/completions"
    };
    for body in bodies {
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .up_to_n_times(1)
            .mount(proxy)
            .await;
    }
}

async fn send_message(app: &axum::Router, cookie: &str, id: &str, text: &str) {
    let (status, body) = json(
        app,
        cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/messages"),
        Some(json!({ "text": text })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
}

/// The `tool` events of the run as (tool, status, result), in order.
fn tool_events(log: &[(String, Value)]) -> Vec<(String, String, String)> {
    // The loop's events carry their step; the first pass's `apply_patch` has none.
    log.iter()
        .filter(|(kind, payload)| kind == "tool" && payload.get("step").is_some())
        .map(|(_, payload)| {
            (
                payload["tool"].as_str().unwrap_or_default().to_owned(),
                payload["status"].as_str().unwrap_or_default().to_owned(),
                payload["result"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// A message after the first version: read, an edit that reaches for axios, a check that
/// refuses it, the fix, a passing check that reloads the frame, finish (SDK-20, SDK-12).
async fn a_message_after_the_first_version_is_a_tool_loop(provider: &str) {
    let forge = code_forge().await;
    // The first version, the completing pass (unchanged), then the six tool turns: the proxy
    // answers in mount order, so everything is mounted before the run starts.
    let first = code_answer("A page listing the stations.", &stations_app(STATIONS));
    let complete = code_answer("Complete.", &[]);
    let (state, app, cookie, proxy) =
        portal_state_with(provider, &[first, complete], Some(&forge)).await;
    mount_types(&proxy).await;
    mount_tool_answers(
        &proxy,
        provider,
        &[
            tool_answer(provider, &[("t1", "read_file", json!({ "path": "src/pages/Stations.tsx" }))]),
            tool_answer(
                provider,
                &[(
                    "t2",
                    "edit_file",
                    json!({ "path": "src/pages/Stations.tsx", "search": "", "replace": STATIONS_AXIOS }),
                )],
            ),
            tool_answer(provider, &[("t3", "check", json!({}))]),
            tool_answer(
                provider,
                &[(
                    "t4",
                    "edit_file",
                    json!({ "path": "src/pages/Stations.tsx", "search": "", "replace": STATIONS }),
                )],
            ),
            tool_answer(provider, &[("t5", "check", json!({}))]),
            tool_answer(
                provider,
                &[("t6", "finish", json!({ "message": "The Stations page reads through the SDK again." }))],
            ),
        ],
    )
    .await;
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;
    send_message(&app, &cookie, &id, "Fetch the stations with axios").await;
    let run = wait_for_version(&app, &cookie, &id, 2).await;
    assert!(
        run["previewUrl"]
            .as_str()
            .unwrap_or_default()
            .ends_with("?v=2"),
        "one passing check after a change is one reload: {run}"
    );

    let log = events(&app, &cookie, &id).await;
    let tools = tool_events(&log);
    let names: Vec<&str> = tools.iter().map(|(name, _, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "read_file",
            "edit_file",
            "check",
            "edit_file",
            "check",
            "finish"
        ],
        "{tools:?}"
    );
    // The opening message of the edit carries every file whole, so a read of one unchanged
    // since answers where it is instead of sending it a second time (SDK-20, T-2466); the
    // model still has the file, in its first request of the loop.
    assert_eq!(tools[0].1, "ok");
    assert!(
        tools[0]
            .2
            .starts_with("src/pages/Stations.tsx: shown whole in the request"),
        "the read points at the file in the request: {}",
        tools[0].2
    );
    let opening = model_requests(&proxy).await[2].to_string();
    assert!(
        opening.contains("\"tools\"") && opening.contains("useEntities"),
        "the edit's opening request carries the file the read points at: {opening}"
    );
    assert_eq!(tools[2].1, "failed");
    assert!(
        tools[2].2.contains("axios"),
        "the check names the refused import: {}",
        tools[2].2
    );
    assert_eq!(
        tools[4],
        ("check".to_owned(), "ok".to_owned(), "ok".to_owned())
    );
    let files = files_of(&state, &id).await;
    assert_eq!(
        files["src/pages/Stations.tsx"].as_str().map(str::trim_end),
        Some(STATIONS.trim_end())
    );
    assert!(
        log.iter().any(|(kind, payload)| kind == "thought"
            && payload["text"] == json!("The Stations page reads through the SDK again.")),
        "the finish message reaches the chat"
    );
    let requests = model_requests(&proxy).await;
    assert_eq!(requests.len(), 8, "two first-run passes and six tool turns");
    let last = requests[7].to_string();
    assert!(
        last.contains("\"tools\""),
        "the tools travel in the body: {last}"
    );
    assert!(last.contains("Fetch the stations with axios"), "{last}");
}

#[tokio::test]
async fn a_message_after_the_first_version_is_a_tool_loop_for_anthropic() {
    a_message_after_the_first_version_is_a_tool_loop("anthropic").await;
}

#[tokio::test]
async fn a_message_after_the_first_version_is_a_tool_loop_for_openai() {
    a_message_after_the_first_version_is_a_tool_loop("openai-compatible").await;
}

/// A write outside SDK-11's paths is a refused tool result, never a write and never the end
/// of the turn.
#[tokio::test]
async fn a_write_outside_the_sdk_paths_is_refused_as_a_tool_result() {
    let forge = code_forge().await;
    let first = code_answer("A page listing the stations.", &stations_app(STATIONS));
    let complete = code_answer("Complete.", &[]);
    let (state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[first, complete], Some(&forge)).await;
    mount_types(&proxy).await;
    mount_tool_answers(
        &proxy,
        "openai-compatible",
        &[
            tool_answer(
                "openai-compatible",
                &[
                    (
                        "w1",
                        "write_file",
                        json!({ "path": "../../etc/passwd", "content": "root" }),
                    ),
                    (
                        "w2",
                        "write_file",
                        json!({ "path": "package.json", "content": "{}" }),
                    ),
                ],
            ),
            tool_answer(
                "openai-compatible",
                &[("f", "finish", json!({ "message": "Nothing to change." }))],
            ),
        ],
    )
    .await;
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;
    wait_for_thought(&app, &cookie, &id, "Complete.").await;
    let before = files_of(&state, &id).await;
    send_message(&app, &cookie, &id, "Write the password file").await;
    let said = wait_for_thought(&app, &cookie, &id, "Nothing to change.").await;
    assert_eq!(said, "Nothing to change.");

    let tools = tool_events(&events(&app, &cookie, &id).await);
    assert_eq!(tools.len(), 3, "{tools:?}");
    for refused in &tools[..2] {
        assert_eq!(refused.0, "write_file");
        assert_eq!(refused.1, "failed");
        assert!(
            refused
                .2
                .starts_with("not a path the application may write"),
            "{}",
            refused.2
        );
    }
    assert_eq!(files_of(&state, &id).await, before, "nothing was written");
}

/// The profile's `stepsPerRun` bounds one message: the turn ends with a word for the person
/// and no further model call.
#[tokio::test]
async fn the_step_limit_ends_the_turn_with_a_message() {
    let forge = code_forge().await;
    let first = code_answer("A page listing the stations.", &stations_app(STATIONS));
    let complete = code_answer("Complete.", &[]);
    let (state, app, cookie, proxy) =
        portal_state_with("anthropic", &[first, complete], Some(&forge)).await;
    mount_types(&proxy).await;
    state.mirror.upsert(envelope(
        "AgentProfile",
        "app-builder",
        "org",
        json!({
            "role": "builder",
            "runtime": {
                "image": "ghcr.io/all-hands-ai/agent-server:v1.4.0",
                "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
            },
            "model": { "provider": "anthropic", "name": "claude-sonnet-5", "maxTokensPerRun": 400000, "reasoningEffort": "medium" },
            "limits": { "stepsPerRun": 3, "wallClock": "PT20M", "concurrentRunsPerOrganization": 2, "requestsPerMinute": 60, "maxResponseBytes": 2097152 },
            "egress": { "allowedHosts": [] },
            "tools": ["shell"],
            "workspace": { "cpu": "1", "memory": "2Gi", "ephemeralStorage": "4Gi" }
        }),
    ));
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;
    wait_for_thought(&app, &cookie, &id, "Complete.").await;

    // The model never stops asking for the file list.
    Mock::given(method("POST"))
        .and(path("/v1/llm/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(tool_answer("anthropic", &[("l", "list_files", json!({}))])),
        )
        .mount(&proxy)
        .await;
    send_message(&app, &cookie, &id, "Look around").await;
    // The stop names the limit reached and what was spent, one message (T-2467).
    let said = wait_for_thought(&app, &cookie, &id, "I stopped before finishing").await;
    assert!(
        said.contains("the step limit of this run (3 tool calls a message)"),
        "{said}"
    );
    assert!(
        said.contains("smaller step"),
        "the person is told what to do: {said}"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let tools = tool_events(&events(&app, &cookie, &id).await);
    assert_eq!(tools.len(), 3, "{tools:?}");
    assert_eq!(
        model_requests(&proxy).await.len(),
        2 + 3,
        "two first-run passes, three tool turns, no call past the limit"
    );
}

/// `preview_errors` reads what the frame reported since the version on screen; `call_function`
/// runs one of the run's functions in jc-functions as the person who started the run and
/// reports a function the run does not have (SDK-14, SDK-18, SDK-20).
#[tokio::test]
async fn preview_errors_and_call_function_answer_the_model_as_tool_results() {
    use joinedcontext_portal::auth::oidc::OidcClient;
    use wiremock::matchers::{body_partial_json, header as has_header};

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
            "request": { "method": "POST", "body": { "types": ["Station"] }, "user": { "email": format!("{STEWARD}@hel.fi") } },
            "token": null
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": 201, "body": { "types": [] }, "logs": ["summary 1 types"]
        })))
        .mount(&runtime)
        .await;

    let forge = code_forge().await;
    let proxy = MockServer::start().await;
    let proxy_base = proxy.uri();
    let model_tools = format!("{proxy_base}/model-tools");
    let runtime_uri = runtime.uri();
    let config = Config::from_vars(|key| {
        match key {
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some(proxy_base.as_str()),
            "JC_PORTAL_MODEL_TOOLS_URL" => Some(model_tools.as_str()),
            "JC_AGENT_PROXY_TOKEN" => Some("the-token-only-jc-agent-proxy-has"),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            "JC_PORTAL_PUBLIC_URL" => Some("https://portal.example.com"),
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
    // The sample rows and the row types, as `portal_state_with` mounts them.
    Mock::given(method("GET"))
        .and(path("/v1/data/ngsi-ld/v1/entities"))
        .and(header_regex("authorization", "^Bearer jcr_"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "urn:ngsi-ld:BikeHireDockingStation:001", "type": "BikeHireDockingStation", "name": "Kaivopuisto", "availableBikeNumber": 4 }
        ])))
        .mount(&proxy)
        .await;
    mount_types(&proxy).await;
    let client = GiteaClient::new(forge.uri().parse().expect("url"), "org", "manifests", "t")
        .expect("a forge client");
    let state = AppState::new(config.clone(), Some(oidc))
        .with_mirror(mirror("anthropic"))
        .with_gitea(Arc::new(client));
    let app = server::app(state.clone());
    let cookie = session_cookie(&config, STEWARD);

    let first = code_answer("A page listing the stations.", &stations_app(STATIONS));
    let complete = code_answer("Complete.", &[]);
    mount_tool_answers(
        &proxy,
        "anthropic",
        &[anthropic(&first), anthropic(&complete)],
    )
    .await;
    mount_tool_answers(
        &proxy,
        "anthropic",
        &[
            tool_answer("anthropic", &[("e", "preview_errors", json!({}))]),
            tool_answer(
                "anthropic",
                &[(
                    "c",
                    "call_function",
                    json!({ "name": "summary", "body": { "types": ["Station"] } }),
                )],
            ),
            tool_answer(
                "anthropic",
                &[("n", "call_function", json!({ "name": "nope" }))],
            ),
            tool_answer(
                "anthropic",
                &[("f", "finish", json!({ "message": "Looked at the errors." }))],
            ),
        ],
    )
    .await;
    let id = create_application(&app, &cookie).await;
    wait_for_version(&app, &cookie, &id, 1).await;
    wait_for_thought(&app, &cookie, &id, "Complete.").await;
    let (status, body) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs/{id}/preview-errors"),
        Some(json!({ "message": "boom in Stations", "file": "src/pages/Stations.tsx", "line": 3 })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    send_message(&app, &cookie, &id, "What is wrong with the page?").await;
    wait_for_thought(&app, &cookie, &id, "Looked at the errors.").await;

    let tools = tool_events(&events(&app, &cookie, &id).await);
    let names: Vec<&str> = tools.iter().map(|(name, _, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["preview_errors", "call_function", "call_function", "finish"],
        "{tools:?}"
    );
    assert_eq!(tools[0].1, "ok");
    assert!(
        tools[0].2.contains("boom in Stations") && tools[0].2.contains("src/pages/Stations.tsx"),
        "the frame's error reaches the model: {}",
        tools[0].2
    );
    if joinedcontext_portal::agents::kit::functions_server().is_some() {
        assert_eq!(tools[1].1, "ok", "{}", tools[1].2);
        assert!(
            tools[1].2.starts_with("status 201") && tools[1].2.contains("summary 1 types"),
            "{}",
            tools[1].2
        );
        let invoked = runtime.received_requests().await.unwrap_or_default();
        assert_eq!(invoked.len(), 1, "the function ran once");
    } else {
        assert_eq!(tools[1].1, "failed");
        assert!(tools[1].2.contains("SDK server module"), "{}", tools[1].2);
    }
    assert_eq!(tools[2].1, "failed");
    assert!(tools[2].2.contains("no function 'nope'"), "{}", tools[2].2);
}

/// T-3575, AP-151: a `wasm` run starts from the template with its server half, the model reads
/// the server's rules and its crate but not the lock, may change the server's Rust and never its
/// manifest, and the run commits the crate, the change and the workflow that builds the component.
#[tokio::test]
async fn a_wasm_run_writes_the_server_half_and_commits_the_crate_with_its_workflow() {
    let forge = code_forge().await;
    let mut blocks = stations_app(STATIONS);
    blocks.push((
        "server/src/lib.rs",
        "pub const MAX_TEXT: usize = 500;\n",
        "pub const MAX_TEXT: usize = 280;".to_owned(),
    ));
    blocks.push((
        "server/Cargo.toml",
        "",
        "[dependencies]\ntokio = \"1\"".to_owned(),
    ));
    let answer = code_answer("Stations, and a server that keeps notes.", &blocks);
    // A new App holds no service beyond identity and data: the builder takes the template's
    // email and ai examples out before the first call (T-3709), so one answer is the version.
    let (_state, app, cookie, proxy) =
        portal_state_with("openai-compatible", &[answer], Some(&forge)).await;
    mount_types(&proxy).await;
    let (status, body) = json(
        &app,
        &cookie,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/agent-runs"),
        Some(json!({
            "appName": "city-bikes-overview",
            "endpointName": "helsinki-bikes",
            "appClass": "wasm",
            "visibility": "project",
            "prompt": "Stations, and notes kept on the server",
            "dataNeeds": [{
                "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
                "types": ["BikeHireDockingStation"],
                "attrs": ["name", "location", "availableBikeNumber"],
                "operations": ["queryEntity"]
            }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["id"].as_str().expect("a run id").to_owned();
    wait_for_version(&app, &cookie, &id, 1).await;

    let requests = model_requests(&proxy).await;
    let user = requests[0]["messages"][1]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(user.contains("## THE SERVER HALF (a wasm App)"));
    // T-3585: never the catalog's kit; services appear once the SDK has calls for them.
    assert!(!user.contains("\"kit\""));
    assert!(user.contains("### server/src/lib.rs"));
    assert!(!user.contains("[[package]]"), "the lock reached the model");
    // the template the model reads holds neither example handler
    assert!(!user.contains("fn mail_digest(") && !user.contains("fn summary("));
    // the call after the version completes it; it is never a repair of the template's calls
    let next = requests
        .get(1)
        .and_then(|request| request["messages"][1]["content"].as_str())
        .unwrap_or_default();
    assert!(
        !next.contains("keeps off"),
        "the first version was sent back: {next}"
    );

    let mut committed = std::collections::BTreeMap::new();
    for request in forge.received_requests().await.unwrap_or_default() {
        if request.method.as_str() != "POST" || request.url.path() != format!("{APP_REPO}/contents")
        {
            continue;
        }
        let body: Value = serde_json::from_slice(&request.body).expect("a JSON commit");
        for file in body["files"].as_array().into_iter().flatten() {
            let text = file["content"].as_str().map(|content| {
                String::from_utf8(
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content)
                        .expect("base64"),
                )
                .expect("utf-8")
            });
            committed.insert(file["path"].as_str().unwrap_or_default().to_owned(), text);
        }
    }
    let file = |path: &str| committed.get(path).cloned().flatten().unwrap_or_default();
    assert!(file("server/src/lib.rs").contains("pub const MAX_TEXT: usize = 280;"));
    assert!(
        !file("server/src/lib.rs").contains("email::send(")
            && !file("server/src/lib.rs").contains("ai::complete("),
        "a call of a service the App keeps off was committed"
    );
    assert!(
        !file("server/src/lib.rs").contains("// jc:"),
        "the template's markers reached the App"
    );
    assert!(
        !file("server/Cargo.toml").contains("tokio"),
        "the model changed the crate's manifest"
    );
    assert!(file("server/Cargo.lock").contains("jc-app-sdk"));
    assert!(file("migrations/0001_items.sql").contains("create table if not exists items"));
    assert!(file(".gitea/workflows/build.yml").contains("runs-on: app-build-rust"));
}
