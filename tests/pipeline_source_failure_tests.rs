//! A source that refuses the run's request ends the run failed with its words (T-4428, PL-78).
//!
//! slovensko/susr-zo-krajiny ran twice on dev against a source that answered `400 Too many
//! results!`; both runs reported `sent 0, failed 0`, because the request's error was dropped
//! with the message. `one_run` now reports it to the run's log as a `failed` line first. Runs
//! against the pinned Bento (see pipeline_runner_stop_tests.rs for how it is started); the
//! source and the Portal's outcome route are mocks in this process.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

fn runner_url() -> Option<String> {
    std::env::var("JC_PORTAL_TEST_BENTO_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty())
}

type Reports = Arc<Mutex<Vec<Value>>>;

/// The source (`/refuses` answers 400, `/empty` an empty list) and the outcome route.
async fn mocks() -> (String, Reports) {
    let reports: Reports = Arc::default();
    let kept = Arc::clone(&reports);
    let app = Router::new()
        .route(
            "/refuses",
            get(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    "{\"error\":\"Too many results!\"}",
                )
            }),
        )
        .route("/empty", get(|| async { "[]" }))
        .route(
            "/outcomes",
            post(move |body: axum::body::Bytes| {
                let kept = Arc::clone(&kept);
                async move {
                    if let Ok(report) = serde_json::from_slice(&body) {
                        kept.lock().expect("the reports").push(report);
                    }
                    axum::http::StatusCode::NO_CONTENT
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("the address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}"), reports)
}

/// The test runner holds no token: the report goes out without one, everything else as rendered.
fn without_token(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            if let Some(oauth) = fields.get_mut("oauth2").and_then(Value::as_object_mut) {
                oauth.insert("enabled".to_owned(), json!(false));
            }
            fields.values_mut().for_each(without_token);
        }
        Value::Array(items) => items.iter_mut().for_each(without_token),
        _ => {}
    }
}

/// Runs one pass of a stream reading `path` of the mock source; returns the reports it posted.
async fn run(runner: &str, path: &str) -> Vec<Value> {
    let (base, reports) = mocks().await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("a client");
    let outcomes = format!("{base}/outcomes");
    let rendered = json!({
        "input": { "label": "input", "http_client": {
            "url": format!("{base}{path}?lang=sk&key=abc123"),
            "verb": "GET",
        }},
        "output": joinedcontext_portal::pipeline_log::with_outcomes(json!({ "drop": {} }), &outcomes),
    });
    let mut stream = joinedcontext_portal::reconciler::run_queue::one_run(&rendered);
    without_token(&mut stream);
    // The two tests run at once: a clock read alone can name both streams alike on a coarse
    // clock, and the second POST is then refused with `Stream already exists` (T-4712).
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let name = format!(
        "t4428-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let created = client
        .post(format!("{runner}/streams/{name}"))
        .json(&stream)
        .send()
        .await
        .expect("the runner answers");
    let status = created.status();
    assert!(
        status.is_success(),
        "the runner takes the stream: {status} {}",
        created.text().await.unwrap_or_default()
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let held: Value = match client.get(format!("{runner}/streams/{name}")).send().await {
            Ok(answer) => answer.json().await.unwrap_or_default(),
            Err(_) => continue,
        };
        if held.get("active").and_then(Value::as_bool) == Some(false) {
            break;
        }
    }
    // The sink gathers for half a second before it sends.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let _ = client
        .delete(format!("{runner}/streams/{name}"))
        .send()
        .await;
    let seen = reports.lock().expect("the reports").clone();
    seen
}

#[tokio::test]
async fn a_source_that_answers_400_ends_the_run_failed_with_its_words() {
    let Some(runner) = runner_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    };
    let reports = run(&runner, "/refuses").await;
    let failed: Vec<&Value> = reports
        .iter()
        .flat_map(|report| report["failed"].as_array().into_iter().flatten())
        .collect();
    assert_eq!(failed.len(), 1, "one failed line: {reports:?}");
    let message = failed[0]["message"].as_str().unwrap_or_default();
    assert!(message.contains("400"), "{message}");
    assert!(message.contains("Too many results!"), "{message}");
    assert!(
        !message.contains("abc123") && !message.contains("lang=sk"),
        "no query: {message}"
    );
    assert!(
        reports.iter().all(|report| report["run"].is_string()),
        "{reports:?}"
    );
}

#[tokio::test]
async fn a_source_that_answers_200_with_nothing_ends_the_run_without_a_failure() {
    let Some(runner) = runner_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    };
    let reports = run(&runner, "/empty").await;
    assert!(
        reports
            .iter()
            .all(|report| report["failed"].as_array().is_none_or(Vec::is_empty)),
        "{reports:?}"
    );
}
