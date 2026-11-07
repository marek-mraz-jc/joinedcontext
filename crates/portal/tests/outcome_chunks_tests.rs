//! The outcome sink's merge, run in the pinned Bento (PL-62).
//!
//! Bento refuses an empty `range(0, 0)`, and the merge counted the refused records' chunks with
//! one: every run that refused nothing, the usual one, threw in `outcome_chunks` and reached the
//! Portal with no report (dev, 2026-10-16, after T-4056). The sink's own post needs a token the
//! test runner does not get, so the merge runs here alone, between a generated batch of reports
//! and a mock.
//!
//! Run against the pinned Bento in streams mode when `JC_PORTAL_TEST_BENTO_URL` names it, with the
//! container on the host network (see `endpoint_source_paging_tests`); without it the tests say so
//! and return.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn runner_url() -> Option<String> {
    std::env::var("JC_PORTAL_TEST_BENTO_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty())
}

/// The merge exactly as the reconciler renders it.
fn merge() -> String {
    let stream = joinedcontext_portal::pipeline_log::with_outcomes(
        json!({ "drop": {} }),
        "http://portal/outcomes",
    );
    stream["broker"]["outputs"][1]["fallback"][0]["http_client"]["batching"]["processors"][2]
        ["mapping"]
        .as_str()
        .expect("the merge")
        .to_owned()
}

/// What the merge makes of one run's gathered reports: the chunks the Portal would receive.
async fn chunks(reports: Value) -> Vec<Value> {
    let runner = runner_url().expect("checked by the caller");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sink"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let name = format!(
        "outcome-chunks-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let stream = json!({
        "input": { "generate": { "count": 1, "interval": "", "mapping": format!("root = {reports}") } },
        "pipeline": { "processors": [
            { "mapping": merge() },
            { "unarchive": { "format": "json_array" } }
        ]},
        "output": { "http_client": { "url": format!("{}/sink", server.uri()), "verb": "POST" } },
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("a client");
    let created = client
        .post(format!("{runner}/streams/{name}"))
        .json(&stream)
        .send()
        .await
        .expect("the runner answers");
    assert!(created.status().is_success(), "{}", created.status());
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        seen = server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter_map(|request| serde_json::from_slice(&request.body).ok())
            .collect();
        if !seen.is_empty() {
            // The batch is one message; give its other chunks a moment to follow.
            tokio::time::sleep(Duration::from_millis(500)).await;
            seen = server
                .received_requests()
                .await
                .unwrap_or_default()
                .iter()
                .filter_map(|request| serde_json::from_slice(&request.body).ok())
                .collect();
            break;
        }
    }
    let _ = client
        .delete(format!("{runner}/streams/{name}"))
        .send()
        .await;
    seen
}

#[tokio::test]
async fn a_run_that_refused_nothing_still_reports_what_it_wrote() {
    if runner_url().is_none() {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    }
    let seen = chunks(json!([
        { "run": "r1", "sent": ["urn:a", "urn:b"], "hashes": ["h1", "h2"], "failed": [] },
        { "run": "r1", "sent": ["urn:c"], "hashes": ["h3"] }
    ]))
    .await;
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0]["run"], "r1");
    assert_eq!(seen[0]["sent"], json!(["urn:a", "urn:b", "urn:c"]));
    assert_eq!(seen[0]["hashes"], json!(["h1", "h2", "h3"]));
    assert!(seen[0].get("failed").is_none(), "{seen:?}");
}

#[tokio::test]
async fn refused_records_come_as_a_chunk_of_their_own() {
    if runner_url().is_none() {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    }
    let seen = chunks(json!([
        { "run": "r2", "sent": ["urn:a"], "hashes": ["h1"] },
        { "run": "r2", "sent": [], "hashes": [], "failed": [{ "id": "urn:x", "message": "refused" }] }
    ]))
    .await;
    assert_eq!(seen.len(), 2, "{seen:?}");
    let failed: Vec<&Value> = seen
        .iter()
        .filter(|chunk| chunk.get("failed").is_some())
        .collect();
    assert_eq!(failed.len(), 1, "{seen:?}");
    assert_eq!(
        failed[0]["failed"],
        json!([{ "id": "urn:x", "message": "refused" }])
    );
    assert_eq!(failed[0]["sent"], json!([]));
}
