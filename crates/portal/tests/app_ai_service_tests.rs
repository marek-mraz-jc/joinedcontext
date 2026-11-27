//! `POST /apps/{name}/api/services/ai/complete`: an App's completion through `jc-agent-proxy` as
//! the Portal's service account naming the App, held to its `aiTokensPerDay`, with the model the
//! platform chooses (AP-169, AP-165, API/06 §3, T-3584). The proxy is a stub with recorded
//! answers; no model is called. Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::envelope;
use joinedcontext_portal::auth::oidc::OidcClient;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::state::AppState;

const APP: &str = "road-defects";
/// The prompt estimate of [`ask`] as sent: a token per four bytes of its messages, rounded up.
const PROMPT_TOKENS: u64 = 16;

async fn database() -> Option<sqlx::PgPool> {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty());
    let Some(url) = url else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return None;
    };
    Some(
        joinedcontext_portal::db::connect(&url)
            .await
            .expect("the test database answers and migrates"),
    )
}

/// A project of its own per test, so the day's counters never meet in the shared database.
fn project(test: &str) -> String {
    let id = crate::common::unique::unique();
    format!("{test}-{id}")
}

/// A stub proxy answering a completion with `content` and 70 + 30 tokens, or with `status`.
async fn proxy(project: &str, content: &str, status: u16) -> MockServer {
    slow_proxy(project, content, status, std::time::Duration::ZERO).await
}

/// [`proxy`], answering after `delay`.
async fn slow_proxy(
    project: &str,
    content: &str,
    status: u16,
    delay: std::time::Duration,
) -> MockServer {
    let proxy = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/llm/chat/completions"))
        .and(wiremock::matchers::header(
            "x-jc-app",
            format!("{project}/{APP}").as_str(),
        ))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer portal-service",
        ))
        .respond_with(
            ResponseTemplate::new(status)
                .set_delay(delay)
                .set_body_json(json!({
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": content } }],
                    "usage": { "prompt_tokens": 70, "completion_tokens": 30, "total_tokens": 100 }
                })),
        )
        .mount(&proxy)
        .await;
    proxy
}

/// A Portal whose realm mints its service token, holding the organization with `org_services`,
/// the `app-builder` profile and App `road-defects` with `ai` and a 150-token day.
async fn state(proxy: &MockServer, project: &str, org_services: Value) -> Option<AppState> {
    let db = database().await?;
    Mock::given(method("POST"))
        .and(path("/realms/banskabystrica/protocol/openid-connect/token"))
        .and(body_string_contains("client_credentials"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "portal-service", "token_type": "Bearer", "expires_in": 300
        })))
        .mount(common::REALM.server)
        .await;
    let config = Config::from_vars(|key| match key {
        "JC_AGENTS_NAMESPACE" => Some("agents".to_owned()),
        "JC_AGENT_PROXY_BASE" => Some(proxy.uri()),
        "JC_OIDC_ISSUER" => Some(common::REALM.issuer.clone()),
        "JC_PORTAL_COOKIE_KEY" => {
            Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
        }
        "JC_OIDC_CLIENT_ID" => Some("portal-api".to_owned()),
        "JC_OIDC_CLIENT_SECRET" => Some("secret".to_owned()),
        _ => None,
    })
    .expect("config");
    let oidc = OidcClient::discover(
        config.oidc.as_ref().expect("a realm"),
        "https://portal.test/api/v1/auth/callback",
    )
    .await
    .expect("discovery");
    let mut state = AppState::new(config, Some(oidc));
    state.db = Some(db);
    state.mirror.upsert(envelope(
        "Organization",
        "helsinki",
        "org",
        json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en",
                "policies": { "apps": { "services": org_services } } }),
    ));
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
            "model": { "provider": "openai-compatible", "name": "deepseek/deepseek-v4.1-flash", "maxTokensPerRun": 400000 },
            "limits": { "stepsPerRun": 120, "wallClock": "PT20M", "concurrentRunsPerOrganization": 2, "requestsPerMinute": 60, "maxResponseBytes": 2097152 },
            "egress": { "allowedHosts": [] },
            "tools": ["shell"],
            "workspace": { "cpu": "1", "memory": "2Gi", "ephemeralStorage": "4Gi" },
        }),
    ));
    state.mirror.upsert(envelope(
        "App",
        APP,
        project,
        json!({
            "kind": "ui", "source": { "path": "." }, "build": {}, "visibility": "project",
            "lifecycle": "published", "dataNeeds": [], "services": ["ai"],
            "limits": { "aiTokensPerDay": 150 },
        }),
    ));
    common::bind(&state.mirror, "jana", project);
    // The project in the Organization's registry row: its day's counters are keyed (T-7200).
    let pool = state.db.clone()?;
    common::keyed_project(&state, &pool, project, "helsinki").await;
    Some(state)
}

async fn complete(state: &AppState, body: Value) -> (StatusCode, Value, String) {
    let token = common::REALM.person_token(APP, "jana", &[]);
    let request = Request::post(format!("/apps/{APP}/api/services/ai/complete"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("a request");
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(request)
        .await
        .expect("a response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("a body");
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    )
}

fn ask() -> Value {
    json!({ "messages": [{ "role": "user", "content": "A pothole on Hlavná 4: how bad?" }] })
}

async fn sent(proxy: &MockServer) -> Vec<Value> {
    proxy
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|request| serde_json::from_slice(&request.body).ok())
        .collect()
}

#[tokio::test]
async fn a_completion_counts_its_tokens_until_the_day_is_spent() {
    let project = project("ai-day");
    let proxy = proxy(&project, "Severity 3.", 200).await;
    let Some(state) = state(&proxy, &project, json!(["identity", "data", "ai"])).await else {
        return;
    };
    let (status, body, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(
        body,
        json!({ "text": "Severity 3.", "tokens": { "in": 70, "out": 30 } })
    );
    // 100 of 150 used: the next answer may take the 50 left and no more.
    let (status, _, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let (status, body, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{text}");
    assert_eq!(
        (body["service"].as_str(), body["quota"].as_str()),
        (Some("ai"), Some("aiTokensPerDay"))
    );
    assert!(
        body["resetAt"]
            .as_str()
            .is_some_and(|at| at.ends_with("T00:00:00Z")),
        "{text}"
    );

    let sent = sent(&proxy).await;
    assert_eq!(sent.len(), 2, "the spent call never reached the proxy");
    assert_eq!(
        sent[0]["model"], "deepseek/deepseek-v4.1-flash",
        "the platform's model"
    );
    assert_eq!(
        sent[0]["max_tokens"],
        150 - PROMPT_TOKENS,
        "never more than the day has left after the prompt"
    );
    assert_eq!(sent[1]["max_tokens"], 50 - PROMPT_TOKENS);
}

#[tokio::test]
async fn a_schema_answer_is_its_json_or_a_502() {
    let project = project("ai-schema");
    let schema = json!({ "type": "object", "required": ["severity"],
                         "properties": { "severity": { "type": "integer" } } });
    let proxy = proxy(&project, "{\"severity\": 3}", 200).await;
    let Some(state) = state(&proxy, &project, json!(["ai"])).await else {
        return;
    };
    let mut asked = ask();
    asked["schema"] = schema.clone();
    let (status, body, text) = complete(&state, asked.clone()).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(body["json"], json!({ "severity": 3 }));
    assert!(sent(&proxy).await[0].to_string().contains("JSON Schema"));

    // a day of its own: the first answer's 100 tokens leave too few for this prompt
    let project = self::project("ai-prose");
    let prose = self::proxy(&project, "It is a bad one.", 200).await;
    let Some(state) = self::state(&prose, &project, json!(["ai"])).await else {
        return;
    };
    let (status, body, text) = complete(&state, asked).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{text}");
    assert_eq!(body["type"], "https://joinedcontext.com/errors/no-json");
}

#[tokio::test]
async fn a_refused_key_is_the_installations_and_ai_off_never_reaches_the_proxy() {
    let project = project("ai-key");
    let proxy = proxy(&project, "", 402).await;
    let Some(state) = state(&proxy, &project, json!(["ai"])).await else {
        return;
    };
    let (status, body, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert_eq!(
        body["type"],
        "https://joinedcontext.com/errors/no-model-key"
    );

    let off = self::proxy(&project, "x", 200).await;
    let Some(state) = self::state(&off, &project, json!(["identity", "data"])).await else {
        return;
    };
    let (status, body, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert_eq!(body["layer"], "organization");
    assert!(sent(&off).await.is_empty());
}

/// [`complete`] with any content type and body.
async fn post_raw(state: &AppState, content_type: &str, body: &str) -> (StatusCode, String) {
    let token = common::REALM.person_token(APP, "jana", &[]);
    let request = Request::post(format!("/apps/{APP}/api/services/ai/complete"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body.to_owned()))
        .expect("a request");
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(request)
        .await
        .expect("a response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("a body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// API/06 §3: a body that is not JSON, not a completion or refused by its limits is a 415 or 400
/// naming the problem; an installation without the model service, without the app-builder
/// profile or whose organization does not allow its model never reaches the proxy.
#[tokio::test]
async fn refused_requests_and_installations_never_reach_the_proxy() {
    let project = project("ai-refused");
    let proxy = proxy(&project, "x", 200).await;
    let Some(mut state) = state(&proxy, &project, json!(["ai"])).await else {
        return;
    };
    let (status, text) = post_raw(&state, "text/plain", &ask().to_string()).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{text}");
    let (status, text) = post_raw(&state, "application/json", "{\"prompt\":\"x\"}").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("not a completion request"), "{text}");
    let (status, _, text) = complete(&state, json!({ "messages": [] })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("1 to 50"), "{text}");

    state.mirror.upsert(envelope(
        "Organization",
        "helsinki",
        "org",
        json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en",
                "policies": { "apps": { "services": ["ai"] },
                              "agents": { "models": ["anthropic/claude-sonnet-5"] } } }),
    ));
    let (status, body, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{text}");
    assert!(
        body.to_string().contains("deepseek/deepseek-v4.1-flash"),
        "{text}"
    );

    state.mirror.upsert(envelope(
        "AgentProfile",
        "app-builder",
        "org",
        json!({ "role": 7 }),
    ));
    let (status, _, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert!(text.contains("app-builder profile"), "{text}");

    let mut config = (*state.config).clone();
    config.agent_settings = None;
    state.config = std::sync::Arc::new(config);
    let (status, _, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert!(text.contains("answers no App's ai calls"), "{text}");
    assert!(sent(&proxy).await.is_empty());
}

/// A proxy that fails or cannot be reached is a 503 the App can retry, and the tokens held for
/// the call are given back to the day.
#[tokio::test]
async fn a_failing_or_unreachable_proxy_is_a_503_and_holds_no_tokens() {
    let project = project("ai-down");
    let failing = proxy(&project, "x", 500).await;
    let Some(state) = state(&failing, &project, json!(["ai"])).await else {
        return;
    };
    let (status, _, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert!(text.contains("the model service failed"), "{text}");

    let Some(mut state) = self::state(&failing, &project, json!(["ai"])).await else {
        return;
    };
    // Port 1 on loopback: nothing listens there, so the connection is refused.
    let mut config = (*state.config).clone();
    if let Some(agents) = config.agent_settings.as_mut() {
        agents.proxy_base = "http://127.0.0.1:1".into();
    }
    state.config = std::sync::Arc::new(config);
    let (status, _, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert!(text.contains("did not answer"), "{text}");

    // A realm that refuses the Portal its own token: nothing to ask the model service with.
    let refusing = MockServer::start().await;
    let issuer = format!("{}/realms/banskabystrica", refusing.uri());
    Mock::given(method("GET"))
        .and(path(
            "/realms/banskabystrica/.well-known/openid-configuration",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
            "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
            "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["ES256"]
        })))
        .mount(&refusing)
        .await;
    Mock::given(method("GET"))
        .and(path("/realms/banskabystrica/protocol/openid-connect/certs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "keys": [] })))
        .mount(&refusing)
        .await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(json!({ "error": "unauthorized_client" })),
        )
        .mount(&refusing)
        .await;
    let Some(mut state) = self::state(&failing, &project, json!(["ai"])).await else {
        return;
    };
    let config = Config::from_vars(|key| match key {
        "JC_OIDC_ISSUER" => Some(issuer.clone()),
        "JC_OIDC_CLIENT_ID" => Some("portal-api".to_owned()),
        "JC_OIDC_CLIENT_SECRET" => Some("secret".to_owned()),
        "JC_PORTAL_COOKIE_KEY" => {
            Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001".into())
        }
        _ => None,
    })
    .expect("config");
    let oidc = OidcClient::discover(
        config.oidc.as_ref().expect("a realm"),
        "https://portal.test/api/v1/auth/callback",
    )
    .await
    .expect("discovery");
    state.oidc = Some(std::sync::Arc::new(oidc));
    let (status, _, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert!(text.contains("no token to ask the model service"), "{text}");

    let unreadable = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "error": "x" })))
        .mount(&unreadable)
        .await;
    let Some(state) = self::state(&unreadable, &project, json!(["ai"])).await else {
        return;
    };
    let (status, _, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    assert!(text.contains("could not be read"), "{text}");

    // The calls settled to nothing: the whole 150-token day is still there.
    let answering = proxy(&project, "Severity 3.", 200).await;
    let Some(state) = self::state(&answering, &project, json!(["ai"])).await else {
        return;
    };
    let (status, _, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::OK, "{text}");
}

/// A database that stops answering is a server error before the model is asked.
#[tokio::test]
async fn a_database_that_stops_answering_never_reaches_the_proxy() {
    let project = project("ai-db-down");
    let proxy = proxy(&project, "x", 200).await;
    let Some(state) = state(&proxy, &project, json!(["ai"])).await else {
        return;
    };
    state.db.as_ref().expect("a database").close().await;
    let (status, _, text) = complete(&state, ask()).await;
    assert!(status.is_server_error(), "{status}: {text}");
    assert!(sent(&proxy).await.is_empty());
}

/// A counter store that refuses a step is a server error where the model is not yet asked, and
/// a settle it refuses after the answer leaves the answer standing and logs it.
#[tokio::test]
async fn a_refusing_counter_store_answers_at_the_step_it_refused() {
    let project = project("ai-counter-refuses");
    let proxy = proxy(&project, "Severity 3.", 200).await;
    let Some(mut state) = state(&proxy, &project, json!(["ai"])).await else {
        return;
    };
    let db = common::build_lane::fresh_db()
        .await
        .expect("JC_PORTAL_TEST_DATABASE_URL names the test database");
    state.db = Some(db.clone());
    common::keyed_project(&state, &db, &project, "helsinki").await;
    let run = |statements: &'static [&'static str]| {
        let db = db.clone();
        async move {
            for statement in statements {
                sqlx::query(sqlx::AssertSqlSafe(*statement))
                    .execute(&db)
                    .await
                    .expect("the test's own database takes it");
            }
        }
    };
    run(&[
        "CREATE FUNCTION refused() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'refused by the test'; END $$",
        "CREATE FUNCTION skipped() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RETURN NULL; END $$",
        // A row trigger: the insert of the day's first call updates no row, the settle does.
        "CREATE TRIGGER refused BEFORE UPDATE ON app_service_usage \
         FOR EACH ROW EXECUTE FUNCTION refused()",
    ])
    .await;
    // The day is held by an insert; the settle's update is refused after the answer.
    let (status, body, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(body["text"], "Severity 3.", "{text}");
    assert_eq!(sent(&proxy).await.len(), 1);

    // An insert the store skips holds nothing: the day reads as spent.
    run(&[
        "DROP TRIGGER refused ON app_service_usage",
        "DELETE FROM app_service_usage",
        "CREATE TRIGGER skipped BEFORE INSERT ON app_service_usage \
         FOR EACH ROW EXECUTE FUNCTION skipped()",
    ])
    .await;
    let (status, _, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{text}");

    // An insert the store refuses is a server error.
    run(&[
        "DROP TRIGGER skipped ON app_service_usage",
        "CREATE TRIGGER refused BEFORE INSERT ON app_service_usage \
         FOR EACH STATEMENT EXECUTE FUNCTION refused()",
    ])
    .await;
    let (status, _, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{text}");

    // A counter that cannot be read is a server error.
    run(&["DROP TABLE app_service_usage"]).await;
    let (status, _, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{text}");
    assert_eq!(sent(&proxy).await.len(), 1, "no refused step asks the model");
}

/// T-3661: the day is reserved before the call, in the statement that checks it, so calls in
/// flight together cannot each see the same day left and pass it together.
#[tokio::test]
async fn parallel_calls_pass_a_one_call_day_at_most_once() {
    let project = project("ai-parallel");
    let proxy = slow_proxy(
        &project,
        "Severity 3.",
        200,
        std::time::Duration::from_millis(400),
    )
    .await;
    let Some(state) = state(&proxy, &project, json!(["ai"])).await else {
        return;
    };
    // One call the gate passes and the handler refuses before the day (415): the person's token
    // is checked once, so the four below race on the day alone.
    let token = common::REALM.person_token(APP, "jana", &[]);
    let warm = Request::post(format!("/apps/{APP}/api/services/ai/complete"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from("x"))
        .expect("a request");
    let warmed = joinedcontext_portal::server::app(state.clone())
        .oneshot(warm)
        .await
        .expect("a response");
    assert_eq!(warmed.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let calls = (0..4).map(|_| complete(&state, ask()));
    let answers = futures_util::future::join_all(calls).await;
    let passed = answers
        .iter()
        .filter(|(status, _, _)| *status == StatusCode::OK)
        .count();
    assert_eq!(passed, 1, "{answers:?}");
    for (status, body, text) in answers.iter().filter(|(s, _, _)| *s != StatusCode::OK) {
        assert_eq!(*status, StatusCode::TOO_MANY_REQUESTS, "{text}");
        assert_eq!(body["quota"], "aiTokensPerDay", "{text}");
    }
    assert_eq!(
        sent(&proxy).await.len(),
        1,
        "the refused calls never reached the proxy"
    );
}

/// T-3661: a prompt is held to what the day has left, as its answer is.
#[tokio::test]
async fn a_prompt_larger_than_the_day_left_is_the_quota_answer() {
    let project = project("ai-prompt");
    let proxy = proxy(&project, "Severity 3.", 200).await;
    let Some(state) = state(&proxy, &project, json!(["ai"])).await else {
        return;
    };
    let long = json!({ "messages": [{ "role": "user", "content": "pothole ".repeat(100) }] });
    let (status, body, text) = complete(&state, long).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{text}");
    assert_eq!(body["quota"], "aiTokensPerDay", "{text}");
    assert!(sent(&proxy).await.is_empty());

    // a call that fails gives its reservation back: the day is whole for the next
    let failing = self::proxy(&project, "", 500).await;
    let Some(state) = self::state(&failing, &project, json!(["ai"])).await else {
        return;
    };
    let (status, _, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{text}");
    let ok = self::proxy(&project, "Severity 3.", 200).await;
    let Some(state) = self::state(&ok, &project, json!(["ai"])).await else {
        return;
    };
    let (status, _, text) = complete(&state, ask()).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(sent(&ok).await[0]["max_tokens"], 150 - PROMPT_TOKENS);
}
