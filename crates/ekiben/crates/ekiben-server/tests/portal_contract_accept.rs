//! Contract with the Portal (PORTAL-CALLS.md): every stream the Portal renders for the
//! deployment seed is accepted by ekiben's runner API through the Portal's own calls, and the API
//! answers each call the way the Portal reads it. Nothing leaves the host: outside addresses are
//! pointed at a closed loopback port, which the outbound rule refuses anyway.

mod portal_contract_common;

use portal_contract_common::{cases, deploy, runner, ADMIN_TOKEN};
use reqwest::StatusCode;
use serde_json::{json, Value};

fn set_runner_env() {
    // What the deployment's runner environment holds, as `jc` reads it (EKIBEN_VAR_*).
    for (k, v) in [
        ("JC_GATEWAY_URL", "http://127.0.0.1:9/gw"),
        ("JC_PIPELINE_TOKEN_URL", "http://127.0.0.1:9/token"),
        ("JC_TOKEN_URL", "http://127.0.0.1:9/token"),
        ("JC_ORG_DOMAIN", "example.org"),
        ("JC_MQTT_NAMESPACE", "contract"),
    ] {
        std::env::set_var(format!("EKIBEN_VAR_{k}"), v);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_stream_the_portal_renders_for_the_seed_is_accepted() {
    set_runner_env();
    let (runner, _root) = runner(false).await;
    let client = reqwest::Client::new();
    let all = cases();
    assert!(all.len() >= 700, "the fixture holds the whole seed ({})", all.len());
    let mut prepared = Vec::new();
    for case in &all {
        let mut stream = case.stream.clone();
        portal_contract_common::rewrite(&mut stream, "http://127.0.0.1:9", "tcp://127.0.0.1:9");
        // A source secret the deployment writes into the runner's environment (DataSource
        // secretRef, reconciler `write_runner_secret`): defined here as it would be there.
        for var in env_vars(&stream) {
            std::env::set_var(format!("EKIBEN_VAR_{var}"), "contract-secret");
        }
        prepared.push((case, stream));
    }
    use futures::StreamExt as _;
    let refused: Vec<String> = futures::stream::iter(prepared)
        .map(|(case, stream)| {
            let (client, runner) = (client.clone(), runner.clone());
            async move {
                let created = deploy(&client, &runner, case, &stream).await;
                let r = client
                    .delete(format!("{runner}/streams/{}", case.id))
                    .bearer_auth(ADMIN_TOKEN)
                    .send()
                    .await
                    .unwrap();
                let deleted = r.status().is_success() || r.status() == StatusCode::NOT_FOUND;
                match (created, deleted) {
                    (Ok(()), true) => None,
                    (Ok(()), false) => Some(format!("{}: DELETE answered {}", case.id, r.status())),
                    (Err(why), _) => Some(format!("{}: {why}", case.id)),
                }
            }
        })
        .buffer_unordered(8)
        .filter_map(|r| async move { r })
        .collect()
        .await;
    assert!(refused.is_empty(), "{} of {} refused:\n{}", refused.len(), all.len(), refused.join("\n"));
}

/// The `${NAME}` variables a stream names outside Bloblang interpolations (`${! ... }`).
fn env_vars(stream: &Value) -> Vec<String> {
    let text = stream.to_string();
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("${") {
        rest = &rest[at + 2..];
        if rest.starts_with('!') {
            continue;
        }
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() && !name.starts_with("JC_") {
            out.push(name);
        }
    }
    out.sort();
    out.dedup();
    out
}

#[tokio::test]
async fn the_runner_api_answers_as_the_portal_reads_it() {
    set_runner_env();
    let (runner, _root) = runner(false).await;
    let client = reqwest::Client::new();
    let auth = |r: reqwest::RequestBuilder| r.bearer_auth(ADMIN_TOKEN);
    // A one-run stream as the run queue sends one (generate count 1), with a variable the
    // runner fills: GET must answer it with the placeholder kept (`held_as_rendered`).
    let stream = json!({
        "input": {"generate": {"count": 1, "interval": "", "mapping": "root = {\"id\": \"urn:ngsi-ld:T:1\"}"}, "label": "input"},
        "pipeline": {"processors": [{"mapping": "root = this.merge({\"at\": \"${JC_ORG_DOMAIN}\"})", "label": "processor_0"}], "threads": 1},
        "output": {"drop": {}, "label": "output"}
    });
    let url = format!("{runner}/streams/helsinki.contract");

    // The Portal sends PUT first; a runner that does not hold the stream answers 404.
    let r = auth(client.put(&url)).json(&stream).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let r = auth(client.post(&url)).json(&stream).send().await.unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    // Creating it again: 400 with exactly Bento's text (`api/pipeline_test.rs` `held()`).
    let r = auth(client.post(&url)).json(&stream).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    assert_eq!(r.text().await.unwrap(), "Stream already exists");

    // GET /streams: id -> {active}; GET /streams/{id}: active, uptime (f64), config as sent.
    let mut finished = false;
    for _ in 0..100 {
        let one: Value = auth(client.get(&url)).send().await.unwrap().json().await.unwrap();
        assert!(one["uptime"].as_f64().is_some(), "{one}");
        assert_eq!(one["config"]["pipeline"]["processors"][0]["mapping"], stream["pipeline"]["processors"][0]["mapping"]);
        if one["active"] == json!(false) {
            finished = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(finished, "a count-1 stream ends and reads as not active (the run queue's Finished)");
    let list: Value = auth(client.get(format!("{runner}/streams"))).send().await.unwrap().json().await.unwrap();
    assert_eq!(list["helsinki.contract"]["active"], json!(false), "{list}");

    // GET /metrics: Prometheus text with the stream label the Portal filters on.
    let metrics = auth(client.get(format!("{runner}/metrics"))).send().await.unwrap().text().await.unwrap();
    assert!(
        metrics.lines().any(|l| l.contains("input_received") && l.contains("stream=\"helsinki.contract\"")),
        "{metrics}"
    );
    assert!(metrics.lines().any(|l| l.contains("output_sent") && l.contains("stream=\"helsinki.contract\"")));

    // The write budget: 200; a second declaration replaces it.
    for _ in 0..2 {
        let r = auth(client.post(format!("{runner}/resources/rate_limit/helsinki__pipeline_egress")))
            .json(&json!({"local": {"count": 500, "interval": "1s"}}))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "{}", r.status());
    }

    // DELETE: 2xx, then 404 (gone), and GET 404 (the run queue's Gone).
    assert!(auth(client.delete(&url)).send().await.unwrap().status().is_success());
    assert_eq!(auth(client.delete(&url)).send().await.unwrap().status(), StatusCode::NOT_FOUND);
    assert_eq!(auth(client.get(&url)).send().await.unwrap().status(), StatusCode::NOT_FOUND);

    // A config the runner cannot build is a 4xx with a body (the Portal shows it as lint errors).
    let bad = json!({"input": {"generate": {"count": 1, "mapping": "root = 1"}}, "pipeline": {"processors": [{"no_such_processor": {}}]}, "output": {"drop": {}}});
    let r = auth(client.post(format!("{runner}/streams/helsinki.bad"))).json(&bad).send().await.unwrap();
    assert!(r.status().is_client_error(), "{}", r.status());
    assert!(!r.text().await.unwrap().is_empty());

    // Every call without the token is refused (jc's runner; the Portal must send it).
    assert_eq!(client.get(format!("{runner}/streams")).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
}
