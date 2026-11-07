//! A run whose write fails ends failed with the target's words instead of replaying it until the
//! run's timeout (T-4952, PL-78).
//!
//! The BBSK events pipelines ran on dev for 15 minutes and were stopped with `sent 0, failed 0`:
//! every source with records wrote to a target that kept refusing, the stream's output nacked
//! the message back to its input and Bento replayed it without end, so the run never went
//! inactive and the refusal reached no log. The two sources with an empty feed ended in 5 s.
//! Runs against the pinned Bento (see pipeline_runner_stop_tests.rs for how it is started); the
//! source, the target and the Portal's outcome route are mocks in this process.

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
type HeaderBytes = Arc<Mutex<Vec<usize>>>;

/// The source (one record), the target (`/refuses` answers 503, `/takes` 204) and the outcome
/// route; the count of writes the target saw and the header bytes of each outcome post.
async fn mocks() -> (String, Reports, Arc<Mutex<u32>>, HeaderBytes) {
    let reports: Reports = Arc::default();
    let header_bytes: HeaderBytes = Arc::default();
    let measured = Arc::clone(&header_bytes);
    let writes: Arc<Mutex<u32>> = Arc::default();
    let kept = Arc::clone(&reports);
    let (refused, taken) = (Arc::clone(&writes), Arc::clone(&writes));
    let app = Router::new()
        .route(
            "/events",
            get(|| async { "{\"id\":\"urn:ngsi-ld:Event:bbsk:1\",\"type\":\"Event\"}" }),
        )
        .route(
            "/refuses",
            post(move || {
                let refused = Arc::clone(&refused);
                async move {
                    *refused.lock().expect("the count") += 1;
                    (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        "{\"title\":\"The context broker is not reachable\"}",
                    )
                }
            }),
        )
        .route(
            "/takes",
            post(move || {
                let taken = Arc::clone(&taken);
                async move {
                    *taken.lock().expect("the count") += 1;
                    axum::http::StatusCode::NO_CONTENT
                }
            }),
        )
        .route(
            "/outcomes",
            post(
                move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                    let kept = Arc::clone(&kept);
                    let measured = Arc::clone(&measured);
                    async move {
                        let bytes = headers
                            .iter()
                            .map(|(name, value)| name.as_str().len() + value.len())
                            .sum();
                        measured.lock().expect("the header sizes").push(bytes);
                        if let Ok(report) = serde_json::from_slice(&body) {
                            kept.lock().expect("the reports").push(report);
                        }
                        axum::http::StatusCode::NO_CONTENT
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("the address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}"), reports, writes, header_bytes)
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

struct Ran {
    reports: Vec<Value>,
    writes: u32,
    ended: bool,
    header_bytes: Vec<usize>,
}

/// One run of a stream that writes its record to `target`, shaped as the Portal renders a
/// pipeline: the write in the pipeline, an output that rejects an errored message. `metadata`
/// is the size of a metadata value the message carries into its write.
async fn run(runner: &str, target: &str, metadata: usize) -> Ran {
    let (base, reports, writes, header_bytes) = mocks().await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("a client");
    let outcomes = format!("{base}/outcomes");
    let rendered = json!({
        "input": { "label": "input", "http_client": { "url": format!("{base}/events"), "verb": "GET" } },
        "pipeline": { "processors": [
            { "label": "meta_0", "mutation": format!("meta jc_refused_big = \"{}\"", "x".repeat(metadata)) },
            { "label": "write_0", "branch": {
            "processors": [{ "http": {
                "url": format!("{base}{target}"),
                "verb": "POST",
                "retries": 1,
                "retry_period": "100ms",
                "drop_on": [400, 413, 422],
            }}],
            "result_map": "meta jc_written = content().length()",
        }}]},
        "output": joinedcontext_portal::pipeline_log::with_outcomes(
            json!({ "switch": { "retry_until_success": false, "cases": [
                { "check": "errored()", "output": { "reject": "${! error() }" } },
                { "output": { "drop": {} } },
            ]}}),
            &outcomes,
        ),
    });
    let mut stream = joinedcontext_portal::reconciler::run_queue::one_run(&rendered);
    without_token(&mut stream);
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let name = format!(
        "t4952-{}-{}",
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
    let mut ended = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let held: Value = match client.get(format!("{runner}/streams/{name}")).send().await {
            Ok(answer) => answer.json().await.unwrap_or_default(),
            Err(_) => continue,
        };
        if held.get("active").and_then(Value::as_bool) == Some(false) {
            ended = true;
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
    let writes = *writes.lock().expect("the count");
    let header_bytes = header_bytes.lock().expect("the header sizes").clone();
    Ran {
        reports: seen,
        writes,
        ended,
        header_bytes,
    }
}

fn lines<'a>(reports: &'a [Value], field: &str) -> Vec<&'a Value> {
    reports
        .iter()
        .flat_map(|report| report[field].as_array().into_iter().flatten())
        .collect()
}

#[tokio::test]
async fn a_write_the_target_keeps_refusing_ends_the_run_failed_with_its_words() {
    let Some(runner) = runner_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    };
    let ran = run(&runner, "/refuses", 0).await;
    assert!(ran.ended, "the run ends: {:?}", ran.reports);
    assert_eq!(ran.writes, 2, "the write and its one retry, never replayed");
    let failed = lines(&ran.reports, "failed");
    assert_eq!(failed.len(), 1, "one failed line: {:?}", ran.reports);
    assert_eq!(failed[0]["id"], "urn:ngsi-ld:Event:bbsk:1");
    let message = failed[0]["message"].as_str().unwrap_or_default();
    assert!(message.contains("503"), "{message}");
    assert!(lines(&ran.reports, "sent").is_empty(), "{:?}", ran.reports);
}

#[tokio::test]
async fn a_write_the_target_takes_ends_the_run_sent() {
    let Some(runner) = runner_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    };
    let ran = run(&runner, "/takes", 0).await;
    assert!(ran.ended, "the run ends: {:?}", ran.reports);
    assert_eq!(ran.writes, 1);
    assert_eq!(lines(&ran.reports, "sent").len(), 1, "{:?}", ran.reports);
    assert!(
        lines(&ran.reports, "failed").is_empty(),
        "{:?}",
        ran.reports
    );
}

/// T-5648: a refused write whose message carries 64 KB of metadata still reports; the sink sends
/// the refusal in its body and none of the metadata as a header (the Portal's mesh hop answers
/// 431 above its header limit, so a header copy of it would lose the outcome).
#[tokio::test]
async fn a_refusal_carrying_oversized_metadata_still_reports_in_the_body() {
    let Some(runner) = runner_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    };
    let ran = run(&runner, "/refuses", 64 * 1024).await;
    assert!(ran.ended, "the run ends: {:?}", ran.reports);
    let failed = lines(&ran.reports, "failed");
    assert_eq!(failed.len(), 1, "one failed line: {:?}", ran.reports);
    assert_eq!(failed[0]["id"], "urn:ngsi-ld:Event:bbsk:1");
    assert!(!ran.header_bytes.is_empty(), "the sink posted");
    assert!(
        ran.header_bytes.iter().all(|bytes| *bytes < 1024),
        "outcome headers stay small: {:?}",
        ran.header_bytes
    );
}
