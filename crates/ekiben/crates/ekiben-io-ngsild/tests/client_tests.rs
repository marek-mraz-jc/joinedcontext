//! The broker client's call loop against the in-process fake: denials before any request, the
//! retry rules on transport errors, oversized answers and refused statuses, the stream stopping
//! mid-retry, the `rate_limit` resource and token refusals.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use common::{conf, FakeBroker};
use ekiben_core::{
    CoreError, OrgContext, OrgId, OrgQuotas, Part, RateLimit, ResourceManager, StreamMetrics,
};
use ekiben_io_ngsild::client::{CallEnv, Failure, NgsiClient};
use ekiben_io_ngsild::config::{parse_client, BrokerUrl, Fields, CLIENT_FIELDS};
use ekiben_io_ngsild::metrics;
use reqwest::Method;
use serde_json::{json, Value as JsonValue};
use tokio::sync::watch;

struct Env {
    org: Arc<OrgContext>,
    resources: ResourceManager,
    metrics: Arc<StreamMetrics>,
}

impl Env {
    fn new(name: &str, quotas: OrgQuotas) -> Self {
        let resources = ResourceManager::new();
        let oid = OrgId::new(name);
        resources.set_org_quotas(&oid, quotas);
        let org = resources.get_or_create_org(&oid);
        Self {
            org,
            resources,
            metrics: StreamMetrics::new(format!("{name}-stream"), name),
        }
    }

    fn open(name: &str) -> Self {
        Self::new(
            name,
            OrgQuotas {
                allow_private_ips: true,
                ..Default::default()
            },
        )
    }

    fn call<'a>(&'a self, shutdown: &'a watch::Receiver<bool>) -> CallEnv<'a> {
        CallEnv {
            org: &self.org,
            resources: &self.resources,
            metrics: self.metrics.as_ref(),
            label: "c",
            shutdown,
        }
    }

    fn client(&self, base: &str, extra: JsonValue) -> NgsiClient {
        let c = conf(base, extra);
        let f = Fields::new(&c, "ngsi_ld", CLIENT_FIELDS).expect("fields");
        let url = BrokerUrl::parse("ngsi_ld", base).expect("url");
        let settings = parse_client(&f, &url, Duration::from_secs(5), &[400]).expect("settings");
        NgsiClient::new(&self.org, settings).expect("client")
    }
}

/// A receiver whose sender lives as long as the returned guard.
fn running() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(false)
}

fn stopped() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(true)
}

fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    l.local_addr().expect("addr").port()
}

fn failed(r: Result<ekiben_io_ngsild::client::Answer, Failure>) -> String {
    match r {
        Err(Failure::Failed(t)) => t,
        other => format!("not a Failed: {other:?}"),
    }
}

#[tokio::test]
async fn a_drop_on_code_is_a_refusal_carrying_the_answer() {
    let fake = FakeBroker::start().await;
    fake.script(400, &[], r#"{"title":"bad"}"#);
    let env = Env::open("drop");
    let client = env.client(&fake.base(), json!({"headers": {"X-Trace": "t-1"}}));
    assert_eq!(client.settings().retry.retries, 3);
    let (_tx, rx) = running();
    let url = format!("{}/entities", fake.base());
    let res = client.call(&env.call(&rx), Method::GET, &url, None).await;
    let Err(f @ Failure::Refused(_)) = res else {
        panic!("a drop_on code is a refusal: {res:?}");
    };
    let text = f.text();
    assert!(text.contains("(400)") && text.contains("bad"), "{text}");
    assert_eq!(fake.requests().len(), 1, "a refusal is never retried");
    assert_eq!(
        fake.requests()[0].header("x-trace").as_deref(),
        Some("t-1"),
        "the configured header is sent"
    );
    assert_eq!(
        env.metrics.get_counter(&metrics::requests_key("c", "400")),
        1
    );
}

#[tokio::test]
async fn a_redirect_is_never_followed() {
    let fake = FakeBroker::start().await;
    let elsewhere = format!("http://127.0.0.1:{}/x", closed_port());
    fake.script(302, &[("location", elsewhere.as_str())], "");
    let env = Env::open("redirect");
    let client = env.client(&fake.base(), json!({"retries": 0}));
    let (_tx, rx) = running();
    let url = format!("{}/entities", fake.base());
    let text = failed(client.call(&env.call(&rx), Method::GET, &url, None).await);
    assert!(
        text.contains("(302)"),
        "the 302 itself is the answer: {text}"
    );
}

#[tokio::test]
async fn an_invalid_url_is_denied_before_any_request() {
    let env = Env::open("invalid");
    let client = env.client("http://127.0.0.1:9/ngsi-ld/v1", json!({}));
    let (_tx, rx) = running();
    let res = client
        .call(&env.call(&rx), Method::GET, "http://u:hunter2@[x/e", None)
        .await;
    let Err(f @ Failure::Denied(_)) = res else {
        panic!("an invalid URL is denied: {res:?}");
    };
    let text = f.text();
    assert!(
        text.contains("invalid URL 'http://u:REDACTED@[x/e'"),
        "{text}"
    );
    assert!(!text.contains("hunter2"), "{text}");
}

#[tokio::test]
async fn a_transport_error_is_retried_then_failed_with_the_cause() {
    let env = Env::open("transport");
    let base = format!("http://127.0.0.1:{}/ngsi-ld/v1", closed_port());
    let client = env.client(&base, json!({"retries": 1}));
    let (tx, rx) = running();
    // A dropped sender never stops the stream: the retry waits its period and goes on.
    drop(tx);
    let res = client
        .call(
            &env.call(&rx),
            Method::GET,
            &format!("{base}/entities"),
            None,
        )
        .await;
    let text = failed(res);
    assert!(text.starts_with(&format!("{base}/entities: ")), "{text}");
    assert_eq!(
        env.metrics
            .get_counter(&metrics::requests_key("c", "transport")),
        2,
        "one request and one retry"
    );
}

#[tokio::test]
async fn a_stopped_stream_ends_each_retry_wait() {
    let fake = FakeBroker::start().await;
    let env = Env::open("stopping");
    let dead = format!("http://127.0.0.1:{}/ngsi-ld/v1", closed_port());
    let (_tx, rx) = stopped();

    let client = env.client(&dead, json!({}));
    let text = failed(
        client
            .call(
                &env.call(&rx),
                Method::GET,
                &format!("{dead}/entities"),
                None,
            )
            .await,
    );
    assert!(
        text.starts_with("the stream stopped while retrying: "),
        "{text}"
    );

    fake.script(503, &[], "busy");
    let client = env.client(&fake.base(), json!({}));
    let url = format!("{}/entities", fake.base());
    let text = failed(client.call(&env.call(&rx), Method::GET, &url, None).await);
    assert!(
        text.starts_with("the stream stopped while retrying: "),
        "{text}"
    );
    assert!(text.contains("(503)"), "{text}");

    fake.script(200, &[], &"x".repeat(64));
    let client = env.client(&fake.base(), json!({"max_response_bytes": "16B"}));
    let text = failed(client.call(&env.call(&rx), Method::GET, &url, None).await);
    assert!(
        text.starts_with("the stream stopped while retrying: "),
        "{text}"
    );
    assert!(text.contains("longer than 16 bytes"), "{text}");
}

#[tokio::test]
async fn a_stop_during_a_long_wait_ends_it_at_once() {
    let fake = FakeBroker::start().await;
    fake.script(503, &[], "busy");
    let env = Env::open("stop-mid-wait");
    let client = env.client(&fake.base(), json!({"retry_period": "60s"}));
    let (tx, rx) = running();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = tx.send(true);
    });
    let url = format!("{}/entities", fake.base());
    let res = tokio::time::timeout(
        Duration::from_secs(10),
        client.call(&env.call(&rx), Method::GET, &url, None),
    )
    .await
    .expect("the stop ends the 60 s wait");
    assert!(failed(res).starts_with("the stream stopped while retrying: "));
}

#[tokio::test]
async fn an_answer_over_the_cap_is_retried_then_failed() {
    let fake = FakeBroker::start().await;
    fake.script(200, &[], &"x".repeat(64));
    fake.script(200, &[], &"x".repeat(64));
    let env = Env::open("capped");
    let client = env.client(
        &fake.base(),
        json!({"retries": 1, "max_response_bytes": "16B"}),
    );
    let (_tx, rx) = running();
    let url = format!("{}/entities", fake.base());
    let text = failed(client.call(&env.call(&rx), Method::GET, &url, None).await);
    assert_eq!(text, format!("{url}: the answer is longer than 16 bytes"));
    assert_eq!(fake.requests().len(), 2);
}

#[tokio::test]
async fn a_401_without_a_token_source_is_an_ordinary_failure() {
    let fake = FakeBroker::start().await;
    fake.script(401, &[], "no");
    fake.script(401, &[], "no");
    let env = Env::open("no-token");
    let client = env.client(&fake.base(), json!({"retries": 1}));
    let (_tx, rx) = running();
    let url = format!("{}/entities", fake.base());
    let text = failed(client.call(&env.call(&rx), Method::GET, &url, None).await);
    assert!(text.contains("(401)"), "{text}");
    assert_eq!(
        fake.requests().len(),
        2,
        "no refresh without a token source"
    );
}

#[tokio::test]
async fn a_failing_token_endpoint_is_retried_then_failed_and_a_stop_ends_it() {
    let fake = FakeBroker::start().await;
    fake.with(|s| s.token_status = Some(500));
    let env = Env::open("token-500");
    let oauth = json!({"oauth2": {"enabled": true, "token_url": fake.token_url(),
        "client_key": "k", "client_secret": "hunter2"}, "retries": 1});
    let client = env.client(&fake.base(), oauth.clone());
    let (_tx, rx) = running();
    let url = format!("{}/entities", fake.base());
    let text = failed(client.call(&env.call(&rx), Method::GET, &url, None).await);
    assert!(text.contains("OAuth2 token request"), "{text}");
    assert!(!text.contains("hunter2"), "{text}");
    assert!(
        fake.requests().is_empty(),
        "no broker request without a token"
    );

    let env = Env::open("token-500-stop");
    let client = env.client(&fake.base(), oauth);
    let (_tx, rx) = stopped();
    let text = failed(client.call(&env.call(&rx), Method::GET, &url, None).await);
    assert!(
        text.starts_with("the stream stopped while retrying: OAuth2"),
        "{text}"
    );
}

#[tokio::test]
async fn a_token_url_the_organization_may_not_call_is_denied_at_once() {
    let fake = FakeBroker::start().await;
    let env = Env::new(
        "token-denied",
        OrgQuotas {
            allow_private_ips: true,
            allowed_http_hosts: Some(vec!["127.0.0.1".into()]),
            ..Default::default()
        },
    );
    let token_url = fake.token_url().replace("127.0.0.1", "localhost");
    let client = env.client(
        &fake.base(),
        json!({"oauth2": {"enabled": true, "token_url": token_url, "client_key": "k"}}),
    );
    let (_tx, rx) = running();
    let url = format!("{}/entities", fake.base());
    let res = client.call(&env.call(&rx), Method::GET, &url, None).await;
    let Err(f @ Failure::Denied(_)) = res else {
        panic!("a refused token host is a denial: {res:?}");
    };
    assert!(f.text().contains("allow-list"), "{}", f.text());
    assert!(fake.requests().is_empty());
}

/// Answers `waits` in order, then zero; counts its calls.
struct Paced {
    waits: std::sync::Mutex<Vec<Duration>>,
    calls: AtomicUsize,
}

#[async_trait]
impl RateLimit for Paced {
    async fn access(&self, _parts: &[Part]) -> Result<Duration, CoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut w = self
            .waits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(if w.is_empty() {
            Duration::ZERO
        } else {
            w.remove(0)
        })
    }
}

#[tokio::test]
async fn the_rate_limit_wait_is_honoured_and_a_stop_ends_it() {
    let fake = FakeBroker::start().await;
    let env = Env::open("paced");
    let rl = Arc::new(Paced {
        waits: std::sync::Mutex::new(vec![Duration::from_millis(20)]),
        calls: AtomicUsize::new(0),
    });
    env.resources
        .insert_rate_limit(&env.org.org_id, "egress", rl.clone());
    let client = env.client(&fake.base(), json!({"rate_limit": "egress"}));
    let (_tx, rx) = running();
    let url = format!("{}/entities", fake.base());
    let answer = client
        .call(&env.call(&rx), Method::GET, &url, None)
        .await
        .expect("paced call");
    assert_eq!(answer.status, 200);
    assert_eq!(
        rl.calls.load(Ordering::SeqCst),
        2,
        "asked again after its wait"
    );

    rl.waits
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(Duration::from_secs(60));
    let (_tx, rx) = stopped();
    let res = client.call(&env.call(&rx), Method::GET, &url, None).await;
    assert!(
        matches!(res, Err(Failure::Denied(CoreError::ComponentClosed))),
        "{res:?}"
    );
    assert_eq!(fake.requests().len(), 1);
}
