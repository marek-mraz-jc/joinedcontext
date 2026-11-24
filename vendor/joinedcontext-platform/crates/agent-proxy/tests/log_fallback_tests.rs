//! The proxy's operator lines through the `log` facade (T-7795): a host that installs a `log`
//! logger and no tracing subscriber still reads the audit line, a refused run id, a refused
//! internal call, a refused redirect and the model key's state, each with its fields and never a
//! token. tracing's `log` feature writes them; this binary sets no tracing dispatcher on purpose.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{app, authed, body_of, sample_run, state, Bases, PROXY_CLIENT, RUN_ID};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        if let Ok(mut lines) = LINES.lock() {
            lines.push(format!("{} {}", record.level(), record.args()));
        }
    }
    fn flush(&self) {}
}

/// Every line the facade has received so far.
fn lines() -> String {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        log::set_logger(&Capture).expect("the suite's logger is the only one");
        log::set_max_level(log::LevelFilter::Trace);
    });
    LINES.lock().map(|l| l.join("\n")).unwrap_or_default()
}

async fn eventually(needle: &str) -> String {
    for _ in 0..100 {
        let seen = lines();
        if seen.contains(needle) {
            return seen;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    lines()
}

/// A run id that is not a name is logged by its length, never the id.
#[tokio::test]
async fn an_unshaped_run_id_reaches_the_facade_by_its_length() {
    lines();
    let answer = app(sample_run(false), Bases::default())
        .oneshot(
            Request::builder()
                .uri("/v1/data/ngsi-ld/v1/entities")
                .header("x-jc-run", "run/../x")
                .header("x-jc-ticket", "t")
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::UNAUTHORIZED);
    let seen = eventually("not shaped like a run id").await;
    assert!(seen.contains("length=8"), "{seen}");
    assert!(!seen.contains("run/../x"), "{seen}");
}

/// A relayed gateway answer is one audit line with the run and the upstream; a redirect from the
/// gateway is refused and logged with where it pointed.
#[tokio::test]
async fn the_audit_line_and_a_refused_redirect_reach_the_facade() {
    lines();
    let gateway = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/endpoint/{}/moved", common::SLUG)))
        .respond_with(
            ResponseTemplate::new(307).insert_header("location", "http://elsewhere.invalid/"),
        )
        .mount(&gateway)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
        .mount(&gateway)
        .await;
    let mut proxy_state = (*state(
        sample_run(false),
        Bases {
            gateway: gateway.uri(),
            ..Bases::default()
        },
    ))
    .clone();
    proxy_state.http = agent_proxy::no_redirect_client(None);
    let proxy = agent_proxy::router(Arc::new(proxy_state));
    let read = proxy
        .clone()
        .oneshot(
            authed("GET", "/v1/data/ngsi-ld/v1/entities")
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(body_of(read).await, "[]");
    let moved = proxy
        .oneshot(
            authed("GET", "/v1/data/moved")
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(moved.status(), StatusCode::BAD_GATEWAY);
    let seen = eventually("was not followed").await;
    assert!(seen.contains(&format!("run_id={RUN_ID}")), "{seen}");
    assert!(seen.contains("upstream=context-gateway"), "{seen}");
    assert!(seen.contains("elsewhere.invalid"), "{seen}");
    assert!(!seen.contains("mock-token"), "{seen}");
}

/// An internal call with a token of the Portal's client that is not its service account is
/// logged with the door it knocked on, never the token.
#[tokio::test]
async fn a_refused_internal_call_reaches_the_facade_with_its_door() {
    lines();
    let realm = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("token=not-the-portal"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "active": true, "azp": "portal-api", "username": "someone", "aud": [PROXY_CLIENT]
        })))
        .mount(&realm)
        .await;
    let answer = app(
        sample_run(false),
        Bases {
            realm: Some(format!("{}/realms/jc", realm.uri())),
            ..Bases::default()
        },
    )
    .oneshot(
        Request::builder()
            .method("POST")
            .uri(format!("/internal/runs/{RUN_ID}/data-credential"))
            .header("authorization", "Bearer not-the-portal")
            .body(Body::empty())
            .expect("a request"),
    )
    .await
    .expect("an answer");
    assert_eq!(answer.status(), StatusCode::UNAUTHORIZED);
    let seen = eventually("not from the Portal's service account").await;
    assert!(
        seen.contains("only the Portal asks for a run's data credential"),
        "{seen}"
    );
    assert!(!seen.contains("not-the-portal"), "{seen}");
}

/// The model key's state from the watch's probe reaches the facade with what is left.
#[tokio::test]
async fn the_model_keys_state_reaches_the_facade() {
    lines();
    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": { "limit": 50, "usage": 4.2, "limit_remaining": 45.8 }
        })))
        .mount(&provider)
        .await;
    let mut proxy_state = (*state(sample_run(false), Bases::default())).clone();
    let address = *provider.address();
    let mut config = (*proxy_state.config).clone();
    config.model_base =
        url::Url::parse(&format!("http://openrouter.ai:{}", address.port())).expect("a URL");
    config.model_probe_secs = 1;
    proxy_state.config = Arc::new(config);
    proxy_state.http = reqwest::Client::builder()
        .resolve("openrouter.ai", address)
        .build()
        .expect("a client");
    let watch = tokio::spawn(agent_proxy::model_key::watch(proxy_state));
    let seen = eventually("as the provider describes it").await;
    watch.abort();
    assert!(seen.contains("state=Valid"), "{seen}");
    assert!(seen.contains("remaining=Some(45.8)"), "{seen}");
}
