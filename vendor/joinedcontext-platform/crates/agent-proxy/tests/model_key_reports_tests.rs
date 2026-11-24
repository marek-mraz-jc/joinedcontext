//! The model key's state as the Portal hears it (AG-96, T-7795): the probe loop reports every
//! answer, a probe that gets no answer or a refusal says so without asking further, a burst of
//! refused calls is one report a minute, and a report that cannot be delivered is a log line.
//!
//! Its own binary: `model_key::refused` keeps the last report in a process-wide slot, which would
//! make the refusal suite in edge_routes_llm_handler_tests.rs depend on the order tests run in.

use agent_proxy::config::Config;
use agent_proxy::inject::CredentialManager;
use agent_proxy::limits::LimitManager;
use agent_proxy::model_key::{self, KeyState};
use agent_proxy::runs::RunResolver;
use agent_proxy::ProxyState;
use std::io::Write;
use std::sync::{Arc, Mutex};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<u8>>>);

impl Write for Lines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn logged() -> String {
    static LINES: std::sync::OnceLock<Lines> = std::sync::OnceLock::new();
    let lines = LINES.get_or_init(|| {
        let lines = Lines::default();
        let writer = lines.clone();
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .finish(),
        )
        .expect("the suite's subscriber is the only one");
        lines
    });
    String::from_utf8_lossy(&lines.0.lock().expect("the lines")).into_owned()
}

async fn eventually(what: impl Fn() -> bool) {
    for _ in 0..100 {
        if what() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
}

/// A proxy with the suites' stub credentials (no client secret), its Portal at `portal` and its
/// realm at `realm`, when given.
fn proxy(portal: &str, realm: Option<String>) -> ProxyState {
    let portal = portal.to_owned();
    let config = Arc::new(
        Config::from_lookup(move |key| match key {
            "JC_PORTAL_BASE" => Some(portal.clone()),
            "JC_MODEL_KEY" => Some("mock-model-key".to_owned()),
            "JC_OIDC_ISSUER" => realm.clone(),
            "JC_OIDC_CLIENT_SECRET" => realm.as_ref().map(|_| "proxy-client-secret".to_owned()),
            _ => None,
        })
        .expect("a configuration"),
    );
    let run = serde_json::from_value(serde_json::json!({
        "id": "run-1", "project": "p", "appName": "a", "endpointSlug": "e",
        "allowsWrite": false, "branch": "b", "pathPrefix": "", "status": "building",
        "ticketHash": "", "maxTokens": 1, "allowedHosts": [], "requestsPerMinute": 1,
        "maxResponseBytes": 1, "createdBy": "u", "modelName": "m"
    }))
    .expect("a run");
    ProxyState::new(
        config.clone(),
        RunResolver::with_cached(run),
        CredentialManager::new(config),
        LimitManager::default(),
    )
}

/// A proxy whose model provider is OpenRouter, reached at `provider`, probing every second; its
/// Portal is `portal` and its realm `realm`, when given.
fn openrouter(provider: &MockServer, portal: &str, realm: Option<String>) -> ProxyState {
    let mut proxy_state = proxy(portal, realm);
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
    proxy_state
}

async fn provider_with_key() -> MockServer {
    let provider = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": { "limit": 50, "usage": 4.2, "limit_remaining": 45.8 }
        })))
        .mount(&provider)
        .await;
    provider
}

async fn portal(status: u16) -> MockServer {
    let portal = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/internal/model-key"))
        .respond_with(ResponseTemplate::new(status))
        .mount(&portal)
        .await;
    portal
}

async fn reports(portal: &MockServer) -> Vec<serde_json::Value> {
    portal
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect()
}

/// The watch probes at once and reports what the provider said, with no key in the report;
/// with the key on another provider it asks nobody.
#[tokio::test]
async fn the_watch_reports_each_probe_to_the_portal() {
    logged();
    let (provider, portal) = (provider_with_key().await, portal(204).await);
    let watch = tokio::spawn(model_key::watch(openrouter(&provider, &portal.uri(), None)));
    for _ in 0..100 {
        if !reports(&portal).await.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    watch.abort();
    let sent = reports(&portal).await;
    let first = sent.first().expect("a report");
    assert_eq!(first["state"], "valid");
    assert_eq!(first["source"], "probe");
    assert!(!first.to_string().contains("mock-model-key"));
    assert!(logged().contains("the model key, as the provider describes it"));

    // The key on another provider: the watch asks nobody and returns.
    model_key::watch(proxy("http://portal.invalid", None)).await;
}

/// A probe the provider never answers is `unreachable`; a key it refuses is `invalid`, and the
/// account's credits are not asked about.
#[tokio::test]
async fn a_probe_without_an_answer_or_with_a_refusal_asks_no_further() {
    let gone = MockServer::start().await;
    let mut unreachable = openrouter(&gone, "http://portal.invalid", None);
    let mut config = (*unreachable.config).clone();
    config.model_base = url::Url::parse("http://openrouter.ai:9").expect("a URL");
    unreachable.config = Arc::new(config);
    unreachable.http = reqwest::Client::builder()
        .resolve("openrouter.ai", "127.0.0.1:9".parse().expect("an address"))
        .build()
        .expect("a client");
    assert_eq!(
        model_key::probe(&unreachable).await.state,
        KeyState::Unreachable
    );

    let refusing = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/key"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&refusing)
        .await;
    let report = model_key::probe(&openrouter(&refusing, "http://portal.invalid", None)).await;
    assert_eq!(report.state, KeyState::Invalid);
    let asked: Vec<String> = refusing
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    assert_eq!(asked, ["/key"]);
}

/// A burst of refused calls in the same state is one report; the next state is reported.
#[tokio::test]
async fn a_burst_of_refusals_is_one_report_a_minute() {
    let (provider, portal) = (MockServer::start().await, portal(204).await);
    let proxy_state = openrouter(&provider, &portal.uri(), None);
    model_key::refused(&proxy_state, 401);
    model_key::refused(&proxy_state, 401);
    model_key::refused(&proxy_state, 500);
    model_key::refused(&proxy_state, 402);
    for _ in 0..100 {
        if reports(&portal).await.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let states: Vec<String> = reports(&portal)
        .await
        .iter()
        .map(|r| r["state"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(states.len(), 2, "{states:?}");
    assert!(states.contains(&"invalid".to_owned()) && states.contains(&"out_of_credit".to_owned()));
}

/// A report the Portal refuses, one that cannot reach it, and one the proxy has no token for are
/// each a log line, and the watch goes on.
#[tokio::test]
async fn an_undelivered_report_is_a_log_line() {
    logged();
    let provider = provider_with_key().await;
    let refusing = portal(500).await;
    // Port 9 (discard) answers nothing on a test host: a realm that cannot be reached.
    let silent_realm = "http://127.0.0.1:9/realms/jc".to_owned();
    for (portal, realm, line) in [
        (
            refusing.uri(),
            None,
            "the Portal refused the model key's state",
        ),
        (
            "http://127.0.0.1:9".to_owned(),
            None,
            "the model key's state did not reach the Portal",
        ),
        (
            refusing.uri(),
            Some(silent_realm),
            "no token to report the model key's state with",
        ),
    ] {
        let watch = tokio::spawn(model_key::watch(openrouter(&provider, &portal, realm)));
        eventually(|| logged().contains(line)).await;
        watch.abort();
        assert!(logged().contains(line), "{line}");
    }
}
