use super::*;
use crate::Kind;
use axum::body::Body;
use jc_door::testing::Issuer;
use serde_json::{Value, json};
use std::io::Write;
use tower::ServiceExt;

const SECRET_TEXT: &str = "rodne-cislo-8505051234-tajne";

struct World {
    a: Issuer,
    b: Issuer,
    service: Arc<Service>,
}

fn deployment(name: &str, issuer: &Issuer, client: &str, per_minute: u32) -> DeploymentConfig {
    DeploymentConfig {
        name: name.into(),
        issuer: issuer.issuer.clone(),
        jwks_url: issuer.jwks_url.clone(),
        clients: vec![client.into()],
        max_texts_per_minute: per_minute,
    }
}

async fn world(limits: Limits, per_minute: u32, millis: u64) -> World {
    logs();
    let a = Issuer::start("https://id.a.example/realms/a", "a1").await;
    let b = Issuer::start("https://id.b.example/realms/b", "b1").await;
    let cfg = Config {
        listen_addr: default_listen(),
        metrics_addr: None,
        model_path: PathBuf::from("/nonexistent"),
        threads: 1,
        limits,
        deployments: vec![
            deployment("a", &a, "assistant-a", per_minute),
            deployment("b", &b, "assistant-b", per_minute),
        ],
    };
    let service = Arc::new(Service::new(Embedder::fake(millis), &cfg).expect("service"));
    World { a, b, service }
}

fn body(kind: &str, input: &[&str]) -> Body {
    Body::from(json!({"model": MODEL, "kind": kind, "input": input}).to_string())
}

fn request(token: Option<&str>, body: Body) -> Request {
    let mut req = Request::builder()
        .method("POST")
        .uri("/v1/embeddings")
        .header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    req.body(body).expect("request")
}

async fn call(service: &Arc<Service>, req: Request) -> (StatusCode, Value, Option<String>) {
    let resp = router(Arc::clone(service))
        .oneshot(req)
        .await
        .expect("answer");
    let status = resp.status();
    let retry = resp
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 24)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        retry,
    )
}

fn vectors(answer: &Value) -> Vec<Vec<f64>> {
    answer["data"]
        .as_array()
        .expect("data")
        .iter()
        .map(|e| {
            e["embedding"]
                .as_array()
                .expect("embedding")
                .iter()
                .map(|v| v.as_f64().expect("number"))
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn an_admitted_deployment_gets_a_vector_per_text_and_the_model_hash() {
    let w = world(Limits::default(), 100, 0).await;
    let token = w.a.token("assistant-a", AUDIENCE, 300);
    let (status, answer, _) = call(
        &w.service,
        request(Some(&token), body("passage", &["one", "two"])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["model"], MODEL);
    assert_eq!(answer["model_sha256"], MODEL_SHA256);
    let got = vectors(&answer);
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|v| v.len() == crate::DIMENSIONS));
    assert_ne!(got[0], got[1]);
}

#[tokio::test]
async fn no_token_wrong_audience_expired_and_another_deployments_client_are_refused() {
    let w = world(Limits::default(), 100, 0).await;
    let stranger = Issuer::start("https://id.c.example/realms/c", "c1").await;
    let refused = [
        ("no token", None),
        ("not a JWT", Some("abc.def.ghi".to_owned())),
        (
            "wrong audience",
            Some(w.a.token("assistant-a", "shako", 300)),
        ),
        ("expired", Some(w.a.token("assistant-a", AUDIENCE, -120))),
        (
            "an issuer no deployment names",
            Some(stranger.token("assistant-a", AUDIENCE, 300)),
        ),
        (
            "deployment b's kaisatsu signing a's client",
            Some(w.b.token("assistant-a", AUDIENCE, 300)),
        ),
        (
            "deployment a's kaisatsu signing b's client",
            Some(w.a.token("assistant-b", AUDIENCE, 300)),
        ),
    ];
    for (what, token) in refused {
        let (status, answer, _) =
            call(&w.service, request(token.as_deref(), body("query", &["q"]))).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{what}: {answer}");
    }
}

#[tokio::test]
async fn the_quota_is_per_deployment_and_refuses_past_it() {
    let w = world(Limits::default(), 3, 0).await;
    let a = w.a.token("assistant-a", AUDIENCE, 300);
    let b = w.b.token("assistant-b", AUDIENCE, 300);
    let (status, ..) = call(&w.service, request(Some(&a), body("passage", &["1", "2"]))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, answer, retry) =
        call(&w.service, request(Some(&a), body("passage", &["3", "4"]))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{answer}");
    assert!(
        retry
            .and_then(|r| r.parse::<u64>().ok())
            .is_some_and(|s| (1..=60).contains(&s)),
        "Retry-After in seconds"
    );
    // b's minute is its own.
    let (status, ..) = call(
        &w.service,
        request(Some(&b), body("passage", &["1", "2", "3"])),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // a may still use what is left of its own.
    let (status, ..) = call(&w.service, request(Some(&a), body("query", &["5"]))).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn oversized_batches_texts_and_bodies_and_other_models_are_refused() {
    let limits = Limits {
        max_texts: 2,
        max_text_bytes: 16,
        ..Limits::default()
    };
    let w = world(limits, 100, 0).await;
    let a = w.a.token("assistant-a", AUDIENCE, 300);
    let long = "x".repeat(17);
    for (what, req_body, want) in [
        (
            "three texts",
            body("passage", &["1", "2", "3"]),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            "no text",
            body("passage", &[]),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            "a text over the cap",
            body("passage", &["ok", &long]),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            "a body over the cap",
            Body::from(vec![b' '; 2 * (16 * 6 + 8) + 1025]),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
        (
            "another model",
            Body::from(json!({"model": "bge-m3", "kind": "query", "input": ["q"]}).to_string()),
            StatusCode::BAD_REQUEST,
        ),
        (
            "an unknown kind",
            Body::from(json!({"model": MODEL, "kind": "image", "input": ["q"]}).to_string()),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, answer, _) = call(&w.service, request(Some(&a), req_body)).await;
        assert_eq!(status, want, "{what}: {answer}");
    }
    // Refusals before the quota cost nothing: two texts still fit a minute of 100 after all that.
    let (status, ..) = call(&w.service, request(Some(&a), body("passage", &["1", "2"]))).await;
    assert_eq!(status, StatusCode::OK);
}

/// A writer the test's subscriber logs into.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("lock").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Every log line of this binary's tests. One subscriber for the whole process: a test without
/// one would make tracing cache the server's call sites as off for every other test (T-5633).
fn logs() -> &'static Captured {
    static LOGS: std::sync::OnceLock<Captured> = std::sync::OnceLock::new();
    LOGS.get_or_init(|| {
        let captured = Captured::default();
        let sink = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || sink.clone())
            .finish();
        tracing::subscriber::set_global_default(subscriber).expect("one subscriber");
        captured
    })
}

#[tokio::test]
async fn no_text_reaches_a_log_line_or_an_error_body() {
    let limits = Limits {
        max_texts: 2,
        max_text_bytes: 64,
        ..Limits::default()
    };
    let w = world(limits, 4, 0).await;
    let a = w.a.token("assistant-a", AUDIENCE, 300);
    let long = format!("{SECRET_TEXT}{}", "y".repeat(64));
    let mut bodies = Vec::new();
    for req_body in [
        body("passage", &[SECRET_TEXT]),
        body("passage", &[SECRET_TEXT, SECRET_TEXT, SECRET_TEXT]),
        body("passage", &[&long]),
        // serde_json would quote this value in its error.
        Body::from(
            json!({"model": MODEL, "kind": SECRET_TEXT, "input": [SECRET_TEXT]}).to_string(),
        ),
        Body::from(
            json!({"model": SECRET_TEXT, "kind": "query", "input": [SECRET_TEXT]}).to_string(),
        ),
        Body::from(json!({"model": MODEL, "kind": "query", "input": [7, SECRET_TEXT]}).to_string()),
        body("query", &[SECRET_TEXT, SECRET_TEXT]),
        body("query", &[SECRET_TEXT, SECRET_TEXT]),
    ] {
        let (status, answer, _) = call(&w.service, request(Some(&a), req_body)).await;
        if !status.is_success() {
            bodies.push(answer.to_string());
        }
    }
    let (status, answer, _) = call(&w.service, request(None, body("query", &[SECRET_TEXT]))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    bodies.push(answer.to_string());
    let logs = String::from_utf8(logs().0.lock().expect("lock").clone()).expect("utf-8");
    assert!(bodies.len() >= 6, "the refusals: {bodies:?}");
    assert!(
        logs.contains("refused") && logs.contains("embedded"),
        "{logs}"
    );
    for text in bodies.iter().chain([&logs]) {
        assert!(!text.contains("rodne-cislo"), "a text leaked: {text}");
    }
}

#[tokio::test]
async fn a_stalled_caller_blocks_nobody_and_gives_its_slot_back() {
    let limits = Limits {
        max_in_flight: 1,
        timeout_secs: 1,
        ..Limits::default()
    };
    let w = world(limits, 1000, 0).await;
    let a = w.a.token("assistant-a", AUDIENCE, 300);
    let b = w.b.token("assistant-b", AUDIENCE, 300);
    // a's one slot: a body that starts and never ends.
    let stalled = || {
        let head = futures_util::stream::iter([Ok::<_, std::io::Error>(
            axum::body::Bytes::from_static(b"{\"model\":"),
        )]);
        Body::from_stream(futures_util::StreamExt::chain(
            head,
            futures_util::stream::pending(),
        ))
    };
    let service = Arc::clone(&w.service);
    let token = a.clone();
    let stuck = tokio::spawn(async move { call(&service, request(Some(&token), stalled())).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    // b is not held up by it.
    let began = Instant::now();
    let (status, ..) = call(&w.service, request(Some(&b), body("query", &["q"]))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(began.elapsed() < Duration::from_millis(500));
    // a's second request while the first holds its slot: refused at once, not queued.
    let (status, _, retry) = call(&w.service, request(Some(&a), body("query", &["q"]))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(retry.as_deref(), Some("1"));
    // The stalled one runs out of time and gives the slot back.
    let (status, ..) = stuck.await.expect("stalled call");
    assert_eq!(status, StatusCode::REQUEST_TIMEOUT);
    let (status, ..) = call(&w.service, request(Some(&a), body("query", &["q"]))).await;
    assert_eq!(status, StatusCode::OK);

    // A caller that drops its connection mid-body gives the slot back too.
    let service = Arc::clone(&w.service);
    let token = a.clone();
    let dropped =
        tokio::spawn(async move { call(&service, request(Some(&token), stalled())).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    dropped.abort();
    let _ = dropped.await;
    let (status, ..) = call(&w.service, request(Some(&a), body("query", &["q"]))).await;
    assert_eq!(status, StatusCode::OK, "the aborted request's slot is free");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_deployments_embed_at_once_with_the_same_vectors() {
    let w = world(Limits::default(), 1000, 20).await;
    let a = w.a.token("assistant-a", AUDIENCE, 300);
    let b = w.b.token("assistant-b", AUDIENCE, 300);
    let texts = ["Knižnica je otvorená", "Odpad vyvážame v utorok", "Bussit"];
    let (ra, rb) = tokio::join!(
        call(&w.service, request(Some(&a), body("passage", &texts))),
        call(&w.service, request(Some(&b), body("passage", &texts))),
    );
    assert_eq!(ra.0, StatusCode::OK);
    assert_eq!(rb.0, StatusCode::OK);
    assert_eq!(vectors(&ra.1), vectors(&rb.1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_question_of_one_deployment_goes_before_anothers_batch_of_passages() {
    let w = world(Limits::default(), 1000, 50).await;
    let a = w.a.token("assistant-a", AUDIENCE, 300);
    let b = w.b.token("assistant-b", AUDIENCE, 300);
    let service = Arc::clone(&w.service);
    let indexing = tokio::spawn(async move {
        let passages: Vec<String> = (0..20).map(|i| format!("passage {i}")).collect();
        let refs: Vec<&str> = passages.iter().map(String::as_str).collect();
        let began = Instant::now();
        let r = call(&service, request(Some(&a), body("passage", &refs))).await;
        (r.0, began.elapsed())
    });
    tokio::time::sleep(Duration::from_millis(120)).await;
    let began = Instant::now();
    let (status, ..) = call(&w.service, request(Some(&b), body("query", &["q"]))).await;
    let asked = began.elapsed();
    assert_eq!(status, StatusCode::OK);
    // At most the one passage running, then the question: ~100 ms, never the ~900 ms left.
    assert!(
        asked < Duration::from_millis(400),
        "the question took {asked:?}"
    );
    let (status, took) = indexing.await.expect("indexing");
    assert_eq!(status, StatusCode::OK);
    assert!(took >= Duration::from_millis(1000));
}

#[test]
fn a_config_without_deployments_or_with_a_zero_limit_is_refused() {
    let base = r#"
model_path = "/models/e5-small"
[[deployments]]
name = "dev"
issuer = "https://id.dev.example/realms/dev"
jwks_url = "https://id.dev.example/realms/dev/certs"
clients = ["jc-assistant"]
max_texts_per_minute = 600
"#;
    let ok: Config = toml::from_str(base).expect("parses");
    ok.check().expect("valid");
    assert_eq!(ok.limits.max_texts, 64);
    let none: Config = toml::from_str("model_path = \"/m\"\ndeployments = []").expect("parses");
    assert!(none.check().is_err());
    let zero: Config =
        toml::from_str(&format!("{base}\n[limits]\nmax_in_flight = 0\n")).expect("parses");
    assert!(zero.check().is_err());
    let twice: Config = toml::from_str(&format!(
        "{base}\n[[deployments]]\nname = \"dev\"\nissuer = \"x\"\njwks_url = \"https://x\"\nclients = [\"c\"]\nmax_texts_per_minute = 1\n"
    ))
    .expect("parses");
    assert!(twice.check().is_err(), "two deployments named dev");
    assert!(toml::from_str::<Config>(&format!("{base}\ntypo = 1\n")).is_err());
}

#[test]
fn the_kind_reads_lowercase() {
    assert_eq!(
        serde_json::from_str::<Kind>("\"query\"").expect("query"),
        Kind::Query
    );
    assert!(serde_json::from_str::<Kind>("\"Query\"").is_err());
}
