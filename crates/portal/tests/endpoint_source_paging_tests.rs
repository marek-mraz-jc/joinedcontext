//! An endpoint source is read page by page (T-3132, PL-31).
//!
//! bbsk's indicators read `StatisticalObservation` through `bbsk-kraj`, 7,742 entities, as one
//! page of 1000: neither cube they compute was in it, and all 28 read "not measured". The read
//! the Portal renders asks the total with the first page, reads every further page, and hands the
//! compute one list; past the ceiling, or a full page with no total, the read fails.
//!
//! Run against the pinned Bento in streams mode when `JC_PORTAL_TEST_BENTO_URL` names it, with
//! the container on the host network so it reaches this test's gateway (locally: `docker run -d
//! --network host -e JC_CLIENT_ID=t -e JC_CLIENT_SECRET=t -e JC_TOKEN_URL=http://127.0.0.1:9/token
//! ghcr.io/warpstreamlabs/bento@sha256:656c55de… streams`); without it the tests say so and
//! return, as `pipeline_runner_stop_tests` do.

use std::time::{Duration, Instant};

use jc_core::kinds::pipeline::PipelineSpec;
use joinedcontext_portal::reconciler::streams::render_endpoint_stream;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const ENTITIES: &str = "/api/endpoint/kraj/ngsi-ld/v1/entities";

fn runner_url() -> Option<String> {
    std::env::var("JC_PORTAL_TEST_BENTO_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty())
}

fn rows(from: usize, to: usize) -> Value {
    Value::Array(
        (from..to)
            .map(|n| json!({ "id": format!("urn:ngsi-ld:StatisticalObservation:bbsk.sk:kraj:{n}"), "type": "StatisticalObservation", "n": n }))
            .collect(),
    )
}

/// A gateway holding `total` entities, in pages of 1000; it states the total when `stated`.
async fn gateway(total: usize, stated: bool) -> MockServer {
    let server = MockServer::start().await;
    let first = ResponseTemplate::new(200).set_body_json(rows(0, total.min(1000)));
    let first = if stated {
        first.insert_header("NGSILD-Results-Count", total.to_string().as_str())
    } else {
        first
    };
    Mock::given(method("GET"))
        .and(path(ENTITIES))
        .and(query_param("count", "true"))
        .and(query_param_is_missing("offset"))
        .respond_with(first)
        .mount(&server)
        .await;
    for offset in (1000..total).step_by(1000) {
        Mock::given(method("GET"))
            .and(path(ENTITIES))
            .and(query_param("offset", offset.to_string().as_str()))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(rows(offset, (offset + 1000).min(total))),
            )
            .mount(&server)
            .await;
    }
    Mock::given(method("POST"))
        .and(path("/sink"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    server
}

/// The read as the Portal renders it for a public source, followed by a count of what the
/// compute would see, written to the gateway's `/sink`.
fn stream(gateway: &str) -> Value {
    let spec: PipelineSpec = serde_json::from_value(json!({
        "class": "scheduled",
        "period": "168h",
        "source": { "endpointRef": { "kind": "Endpoint", "name": "bbsk-kraj" },
                    "query": { "type": "StatisticalObservation" } },
        "targetEndpoint": "urn:ngsi-ld:Endpoint:bbsk.sk:bbsk-kpi:bbsk-kpi",
        "compute": { "kind": "bloblang", "bloblang": "root = this" }
    }))
    .expect("a pipeline");
    let rendered = render_endpoint_stream(&spec, "bbsk", "bbsk/test", "kraj", true, "kpi")
        .expect("rendered")
        .to_string()
        .replace("${JC_GATEWAY_URL}", gateway);
    let rendered: Value = serde_json::from_str(&rendered).expect("JSON");
    let read = rendered["pipeline"]["processors"]
        .as_array()
        .expect("processors")[..2]
        .to_vec();
    let mut processors = read;
    processors.push(json!({ "mapping": "root = { \"n\": this.length(), \"distinct\": this.map_each(r -> r.n).unique().length() }" }));
    json!({
        "input": { "generate": { "count": 1, "interval": "", "mapping": "root = \"\"" } },
        "pipeline": { "processors": processors },
        "output": { "http_client": { "url": format!("{gateway}/sink"), "verb": "POST" } }
    })
}

/// What reached the sink, waiting up to 20 s; `None` when nothing did.
async fn sunk(
    server: &MockServer,
    client: &reqwest::Client,
    runner: &str,
    name: &str,
) -> Option<Value> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen = None;
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let posts: Vec<Request> = server
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|request| request.url.path() == "/sink")
            .collect();
        if let Some(post) = posts.first() {
            seen = serde_json::from_slice(&post.body).ok();
            break;
        }
    }
    let _ = client
        .delete(format!("{runner}/streams/{name}"))
        .send()
        .await;
    seen
}

async fn run(total: usize, stated: bool) -> (Option<Value>, usize) {
    let runner = runner_url().expect("checked by the caller");
    let server = gateway(total, stated).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("a client");
    // The two tests run in parallel against one runner, and the clock can hand them the same
    // nanosecond: the process and a counter keep their stream names apart (a duplicate is a 400).
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let name = format!(
        "t3132-{}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after 1970")
            .as_nanos()
    );
    let created = client
        .post(format!("{runner}/streams/{name}"))
        .json(&stream(&server.uri()))
        .send()
        .await
        .expect("the runner answers");
    assert!(created.status().is_success(), "{}", created.status());
    let seen = sunk(&server, &client, &runner, &name).await;
    let reads = server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.url.path() == ENTITIES)
        .count();
    (seen, reads)
}

#[tokio::test]
async fn every_page_of_a_large_source_reaches_the_compute_once() {
    if runner_url().is_none() {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    }
    let (seen, reads) = run(2_500, true).await;
    assert_eq!(seen, Some(json!({ "n": 2500, "distinct": 2500 })));
    assert_eq!(reads, 3, "one request per page");
    // A source within one page is one request, as before.
    let (seen, reads) = run(999, true).await;
    assert_eq!(seen, Some(json!({ "n": 999, "distinct": 999 })));
    assert_eq!(reads, 1);
}

#[tokio::test]
async fn a_full_page_without_a_total_and_a_source_past_the_ceiling_fail_the_read() {
    if runner_url().is_none() {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    }
    // A full page and no stated total: one page would be a cut list, so nothing goes on.
    let (seen, reads) = run(1_000, false).await;
    assert_eq!(seen, None);
    assert_eq!(reads, 1);
    // Past the ceiling: the first page is read, nothing further, nothing goes on.
    let (seen, reads) = run(60_000, true).await;
    assert_eq!(seen, None);
    assert_eq!(reads, 1);
}
