//! E4: the `http_server` input, ported from Bento's `internal/impl/io/input_http_server_test.go`
//! (`bento_input_http_server_test_*`), and its security properties.
//!
//! Each test serves `ingest::router()` on its own loopback port (the route table is global, so
//! every test uses its own stream id) and drives the input with a small loop standing in for the
//! pipeline: it records each batch, fills the sync response store (Bento's `SetAsResponse`) and
//! acks, nacks or never answers.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ekiben_bloblang::Environment;
use ekiben_core::{
    AckResponder, Batch, BatchError, ComponentConfig, OrgId, Registry, ResourceManager,
    StreamContext, StreamMetrics,
};
use ekiben_io_http::http_server::{hmac_signature, lint_streams_mode, HttpServerConf};
use ekiben_io_http::ingest;
use reqwest::StatusCode;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};

const ORG: &str = "org-hs";

enum Action {
    Ack,
    Nack(String),
    Hang,
}

type Respond = Arc<dyn Fn(&mut Batch) -> Action + Send + Sync>;

fn echo() -> Respond {
    Arc::new(|_| Action::Ack)
}

struct Harness {
    /// `http://127.0.0.1:port/{org}/{stream}`.
    base: String,
    /// The ingest listener alone, for paths outside this stream.
    root: String,
    seen: mpsc::UnboundedReceiver<Batch>,
    stop: watch::Sender<bool>,
}

async fn serve_ingest() -> String {
    ingest::activate();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            ingest::router().into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    format!("http://{addr}")
}

fn context(
    stream: &str,
    resources: ResourceManager,
) -> (Arc<Registry>, StreamContext, watch::Sender<bool>) {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    ekiben_io_http::register_all(&reg);
    let org = resources.get_or_create_org(&OrgId::new(ORG));
    let (tx, rx) = watch::channel(false);
    let ctx = StreamContext {
        stream_id: stream.to_string(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new(stream, ORG),
        label: String::new(),
        path: "root.input".to_string(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    };
    (reg, ctx, tx)
}

fn component(conf: Value) -> ComponentConfig {
    ComponentConfig {
        label: String::new(),
        component_type: "http_server".to_string(),
        config: conf,
        processors: Vec::new(),
    }
}

async fn start_with(
    stream: &str,
    conf: Value,
    resources: ResourceManager,
    respond: Respond,
) -> Harness {
    let root = serve_ingest().await;
    let (reg, ctx, stop) = context(stream, resources);
    let mut input = reg
        .build_input(&component(conf), &ctx)
        .expect("the input builds");
    input.connect(&ctx).await.expect("the input connects");
    let (seen_tx, seen) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut held: Vec<AckResponder> = Vec::new();
        while let Ok((mut batch, ack)) = input.read_batch(&ctx).await {
            let action = respond(&mut batch);
            let _ = seen_tx.send(batch.iter().map(|p| p.deep_copy()).collect::<Batch>());
            match action {
                Action::Ack => {
                    if let Some(store) = batch.first().and_then(|p| p.sync_response_store()) {
                        store.add(&batch);
                    }
                    ack.ack();
                }
                Action::Nack(msg) => ack.nack(BatchError::new(batch, msg)),
                Action::Hang => held.push(ack),
            }
        }
        let _ = input.close().await;
    });
    Harness {
        base: format!("{root}/{ORG}/{stream}"),
        root,
        seen,
        stop,
    }
}

async fn start(stream: &str, conf: Value, respond: Respond) -> Harness {
    start_with(stream, conf, ResourceManager::new(), respond).await
}

async fn next_batch(h: &mut Harness) -> Batch {
    tokio::time::timeout(Duration::from_secs(5), h.seen.recv())
        .await
        .expect("a batch arrives")
        .expect("the input runs")
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("ekiben-test-agent")
        .build()
        .unwrap()
}

fn multipart_body(boundary: &str, parts: &[(&str, &str)]) -> String {
    let mut body = String::new();
    for (ct, payload) in parts {
        body.push_str(&format!(
            "--{boundary}\r\nContent-Type: {ct}\r\n\r\n{payload}\r\n"
        ));
    }
    body.push_str(&format!("--{boundary}--\r\n"));
    body
}

/// The (content type, payload) of each part of a multipart answer.
fn read_multipart(content_type: &str, body: &str) -> Vec<(String, String)> {
    let boundary = content_type
        .split("boundary=")
        .nth(1)
        .expect("a boundary")
        .trim_matches('"');
    let delim = format!("--{boundary}");
    body.split(&delim)
        .skip(1)
        .filter(|chunk| !chunk.starts_with("--"))
        .map(|chunk| {
            let chunk = chunk.trim_start_matches("\r\n");
            let (head, payload) = chunk.split_once("\r\n\r\n").expect("part headers");
            let ct = head
                .lines()
                .find_map(|l| l.strip_prefix("Content-Type: "))
                .unwrap_or_default()
                .to_string();
            (ct, payload.trim_end_matches("\r\n").to_string())
        })
        .collect()
}

#[tokio::test]
async fn bento_input_http_server_test_http_basic() {
    let mut h = start("basic", json!({"path": "/testpost"}), echo()).await;
    let c = http();

    for i in 0..3 {
        let payload = format!("test{i}");
        let res = c
            .post(format!("{}/testpost", h.base))
            .header("Content-Type", "application/octet-stream")
            .body(payload.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.text().await.unwrap(),
            payload,
            "the sync response is the message"
        );
        let batch = next_batch(&mut h).await;
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].as_bytes_ref().as_ref(), payload.as_bytes());
    }

    // A multipart body is one message per part, and the answer is multipart again.
    let body = multipart_body(
        "bndry",
        &[("text/plain", "part one"), ("text/plain", "part two")],
    );
    let res = c
        .post(format!("{}/testpost", h.base))
        .header("Content-Type", "multipart/mixed; boundary=bndry")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ct = res.headers()["content-type"].to_str().unwrap().to_string();
    assert!(ct.starts_with("multipart/form-data; boundary="), "{ct}");
    let parts = read_multipart(&ct, &res.text().await.unwrap());
    let payloads: Vec<&str> = parts.iter().map(|(_, p)| p.as_str()).collect();
    assert_eq!(payloads, ["part one", "part two"]);
    let batch = next_batch(&mut h).await;
    assert_eq!(batch.len(), 2);

    // No content type at all is one message.
    let res = c
        .post(format!("{}/testpost", h.base))
        .body("no content type")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await.unwrap(), "no content type");
    assert_eq!(next_batch(&mut h).await.len(), 1);
}

#[tokio::test]
async fn bento_input_http_server_test_http_server_lifecycle() {
    // Skipped in Bento; here: a stopped input leaves the route table, later requests get 404.
    let mut h = start("lifecycle", json!({"path": "/testpost"}), echo()).await;
    let c = http();
    let res = c
        .post(format!("{}/testpost", h.base))
        .body("a")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    next_batch(&mut h).await;
    h.stop.send(true).unwrap();
    let mut status = StatusCode::OK;
    for _ in 0..50 {
        status = c
            .post(format!("{}/testpost", h.base))
            .body("b")
            .send()
            .await
            .unwrap()
            .status();
        if status == StatusCode::NOT_FOUND {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bento_input_http_server_test_http_server_metadata() {
    let mut h = start("metadata", json!({"path": "/across/the/area"}), echo()).await;
    let res = http()
        .post(format!("{}/across/the/area?foo=bar&foo=baz", h.base))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let batch = next_batch(&mut h).await;
    let p = &batch[0];
    assert_eq!(
        p.meta_get_str("http_server_request_path"),
        "/across/the/area"
    );
    assert_eq!(p.meta_get_str("http_server_verb"), "POST");
    assert_eq!(
        p.meta_get_str("http_server_user_agent"),
        "ekiben-test-agent"
    );
    assert_eq!(p.meta_get_str("http_server_remote_ip"), "127.0.0.1");
    assert_eq!(
        p.meta_get_str("foo"),
        "bar",
        "the first query value, as Bento"
    );
    assert_eq!(p.meta_get_str("User-Agent"), "ekiben-test-agent");
    assert!(p.meta_get("Host").is_none());
}

#[tokio::test]
async fn bento_input_http_server_test_http_server_path_parameters() {
    let mut h = start(
        "pathparams",
        json!({"path": "/test/{foo}/{bar}", "allowed_verbs": ["PUT"]}),
        echo(),
    )
    .await;
    let res = http()
        .put(format!("{}/test/foo1/bar1?mylove=u2", h.base))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let p = &next_batch(&mut h).await[0];
    assert_eq!(
        p.meta_get_str("http_server_request_path"),
        "/test/foo1/bar1"
    );
    assert_eq!(p.meta_get_str("http_server_verb"), "PUT");
    assert_eq!(p.meta_get_str("foo"), "foo1");
    assert_eq!(p.meta_get_str("bar"), "bar1");
    assert_eq!(p.meta_get_str("mylove"), "u2");
}

#[tokio::test]
async fn bento_input_http_server_test_http_server_path_parameters_prefixed_metadata() {
    let mut h = start(
        "prefixed",
        json!({"path": "/test/{foo}", "prefix_metadata_keys": true}),
        echo(),
    )
    .await;
    let res = http()
        .post(format!("{}/test/foo1?mylove=u2", h.base))
        .header("X-Custom-Header", "custom")
        .header("Cookie", "mycookie=yum")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let p = &next_batch(&mut h).await[0];
    assert_eq!(p.meta_get_str("path_foo"), "foo1");
    assert_eq!(p.meta_get_str("header_X-Custom-Header"), "custom");
    assert_eq!(p.meta_get_str("query_mylove"), "u2");
    assert_eq!(p.meta_get_str("cookie_mycookie"), "yum");
    assert!(p.meta_get("foo").is_none());
}

#[tokio::test]
async fn bento_input_http_server_test_http_server_path_is_prefix() {
    let mut h = start("pathprefix", json!({"path": "/test/{foo}/{bar}/"}), echo()).await;
    let res = http()
        .post(format!("{}/test/foo1/bar1/baz1", h.base))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let p = &next_batch(&mut h).await[0];
    assert_eq!(
        p.meta_get_str("http_server_request_path"),
        "/test/foo1/bar1/baz1"
    );
    assert_eq!(p.meta_get_str("foo"), "foo1");
    assert_eq!(p.meta_get_str("bar"), "bar1");
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn bento_input_http_server_test_http_server_path_parameters_custom_server_path_is_prefix() {
    let port = free_port();
    let mut h = start(
        "customprefix",
        json!({"address": format!("127.0.0.1:{port}"), "path": "/test/{foo}/{bar}/"}),
        echo(),
    )
    .await;
    let res = http()
        .post(format!(
            "http://127.0.0.1:{port}/test/foo1/bar1/baz1?mylove=u2"
        ))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let p = &next_batch(&mut h).await[0];
    assert_eq!(
        p.meta_get_str("http_server_request_path"),
        "/test/foo1/bar1/baz1"
    );
    assert_eq!(p.meta_get_str("foo"), "foo1");
    assert_eq!(p.meta_get_str("bar"), "bar1");
    assert_eq!(p.meta_get_str("mylove"), "u2");
}

#[tokio::test]
async fn bento_input_http_server_test_http_server_path_parameters_custom_server() {
    // `address` (outside streams mode): the input serves its own listener.
    let port = free_port();
    let mut h = start(
        "customserver",
        json!({"address": format!("127.0.0.1:{port}"), "path": "/testpost"}),
        echo(),
    )
    .await;
    let c = http();
    let res = c
        .post(format!("http://127.0.0.1:{port}/testpost"))
        .body("custom")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.text().await.unwrap(), "custom");
    assert_eq!(
        next_batch(&mut h).await[0].meta_get_str("http_server_remote_ip"),
        "127.0.0.1"
    );
    let res = c
        .post(format!("http://127.0.0.1:{port}/other"))
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    // It is not on the ingest listener.
    let res = c
        .post(format!("{}/testpost", h.base))
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bento_input_http_server_test_http_bad_requests() {
    let h = start("badrequests", json!({"path": "/testpost"}), echo()).await;
    let res = http()
        .get(format!("{}/testpost", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(res.text().await.unwrap(), "Incorrect method\n");
}

#[tokio::test]
async fn bento_input_http_server_test_http_timeout() {
    let h = start(
        "timeout",
        json!({"path": "/testpost", "timeout": "1ms"}),
        Arc::new(|_| Action::Hang),
    )
    .await;
    let res = http()
        .post(format!("{}/testpost", h.base))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(res.text().await.unwrap(), "Request timed out\n");
}

#[tokio::test]
async fn bento_input_http_server_test_http_rate_limit() {
    let resources = ResourceManager::new();
    resources.insert_rate_limit(
        &OrgId::new(ORG),
        "foorl",
        Arc::new(
            ekiben_processors::resources::LocalRateLimit::new(1, 0, Duration::from_secs(60))
                .unwrap(),
        ),
    );
    let h = start_with(
        "ratelimit",
        json!({"path": "/testpost", "rate_limit": "foorl"}),
        resources,
        echo(),
    )
    .await;
    let c = http();
    let first = c
        .post(format!("{}/testpost", h.base))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let second = c
        .post(format!("{}/testpost", h.base))
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(second.headers().contains_key("retry-after"));
    assert_eq!(second.text().await.unwrap(), "Too Many Requests\n");
}

#[test]
fn a_missing_rate_limit_resource_is_a_config_error() {
    let (reg, ctx, _stop) = context("norl", ResourceManager::new());
    let err = reg
        .build_input(&component(json!({"rate_limit": "nope"})), &ctx)
        .err()
        .expect("refused");
    assert!(
        err.to_string()
            .contains("rate limit resource 'nope' was not found"),
        "{err}"
    );
}

#[test]
fn bento_input_http_server_test_http_server_websockets() {
    // Bento's ws_path is not built in: the field is refused rather than ignored.
    let err = HttpServerConf::from_json(&json!({"ws_path": "/ws"}), &Environment::new())
        .expect_err("ws_path is refused");
    assert!(err.to_string().contains("ws_path"), "{err}");
}

#[tokio::test]
async fn bento_input_http_server_test_http_sync_response_headers() {
    let mut h = start(
        "syncheaders",
        json!({
            "path": "/testpost",
            "sync_response": {
                "headers": {
                    "Content-Type": "application/json",
                    "foo": "${!json(\"field1\")}"
                },
                "metadata_headers": {
                    "include_prefixes": ["Loca"],
                    "include_patterns": ["name"]
                }
            }
        }),
        Arc::new(|batch: &mut Batch| {
            for p in batch.iter_mut() {
                p.meta_set_str("Location", "Asgard");
                p.meta_set_str("Username", "Thor");
                p.meta_set_str("Language", "Norse");
            }
            Action::Ack
        }),
    )
    .await;
    let input = r#"{"foo":"test message","field1":"bar"}"#;
    let res = http()
        .post(format!("{}/testpost", h.base))
        .body(input)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let headers = res.headers().clone();
    assert_eq!(headers["content-type"], "application/json");
    assert_eq!(headers["foo"], "bar");
    assert_eq!(headers["location"], "Asgard");
    assert_eq!(headers["username"], "Thor");
    assert!(!headers.contains_key("language"));
    assert_eq!(res.text().await.unwrap(), input);
    next_batch(&mut h).await;
}

#[tokio::test]
async fn bento_input_http_server_test_http_sync_response_multipart() {
    let mut h = start(
        "syncmultipart",
        json!({"path": "/testpost", "sync_response": {"headers": {"content-type": "application/json"}}}),
        Arc::new(|batch: &mut Batch| {
            for (i, p) in batch.iter_mut().enumerate() {
                p.set_bytes(format!("{{\"replaced\":{i}}}"));
            }
            Action::Ack
        }),
    )
    .await;
    let body = multipart_body(
        "inbound",
        &[
            ("text/plain", "one"),
            ("text/plain", "two"),
            ("text/plain", "three"),
        ],
    );
    let res = http()
        .post(format!("{}/testpost", h.base))
        .header("Content-Type", "multipart/mixed; boundary=inbound")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ct = res.headers()["content-type"].to_str().unwrap().to_string();
    let parts = read_multipart(&ct, &res.text().await.unwrap());
    assert_eq!(
        parts,
        vec![
            (
                "application/json".to_string(),
                "{\"replaced\":0}".to_string()
            ),
            (
                "application/json".to_string(),
                "{\"replaced\":1}".to_string()
            ),
            (
                "application/json".to_string(),
                "{\"replaced\":2}".to_string()
            ),
        ]
    );
    assert_eq!(next_batch(&mut h).await.len(), 3);
}

#[tokio::test]
async fn bento_input_http_server_test_http_sync_response_headers_status() {
    let mut h = start(
        "syncstatus",
        json!({
            "path": "/testpost",
            "sync_response": {
                "status": "${! meta(\"status\").or(\"200\") }",
                "headers": {"Content-Type": "application/json"}
            }
        }),
        Arc::new(|batch: &mut Batch| {
            for p in batch.iter_mut() {
                if p.as_bytes_ref().as_ref() == b"bad" {
                    p.meta_set_str("status", "400");
                }
            }
            Action::Ack
        }),
    )
    .await;
    let c = http();
    let ok = c
        .post(format!("{}/testpost", h.base))
        .body("good")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(ok.headers()["content-type"], "application/json");
    let bad = c
        .post(format!("{}/testpost", h.base))
        .body("bad")
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    assert_eq!(bad.text().await.unwrap(), "bad");
    next_batch(&mut h).await;
    next_batch(&mut h).await;
}

#[tokio::test]
async fn bento_input_http_server_test_http_server_input_enable_cors_origins() {
    let h = start(
        "cors",
        json!({"path": "/testpost", "cors": {"enabled": true, "allowed_origins": ["foo"]}}),
        echo(),
    )
    .await;
    let c = http();
    let pre = c
        .request(reqwest::Method::OPTIONS, format!("{}/testpost", h.base))
        .header("Origin", "foo")
        .header("Access-Control-Request-Method", "POST")
        .send()
        .await
        .unwrap();
    assert_eq!(pre.status(), StatusCode::OK);
    assert_eq!(pre.headers()["access-control-allow-origin"], "foo");
    let other = c
        .request(reqwest::Method::OPTIONS, format!("{}/testpost", h.base))
        .header("Origin", "bar")
        .send()
        .await
        .unwrap();
    assert!(!other.headers().contains_key("access-control-allow-origin"));
}

#[tokio::test]
async fn a_nacked_request_answers_502_with_the_error() {
    let h = start(
        "nack",
        json!({"path": "/testpost"}),
        Arc::new(|_| Action::Nack("output went away".to_string())),
    )
    .await;
    let res = http()
        .post(format!("{}/testpost", h.base))
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(res.text().await.unwrap(), "output went away\n");
}

const BEARER: &str = "ingest-bearer-e4-secret";

#[tokio::test]
async fn a_missing_and_a_wrong_bearer_get_the_same_401_and_the_token_never_becomes_metadata() {
    let mut h = start(
        "bearer",
        json!({"path": "/in", "auth": {"bearer": BEARER}}),
        echo(),
    )
    .await;
    let c = http();
    let missing = c
        .post(format!("{}/in", h.base))
        .body("x")
        .send()
        .await
        .unwrap();
    let wrong = c
        .post(format!("{}/in", h.base))
        .bearer_auth("ingest-bearer-e4-secreT")
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        missing.headers()["www-authenticate"],
        wrong.headers()["www-authenticate"]
    );
    let (a, b) = (missing.text().await.unwrap(), wrong.text().await.unwrap());
    assert_eq!(a, b);
    assert!(!a.contains(BEARER));
    assert!(h.seen.try_recv().is_err(), "nothing reached the pipeline");

    let ok = c
        .post(format!("{}/in", h.base))
        .bearer_auth(BEARER)
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let p = &next_batch(&mut h).await[0];
    for (k, v) in p.meta_all() {
        assert!(
            !v.to_string_coerced().contains(BEARER),
            "metadata {k} holds the token"
        );
    }
    assert!(p.meta_get("Authorization").is_none());
}

#[tokio::test]
async fn an_hmac_signed_body_passes_and_a_bad_signature_does_not() {
    let secret = "hmac-e4-secret";
    let mut h = start(
        "hmac",
        json!({"path": "/in", "auth": {"hmac": {"header": "X-Hub-Signature-256", "secret": secret}}}),
        echo(),
    )
    .await;
    let c = http();
    let body = r#"{"event":"push"}"#;
    let sig = format!("sha256={}", hmac_signature(secret, body.as_bytes()));
    let ok = c
        .post(format!("{}/in", h.base))
        .header("X-Hub-Signature-256", &sig)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let p = &next_batch(&mut h).await[0];
    assert!(p.meta_get("X-Hub-Signature-256").is_none());

    for bad in [
        format!("sha256={}", hmac_signature("other", body.as_bytes())),
        "sha256=zz".to_string(),
        String::new(),
    ] {
        let res = c
            .post(format!("{}/in", h.base))
            .header("X-Hub-Signature-256", bad)
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
    // The signature covers the body: a changed body with an old signature is refused.
    let res = c
        .post(format!("{}/in", h.base))
        .header("X-Hub-Signature-256", &sig)
        .body(r#"{"event":"delete"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_body_over_max_body_bytes_gets_413() {
    let h = start(
        "bodycap",
        json!({"path": "/in", "max_body_bytes": 10}),
        echo(),
    )
    .await;
    let c = http();
    let res = c
        .post(format!("{}/in", h.base))
        .body("0123456789A")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let res = c
        .post(format!("{}/in", h.base))
        .body("0123456789")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    // A chunked body without a length is cut at the cap too.
    let chunks: Vec<Result<&'static str, std::io::Error>> = vec![Ok("012345"), Ok("6789AB")];
    let res = c
        .post(format!("{}/in", h.base))
        .body(reqwest::Body::wrap_stream(futures::stream::iter(chunks)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn requests_over_max_in_flight_get_503_with_retry_after() {
    let h = start(
        "inflight",
        json!({"path": "/in", "max_in_flight": 1, "timeout": "3s"}),
        Arc::new(|_| Action::Hang),
    )
    .await;
    let url = format!("{}/in", h.base);
    let first = tokio::spawn({
        let url = url.clone();
        async move { http().post(url).body("a").send().await.map(|r| r.status()) }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let second = http().post(&url).body("b").send().await.unwrap();
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(second.headers()["retry-after"], "1");
    assert_eq!(first.await.unwrap().unwrap(), StatusCode::REQUEST_TIMEOUT);
}

#[tokio::test]
async fn another_organizations_path_and_an_unknown_route_get_404() {
    let h = start(
        "isolated",
        json!({"path": "/in", "auth": {"public": true}}),
        echo(),
    )
    .await;
    let c = http();
    for path in [
        "/other-org/isolated/in",
        "/org-hs/unknown/in",
        "/org-hs/isolated/nope",
        "/",
        "/org-hs",
    ] {
        let res = c
            .post(format!("{}{path}", h.root))
            .body("x")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(res.text().await.unwrap(), "404 page not found\n");
    }
    let res = c
        .post(format!("{}/in", h.base))
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_same_path_twice_in_one_stream_is_refused() {
    let _h = start("dup", json!({"path": "/in"}), echo()).await;
    let (reg, ctx, _stop) = context("dup", ResourceManager::new());
    let mut second = reg
        .build_input(&component(json!({"path": "/in"})), &ctx)
        .unwrap();
    let err = second
        .connect(&ctx)
        .await
        .expect_err("a second /in is refused");
    assert!(err.to_string().contains("already served"), "{err}");
}

#[test]
fn streams_mode_lint_refuses_own_listeners_tls_and_missing_auth() {
    let conf = json!({
        "input": {"broker": {"inputs": [
            {"http_server": {"address": "0.0.0.0:4195", "auth": {"public": true}}},
            {"http_server": {"path": "/x"}}
        ]}},
        "input_resources": [{"label": "r", "http_server": {"cert_file": "/c.pem", "auth": {"bearer": "t"}}}]
    });
    let errs = lint_streams_mode(&conf);
    assert!(
        errs.iter().any(|e| e.contains("http_server.address")),
        "{errs:?}"
    );
    assert!(
        errs.iter().any(|e| e.contains("http_server.cert_file")),
        "{errs:?}"
    );
    assert!(errs.iter().any(|e| e.contains("needs auth")), "{errs:?}");
    assert!(
        lint_streams_mode(&json!({"input": {"http_server": {"auth": {"public": true}}}}))
            .is_empty()
    );
}

#[test]
fn auth_config_is_checked_and_never_shown() {
    let env = Environment::new();
    let conf = HttpServerConf::from_json(&json!({"auth": {"bearer": BEARER}}), &env).unwrap();
    assert!(!format!("{conf:?}").contains(BEARER));
    let conf =
        HttpServerConf::from_json(&json!({"auth": {"hmac": {"secret": "hmac-shown?"}}}), &env)
            .unwrap();
    assert!(!format!("{conf:?}").contains("hmac-shown?"));
    for bad in [
        json!({"auth": {}}),
        json!({"auth": {"bearer": ""}}),
        json!({"auth": {"bearer": "a", "public": true}}),
        json!({"auth": {"hmac": {"secret": "s", "algorithm": "md5"}}}),
        json!({"auth": {"basic": "x"}}),
        json!({"max_body_bytes": 0}),
        json!({"max_body_bytes": 17 * 1024 * 1024}),
        json!({"max_in_flight": 0}),
        json!({"path": "/x/{id:[0-9]+}"}),
        json!({"path": "nope"}),
        json!({"cert_file": "/c.pem"}),
    ] {
        assert!(HttpServerConf::from_json(&bad, &env).is_err(), "{bad}");
    }
}

#[test]
fn defaults_are_bentos() {
    let conf = HttpServerConf::from_json(&json!({}), &Environment::new()).unwrap();
    assert_eq!(conf.path.as_str(), "/post");
    assert!(conf.allowed_verbs.contains("POST") && conf.allowed_verbs.len() == 1);
    assert_eq!(conf.timeout, Duration::from_secs(5));
    assert_eq!(conf.response_header_keys, ["Content-Type"]);
    assert!(!conf.prefix_metadata_keys);
    assert!(conf.auth.is_none());
}
