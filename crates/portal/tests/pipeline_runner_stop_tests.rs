//! A stream whose write fails can still be replaced and deleted (T-2983, PL-62).
//!
//! praha/waste-stations sat in `RunnerRefused` for half an hour: its upsert target failed, the
//! outcome broker retried the write inside the runner without end, and every PUT and DELETE
//! waited on a stream that could not stop until the runner was restarted.
//!
//! These tests run when `JC_PORTAL_TEST_BENTO_URL` points at the pinned Bento image in streams
//! mode (locally: `docker run -d -p 4195:4195 -e JC_CLIENT_ID=t -e JC_CLIENT_SECRET=t
//! -e JC_TOKEN_URL=http://127.0.0.1:9/token ghcr.io/warpstreamlabs/bento@sha256:656c55de…
//! streams`, the digest `components/pipeline-runner/images.yaml` pins); without it they say so
//! and return. The outcome sink reads its credential from those three variables and the runner
//! refuses a stream naming one it lacks; nothing answers at that token URL, so the sink fails
//! here, and a failing sink is dropped (PL-62).

use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// The Portal's own wait for the runner (`RUNNER_TIMEOUT` in src/reconciler/streams.rs).
const RUNNER_TIMEOUT: Duration = Duration::from_secs(30);

fn runner_url() -> Option<String> {
    std::env::var("JC_PORTAL_TEST_BENTO_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty())
}

/// A stream shaped like the rendered ones: a clock reading a page of records, and the write
/// followed by the outcome sink exactly as `with_outcomes` renders it.
fn stream(write_to: &str, sink_to: &str) -> Value {
    let write = json!({
        "label": "output",
        "http_client": { "url": write_to, "verb": "POST", "timeout": "30s", "drop_on": [400, 413, 422] }
    });
    json!({
        "input": { "label": "input", "generate": {
            "interval": "168h",
            "mapping": "root = range(0, 50).map_each(i -> {\"id\": i})"
        }},
        "pipeline": { "processors": [{ "label": "processor_0", "unarchive": { "format": "json_array" } }] },
        "output": joinedcontext_portal::pipeline_log::with_outcomes(write, sink_to),
    })
}

/// The runner's `input_received` for one stream: what reached a receiving stream.
async fn received(client: &reqwest::Client, runner: &str, stream: &str) -> u64 {
    let Ok(response) = client.get(format!("{runner}/metrics")).send().await else {
        return 0;
    };
    let body = response.text().await.unwrap_or_default();
    body.lines()
        .filter(|line| {
            line.starts_with("input_received{") && line.contains(&format!("stream=\"{stream}\""))
        })
        .filter_map(|line| line.rsplit_once(' ')?.1.parse::<f64>().ok())
        .sum::<f64>() as u64
}

#[tokio::test]
async fn a_stream_whose_write_fails_is_replaced_within_the_runner_timeout() {
    let Some(runner) = runner_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    };
    let client = reqwest::Client::builder()
        .timeout(RUNNER_TIMEOUT)
        .build()
        .expect("a client");
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_nanos();
    let (name, receiver) = (format!("t2983-{unique}"), format!("t2983rx-{unique}"));
    // Addresses as the runner dials them: nothing listens on its port 9, so every write there
    // is refused, and its own server on 4195 carries the receiving stream.
    let refused = "http://127.0.0.1:9";
    let receiver_url = format!("http://127.0.0.1:4195/{receiver}/post");

    let put = |body: Value, path: String| {
        let client = client.clone();
        async move { client.post(path).json(&body).send().await }
    };
    let created = put(
        stream(&format!("{refused}/write"), &format!("{refused}/sink")),
        format!("{runner}/streams/{name}"),
    )
    .await
    .expect("the runner answers");
    assert!(created.status().is_success(), "{}", created.status());
    // Let the write fail and go to retrying.
    tokio::time::sleep(Duration::from_secs(5)).await;

    let catcher =
        json!({ "input": { "http_server": { "path": "/post" } }, "output": { "drop": {} } });
    let up = put(catcher, format!("{runner}/streams/{receiver}"))
        .await
        .expect("the runner answers");
    assert!(up.status().is_success(), "{}", up.status());

    // The Portal's change: the same stream, now writing somewhere that takes it.
    let started = Instant::now();
    let replaced = client
        .put(format!("{runner}/streams/{name}"))
        .json(&stream(&receiver_url, &receiver_url))
        .send()
        .await;
    let took = started.elapsed();
    let replaced = replaced.unwrap_or_else(|err| {
        panic!("the runner did not replace the failing stream within {RUNNER_TIMEOUT:?}: {err}")
    });
    assert!(replaced.status().is_success(), "{}", replaced.status());
    assert!(took < RUNNER_TIMEOUT, "{took:?}");

    // The replaced stream writes every record once. Its sink cannot get a token and is dropped,
    // which neither holds the write back nor sends it again.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut got = 0;
    while Instant::now() < deadline && got < 50 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        got = received(&client, &runner, &receiver).await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        received(&client, &runner, &receiver).await,
        50,
        "every record written once"
    );

    for stream in [&name, &receiver] {
        let gone = client
            .delete(format!("{runner}/streams/{stream}"))
            .send()
            .await
            .expect("deleted in time");
        assert!(gone.status().is_success(), "{}", gone.status());
    }
}

#[tokio::test]
async fn a_stream_whose_write_fails_is_deleted_within_the_runner_timeout() {
    let Some(runner) = runner_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_BENTO_URL is not set");
        return;
    };
    let client = reqwest::Client::builder()
        .timeout(RUNNER_TIMEOUT)
        .build()
        .expect("a client");
    let name = format!(
        "t2983del-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after 1970")
            .as_nanos()
    );
    let refused = "http://127.0.0.1:9";
    let created = client
        .post(format!("{runner}/streams/{name}"))
        .json(&stream(
            &format!("{refused}/write"),
            &format!("{refused}/sink"),
        ))
        .send()
        .await
        .expect("the runner answers");
    assert!(created.status().is_success(), "{}", created.status());
    tokio::time::sleep(Duration::from_secs(5)).await;

    let gone = client
        .delete(format!("{runner}/streams/{name}"))
        .send()
        .await
        .unwrap_or_else(|err| {
            panic!("the failing stream was not deleted within {RUNNER_TIMEOUT:?}: {err}")
        });
    assert!(gone.status().is_success(), "{}", gone.status());
}
