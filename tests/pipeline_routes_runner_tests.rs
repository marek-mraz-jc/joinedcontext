//! Routes by condition run in the pinned Bento (PL-73, T-3920): each record lands at the outputs
//! of the first route whose `when` holds, after that route's steps; the rest at the default lane.
//!
//! Run against the pinned Bento in streams mode when `JC_PORTAL_TEST_BENTO_URL` names it, on the
//! host network so it reaches this test's gateway (see `endpoint_source_paging_tests`); without
//! it the test says so and returns.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use jc_core::kinds::pipeline::PipelineSpec;
use joinedcontext_portal::reconciler::streams::{render_merged, ResolvedSource};
use serde_json::{json, Value};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn runner_url() -> Option<String> {
    std::env::var("JC_PORTAL_TEST_BENTO_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty())
}

/// The gateway's credentials and the runner's rate limit are the cluster's; the test's gateway
/// takes the writes without them.
fn without_cluster_fields(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.remove("oauth2");
            fields.remove("rate_limit");
            fields.values_mut().for_each(without_cluster_fields);
        }
        Value::Array(items) => items.iter_mut().for_each(without_cluster_fields),
        _ => {}
    }
}

fn stream(gateway: &str) -> Value {
    let spec: PipelineSpec = serde_json::from_value(json!({
        "class": "resident",
        "period": "30s",
        "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "sensors" } }],
        "steps": [{ "kind": "bloblang", "bloblang": "root = this" }],
        "routes": [
            { "name": "hot", "when": "this.t > 30",
              "steps": [{ "kind": "bloblang", "bloblang": "root = this.merge({\"alert\": true})" }],
              "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:alerts:hot" }] },
            { "name": "frost-warning", "when": "this.t < 0 || this.id == \"d\"",
              "outputs": [
                  { "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:alerts:cold" },
                  { "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:kpi:cold-kpi" }
              ] }
        ],
        "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:example.org:air:lane" }]
    }))
    .expect("a routed pipeline");
    let sources = [ResolvedSource::Data {
        spec: Box::new(
            serde_json::from_value(
                json!({ "type": "http", "http": { "url": "https://example.org/air" } }),
            )
            .expect("a DataSource"),
        ),
        name: "sensors".to_owned(),
    }];
    let slugs = ["lane", "hot", "cold", "coldkpi"].map(str::to_owned);
    let rendered = render_merged(&spec, "air-routed", "helsinki", &sources, &slugs)
        .expect("rendered")
        .to_string()
        .replace("${JC_GATEWAY_URL}", gateway);
    let mut rendered: Value = serde_json::from_str(&rendered).expect("JSON");
    without_cluster_fields(&mut rendered);
    // One poll of five records; `d` meets both conditions and takes the first route.
    rendered["input"] = json!({ "generate": { "count": 1, "interval": "",
        "mapping": "root = [{\"id\":\"a\",\"t\":10},{\"id\":\"b\",\"t\":35},{\"id\":\"c\",\"t\":-5},{\"id\":\"d\",\"t\":50},{\"id\":\"e\",\"t\":20}]" } });
    rendered
}

/// The ids each Endpoint slug was sent, once five records have arrived or 20 s have passed.
async fn written(server: &MockServer) -> BTreeMap<String, Vec<Value>> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut by_slug: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for request in server.received_requests().await.unwrap_or_default() {
            let slug = request
                .url
                .path()
                .split('/')
                .nth(3)
                .unwrap_or_default()
                .to_owned();
            let records: Vec<Value> = serde_json::from_slice(&request.body).unwrap_or_default();
            by_slug.entry(slug).or_default().extend(records);
        }
        let count: usize = by_slug.values().map(Vec::len).sum();
        if count >= 6 || Instant::now() > deadline {
            return by_slug;
        }
    }
}

#[tokio::test]
async fn each_record_lands_at_the_outputs_of_its_route_after_its_steps() {
    let Some(runner) = runner_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("a client");
    let name = format!(
        "t3920-{}-{}",
        std::process::id(),
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
    let status = created.status();
    let body = created.text().await.unwrap_or_default();
    assert!(status.is_success(), "{status}: {body}");
    let by_slug = written(&server).await;
    let _ = client
        .delete(format!("{runner}/streams/{name}"))
        .send()
        .await;

    let ids = |slug: &str| -> Vec<&str> {
        let mut ids: Vec<&str> = by_slug
            .get(slug)
            .map(|records| records.iter().filter_map(|r| r["id"].as_str()).collect())
            .unwrap_or_default();
        ids.sort_unstable();
        ids
    };
    assert_eq!(ids("hot"), ["b", "d"], "{by_slug:?}");
    assert_eq!(ids("cold"), ["c"], "{by_slug:?}");
    assert_eq!(ids("coldkpi"), ["c"], "{by_slug:?}");
    assert_eq!(ids("lane"), ["a", "e"], "{by_slug:?}");
    assert!(by_slug["hot"].iter().all(|record| record["alert"] == true));
    assert!(by_slug["lane"]
        .iter()
        .all(|record| record.get("alert").is_none()));
}
