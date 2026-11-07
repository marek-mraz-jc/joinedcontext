//! The `expiresAt` the output mapper writes is Bloblang the runner runs (T-4822, TS 104 175 4.22).
//!
//! The UI generates the bus recipe (observedAt plus 10 minutes) as `tests/fixtures/bus-expires.blobl`,
//! which `ui/tests/retention_expiry.test.ts` holds equal to its generator. This runs that step in
//! the pinned Bento in streams mode when `JC_PORTAL_TEST_BENTO_URL` names it, as
//! `endpoint_source_paging_tests` do, and checks the entity it builds; without it the test says so.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const STEP: &str = include_str!("fixtures/bus-expires.blobl");

fn runner_url() -> Option<String> {
    std::env::var("JC_PORTAL_TEST_BENTO_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty())
}

/// The entity the step builds from `record`, as the runner posts it to a sink.
async fn built(runner: &str, record: Value) -> Value {
    let sink = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sink"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&sink)
        .await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("a client");
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let name = format!(
        "t4822-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let stream = json!({
        "input": { "generate": { "count": 1, "interval": "", "mapping": format!("root = {record}") } },
        "pipeline": { "processors": [{ "mapping": STEP }] },
        "output": { "http_client": { "url": format!("{}/sink", sink.uri()), "verb": "POST" } }
    });
    let created = client
        .post(format!("{runner}/streams/{name}"))
        .json(&stream)
        .send()
        .await
        .expect("the runner answers");
    let status = created.status();
    let said = created.text().await.unwrap_or_default();
    assert!(
        status.is_success(),
        "the runner refused the step: {status} {said}"
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = Value::Null;
    while Instant::now() < deadline && seen.is_null() {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let posts: Vec<Request> = sink.received_requests().await.unwrap_or_default();
        if let Some(post) = posts.first() {
            seen = serde_json::from_slice(&post.body).expect("JSON");
        }
    }
    let _ = client
        .delete(format!("{runner}/streams/{name}"))
        .send()
        .await;
    seen
}

#[tokio::test]
async fn the_bus_recipe_writes_observed_at_plus_ten_minutes_as_expires_at() {
    let Some(runner) = runner_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    };
    let entity = built(
        &runner,
        json!({ "vehicle": "bus 12", "speed": "31", "observedAt": "2026-10-20T10:00:00+02:00" }),
    )
    .await;
    assert_eq!(entity["type"], "Vehicle", "{entity}");
    assert!(
        entity["id"]
            .as_str()
            .is_some_and(|id| id.ends_with(":transport:bus-12")),
        "{entity}"
    );
    assert_eq!(entity["speed"], json!({ "type": "Property", "value": 31 }));
    assert_eq!(entity["expiresAt"], "2026-10-20T08:10:00Z", "{entity}");
}

#[tokio::test]
async fn a_record_without_the_time_gets_no_expires_at() {
    let Some(runner) = runner_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    };
    let entity = built(&runner, json!({ "vehicle": "7", "speed": "0" })).await;
    assert_eq!(entity["type"], "Vehicle", "{entity}");
    assert!(entity.get("expiresAt").is_none(), "{entity}");
}
