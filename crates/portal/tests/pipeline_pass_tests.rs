//! Every pass of a clocked pipeline is reported, and a report never costs the pass (T-3001, PL-62).
//!
//! helsinki/vehicles-reaper deletes through its own step and writes nothing through its output,
//! so its run list stayed empty for a day while it worked; air-quality looked dead whenever FMI
//! had nothing new. `with_passes` reports each tick before the read. The runner must take the
//! rendered stream, try the report on every tick, and carry on with the read when the report
//! fails, which is what happens here: the runner gets no token (see pipeline_runner_stop_tests.rs
//! for how the pinned Bento is started).

use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn runner_url() -> Option<String> {
    std::env::var("JC_PORTAL_TEST_BENTO_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty())
}

/// The sum of one counter for one stream, and one label when given.
async fn counter(
    client: &reqwest::Client,
    runner: &str,
    name: &str,
    stream: &str,
    label: Option<&str>,
) -> u64 {
    let Ok(response) = client.get(format!("{runner}/metrics")).send().await else {
        return 0;
    };
    let body = response.text().await.unwrap_or_default();
    body.lines()
        .filter(|line| {
            line.starts_with(&format!("{name}{{"))
                && line.contains(&format!("stream=\"{stream}\""))
                && label.is_none_or(|label| line.contains(&format!("label=\"{label}\"")))
        })
        .filter_map(|line| line.rsplit_once(' ')?.1.parse::<f64>().ok())
        .sum::<f64>() as u64
}

#[tokio::test]
async fn a_pass_whose_report_fails_still_reads_and_writes_every_tick() {
    let Some(runner) = runner_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("a client");
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_nanos();
    let (name, receiver) = (format!("t3001-{unique}"), format!("t3001rx-{unique}"));

    let catcher =
        json!({ "input": { "http_server": { "path": "/post" } }, "output": { "drop": {} } });
    let up = client
        .post(format!("{runner}/streams/{receiver}"))
        .json(&catcher)
        .send()
        .await
        .expect("the runner answers");
    assert!(up.status().is_success(), "{}", up.status());

    // A clocked stream as the reconciler renders one: the tick, the pass report, then the read.
    let mut stream: Value = json!({
        "input": { "label": "input", "generate": {
            "interval": "1s",
            "mapping": format!("root = \"\"\n{}", joinedcontext_portal::pipeline_log::TICK)
        }},
        // The read of an http DataSource: a failed read is dropped by the mutation after it, so a
        // report whose error were still on the tick would drop the tick too.
        "pipeline": { "processors": [
            { "try": [{ "mapping": "root = { \"id\": \"urn:x\", \"run\": @jc_run }" }] },
            { "mutation": "root = if errored() { deleted() }" }
        ]},
        "output": { "label": "output", "http_client": {
            "url": format!("http://127.0.0.1:4195/{receiver}/post"),
            "verb": "POST",
            "timeout": "5s"
        }},
    });
    // Nothing answers on port 9, and the runner gets no token either: every report fails.
    joinedcontext_portal::pipeline_log::with_passes(
        &mut stream,
        "http://127.0.0.1:9/internal/pipelines/helsinki/reaper/outcomes",
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
        "the runner takes the rendered stream: {status} {}",
        created.text().await.unwrap_or_default()
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut written = 0;
    while Instant::now() < deadline && written < 3 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        written = counter(&client, &runner, "input_received", &receiver, None).await;
    }
    let tried = counter(
        &client,
        &runner,
        "processor_received",
        &name,
        Some(joinedcontext_portal::pipeline_log::PASS_LABELS[0]),
    )
    .await;

    for stream in [&name, &receiver] {
        let _ = client
            .delete(format!("{runner}/streams/{stream}"))
            .send()
            .await;
    }
    assert!(
        written >= 3,
        "every tick still writes, the report failing: {written}"
    );
    assert!(tried >= 3, "every tick is reported: {tried}");
}
