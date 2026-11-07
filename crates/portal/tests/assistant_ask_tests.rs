//! The assistant asks with things to click (AG-83, UI-73, T-1451): a `pick` question offers
//! exactly the resources the person may read, the model narrows that list but never widens it,
//! and an answer is one of what was offered. The model is a stub proxy that asks every turn.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::agents::run::AgentRunEvent;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CSRF: &str = "test-csrf-token-ask";
const READER: &str = "reader@hel.fi";
const BLIND: &str = "blind@hel.fi";

fn config(proxy_base: &str) -> Config {
    Config::from_vars(|key| {
        match key {
            "JC_AGENTS_NAMESPACE" => Some("agents"),
            "JC_AGENT_PROXY_BASE" => Some(proxy_base),
            "JC_AGENT_PROXY_TOKEN" => Some("the-token-only-jc-agent-proxy-has"),
            "JC_PORTAL_BOOTSTRAP_ADMINS" => Some("portal-approver"),
            "JC_PORTAL_PUBLIC_URL" => Some("https://portal.example.com"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("the agent runner block is complete")
}

fn cookie(config: &Config, email: &str) -> String {
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{email}"),
            username: email.split('@').next().unwrap_or(email).to_owned(),
            email: Some(email.to_owned()),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
        .collect();
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, namespace),
        spec,
        status: None,
    }
}

fn endpoint(name: &str, project: &str, representations: &[&str]) -> ResourceEnvelope {
    envelope(
        "Endpoint",
        name,
        project,
        json!({
            "contextSpaceRef": { "name": project },
            "slug": name,
            "audience": "organization",
            "enabledRepresentations": representations,
        }),
    )
}

/// The organization, the builder profile, two endpoints of helsinki and one of espoo; the reader
/// may start a conversation and read endpoints in helsinki, the blind person only start one.
fn mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en" }),
    ));
    mirror.upsert(envelope(
        "AgentProfile",
        "app-builder",
        ORG_NAMESPACE,
        json!({
            "role": "builder",
            "runtime": {
                "image": "ghcr.io/all-hands-ai/agent-server:v1.4.0",
                "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111"
            },
            "model": { "provider": "openai-compatible", "name": "deepseek/deepseek-v4.1-flash", "maxTokensPerRun": 400000 },
            "limits": { "stepsPerRun": 120, "wallClock": "PT20M", "concurrentRunsPerOrganization": 2, "requestsPerMinute": 60, "maxResponseBytes": 2097152 },
            "egress": { "allowedHosts": ["registry.npmjs.org"] },
            "tools": ["shell"],
            "workspace": { "cpu": "1", "memory": "2Gi", "ephemeralStorage": "4Gi" }
        }),
    ));
    for (role, rules) in [
        (
            "endpoint-reader",
            json!([{ "kinds": ["App"], "verbs": ["propose"] }, { "kinds": ["Endpoint"], "verbs": ["read"] }]),
        ),
        (
            "app-starter",
            json!([{ "kinds": ["App"], "verbs": ["propose"] }]),
        ),
    ] {
        mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": rules }),
        ));
    }
    for (binding, who, role) in [
        ("reader", READER, "endpoint-reader"),
        ("blind", BLIND, "app-starter"),
    ] {
        mirror.upsert(envelope(
            "RoleBinding",
            binding,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": who }], "role": role, "scope": { "project": "helsinki" } }),
        ));
    }
    mirror.upsert(endpoint("bikes", "helsinki", &["ngsi-ld", "ogc-features"]));
    mirror.upsert(endpoint("air", "helsinki", &["csv"]));
    mirror.upsert(endpoint("espoo-bikes", "espoo", &["ngsi-ld"]));
    mirror
}

struct Asked {
    state: AppState,
    config: Config,
    run: String,
    proxy: MockServer,
}

/// Starts a conversation as `who` whose model answers every turn with `arguments` for `jc_ask`.
async fn ask(who: &str, arguments: Value) -> Asked {
    let proxy = MockServer::start().await;
    let answer = format!(
        "```json\n{}\n```",
        json!({ "tool": "jc_ask", "arguments": arguments })
    );
    Mock::given(method("POST"))
        .and(path("/v1/llm/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-1", "object": "chat.completion",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": answer }, "finish_reason": "stop" }],
            "usage": { "total_tokens": 900 }
        })))
        .mount(&proxy)
        .await;
    let config = config(&proxy.uri());
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    let (status, created) = send(
        &state,
        &config,
        who,
        "/api/v1/projects/helsinki/assistant/conversations",
        json!({ "message": "Build me a dashboard of bikes and air quality" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let run = created["id"].as_str().expect("run id").to_owned();
    Asked {
        state,
        config,
        run,
        proxy,
    }
}

async fn send(
    state: &AppState,
    config: &Config,
    who: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::COOKIE, cookie(config, who))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn question(asked: &Asked) -> Value {
    for _ in 0..200 {
        let events: Vec<AgentRunEvent> = asked
            .state
            .agents
            .events_since(&asked.run, 0)
            .await
            .expect("events");
        if let Some(q) = events.iter().find(|e| e.kind == "question") {
            return q.payload.clone();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the run asked no question");
}

/// Every prompt the model was sent once it was sent at least `turns` of them.
async fn prompts(asked: &Asked, turns: usize) -> String {
    for _ in 0..200 {
        let requests = asked.proxy.received_requests().await.unwrap_or_default();
        if requests.len() >= turns {
            return requests
                .iter()
                .map(|r| String::from_utf8_lossy(&r.body).into_owned())
                .collect::<Vec<_>>()
                .join("\n");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the model was asked fewer than {turns} times");
}

fn values(question: &Value) -> Vec<String> {
    question["options"]
        .as_array()
        .expect("options")
        .iter()
        .map(|o| o["value"].as_str().expect("value").to_owned())
        .collect()
}

async fn answer(asked: &Asked, question: &Value, answer: Value) -> StatusCode {
    let id = question["questionId"].as_str().expect("question id");
    send(
        &asked.state,
        &asked.config,
        READER,
        &format!("/api/v1/projects/helsinki/agent-runs/{}/answers", asked.run),
        json!({ "questionId": id, "answers": { "answer": answer } }),
    )
    .await
    .0
}

#[tokio::test]
async fn pick_endpoints_offers_exactly_what_the_person_may_read_with_a_description() {
    let asked = ask(
        READER,
        json!({ "question": "Which endpoints should the app read?", "pick": "endpoints", "multiple": true, "min": 1 }),
    )
    .await;
    let q = question(&asked).await;

    let mut offered = values(&q);
    offered.sort();
    assert_eq!(offered, ["air", "bikes"], "never another project's: {q}");
    let bikes = q["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["value"] == "bikes")
        .unwrap();
    let description = bikes["description"].as_str().expect("a description");
    assert!(
        description.contains("ngsi-ld, ogc-features"),
        "{description}"
    );
    assert!(description.contains("organization"), "{description}");
    let schema = &q["schema"]["properties"]["answer"];
    assert_eq!(schema["type"], "array");
    assert_eq!(schema["minItems"], 1);
    assert_eq!(
        q["default"].as_array().map(Vec::len),
        Some(1),
        "at least min by default"
    );
}

#[tokio::test]
async fn an_answer_is_one_of_what_was_offered() {
    let asked = ask(
        READER,
        json!({ "question": "Which endpoints?", "pick": "endpoints", "multiple": true, "min": 1, "max": 2 }),
    )
    .await;
    let q = question(&asked).await;

    assert_eq!(
        answer(&asked, &q, json!(["espoo-bikes"])).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(answer(&asked, &q, json!([])).await, StatusCode::BAD_REQUEST);
    assert_eq!(
        answer(&asked, &q, json!(["air", "air"])).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        answer(&asked, &q, json!("air")).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        answer(&asked, &q, json!(["air", "bikes"])).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn names_the_model_gives_narrow_the_list_and_an_unknown_one_is_dropped() {
    let asked = ask(
        READER,
        json!({ "question": "Which endpoint?", "pick": "endpoints", "options": ["bikes", "ghost", "espoo-bikes"] }),
    )
    .await;
    let q = question(&asked).await;

    assert_eq!(values(&q), ["bikes"]);
    assert_eq!(q["default"], "bikes");
    assert_eq!(
        answer(&asked, &q, json!("air")).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        answer(&asked, &q, json!("bikes")).await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn a_person_who_may_not_read_the_kind_gets_a_refusal_the_model_reads_never_a_question() {
    let asked = ask(
        BLIND,
        json!({ "question": "Which endpoint?", "pick": "endpoints" }),
    )
    .await;
    // Waits for the request that carries the refusal, not for a count of requests: under load a
    // second request can arrive before the one with the tool's answer (seen in a train test).
    let mut sent = String::new();
    for _ in 0..200 {
        sent = prompts(&asked, 2).await;
        if sent.contains("error:") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(
        sent.contains("error:"),
        "the refusal goes back to the model"
    );
    let events = asked
        .state
        .agents
        .events_since(&asked.run, 0)
        .await
        .expect("events");
    assert!(
        events.iter().all(|e| e.kind != "question"),
        "no empty question"
    );
}

#[tokio::test]
async fn the_models_own_options_keep_free_text_and_seven_survive() {
    let asked = ask(
        READER,
        json!({ "question": "Which district?", "options": ["a", "b", "c", "d", "e", "f", "g"] }),
    )
    .await;
    let q = question(&asked).await;

    assert_eq!(values(&q).len(), 7);
    assert_eq!(
        answer(&asked, &q, json!("Kallio, near the station")).await,
        StatusCode::NO_CONTENT
    );
}
