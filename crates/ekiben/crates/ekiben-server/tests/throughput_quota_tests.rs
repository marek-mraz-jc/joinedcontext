//! Parallel pipeline workers, organization quotas that pace instead of failing, the admin quota
//! API, and a `messages_out` that counts the messages written (T-6646).

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::Duration;

use ekiben_core::{OrgId, OrgQuotas, ResourceManager, StreamConfig};
use ekiben_server::runner::spawn_stream;
use ekiben_server::{build_router, run_stream_once, ServerAuthConfig, ServerState, StreamManager};
use ekiben_testkit::full_registry;
use reqwest::StatusCode;
use tokio::net::TcpListener;

/// Runs `yaml` once with its file output under a temp root and returns the summary and the lines.
async fn run_to_file(yaml: &str) -> (ekiben_core::RunSummary, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let conf = StreamConfig::from_yaml_or_json(yaml).unwrap();
    let summary = run_stream_once(
        "job",
        conf,
        full_registry(),
        ResourceManager::new(),
        Some(dir.path().to_path_buf()),
        false,
    )
    .await
    .unwrap();
    let text = std::fs::read_to_string(dir.path().join("out.jsonl")).unwrap_or_default();
    (summary, text.lines().map(str::to_string).collect())
}

fn batched_generate(count: usize, threads: i64, procs: &str) -> String {
    format!(
        r#"
run: once
input:
  broker:
    inputs:
      - generate:
          count: {count}
          interval: ""
          mapping: 'root = {{"n": counter()}}'
    batching:
      count: 1000
      period: 1s
pipeline:
  threads: {threads}
  processors:
{procs}
output:
  file:
    path: out.jsonl
    codec: lines
"#
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn messages_out_counts_the_lines_written_after_archive_and_unarchive() {
    let procs = "    - archive:\n        format: json_array\n    - unarchive:\n        format: json_array\n";
    let (summary, lines) = run_to_file(&batched_generate(2_500, 1, procs)).await;
    assert!(summary.success, "{:?}", summary.error);
    assert_eq!(lines.len(), 2_500);
    assert_eq!(summary.messages_out, lines.len() as u64);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn default_quotas_never_stop_a_fast_run_through_many_processors() {
    // Each processor used to charge the message-rate quota again: 20 processors x 20,000
    // messages failed the run at the default 50,000 msg/s.
    let procs = "    - mapping: 'root = this'\n".repeat(20);
    let (summary, lines) = run_to_file(&batched_generate(20_000, -1, &procs)).await;
    assert!(summary.success, "{:?}", summary.error);
    assert_eq!(lines.len(), 20_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_message_is_written_once_with_parallel_workers() {
    let procs = "    - mutation: 'root.doubled = this.n * 2'\n";
    let (summary, lines) = run_to_file(&batched_generate(30_000, -1, procs)).await;
    assert!(summary.success, "{:?}", summary.error);
    let seen: HashSet<i64> = lines
        .iter()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["n"]
                .as_i64()
                .unwrap()
        })
        .collect();
    assert_eq!(lines.len(), 30_000);
    assert_eq!(seen.len(), 30_000, "a message was written twice or torn");
    assert_eq!(summary.messages_in, 30_000);
    assert_eq!(summary.messages_out, 30_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_output_keeps_the_input_order_with_any_thread_count() {
    let procs = "    - mapping: 'root = this'\n";
    for threads in [1, -1] {
        let (summary, lines) = run_to_file(&batched_generate(5_000, threads, procs)).await;
        assert!(summary.success, "{:?}", summary.error);
        let order: Vec<i64> = lines
            .iter()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).unwrap()["n"]
                    .as_i64()
                    .unwrap()
            })
            .collect();
        assert_eq!(order, (1..=5_000).collect::<Vec<_>>(), "threads {threads}");
    }
}

const ADMIN: &str = "Bearer admin-token-t6646";

async fn serve(resources: ResourceManager, admin: bool) -> String {
    let manager = StreamManager::new(full_registry(), resources, None, false);
    let auth = ServerAuthConfig {
        admin_bearer_token: admin.then(|| ADMIN.trim_start_matches("Bearer ").to_string()),
        org_tokens: vec![("org-a-token".to_string(), OrgId::new("org-a"))],
        ..Default::default()
    };
    let app = build_router(ServerState::new(manager, auth));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn the_quota_api_admits_only_the_admin_and_validates_limits() {
    let resources = ResourceManager::new();
    let base = serve(resources.clone(), true).await;
    let url = format!("{base}/orgs/org-a/quotas");
    let http = reqwest::Client::new();
    let put = |auth: Option<&'static str>, body: &'static str| {
        let mut req = http.put(&url).body(body);
        if let Some(a) = auth {
            req = req.header("Authorization", a);
        }
        req.send()
    };

    assert_eq!(
        http.get(&url).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        put(None, "{}").await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        put(Some("Bearer wrong"), "{}").await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    // An organization's own token cannot raise its own quotas.
    let own = put(Some("Bearer org-a-token"), r#"{"max_messages_per_sec": 9}"#);
    assert_eq!(own.await.unwrap().status(), StatusCode::FORBIDDEN);

    for (body, field) in [
        (r#"{"max_messages_per_sec": 0}"#, "max_messages_per_sec"),
        (
            r#"{"max_http_requests_per_sec": -1}"#,
            "max_http_requests_per_sec",
        ),
        (r#"{"max_mesages_per_sec": 5}"#, "max_mesages_per_sec"),
    ] {
        let res = put(Some(ADMIN), body).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(res.text().await.unwrap().contains(field), "{body}");
    }
    assert_eq!(
        resources
            .get_or_create_org(&OrgId::new("org-a"))
            .get_quotas()
            .max_messages_per_sec,
        OrgQuotas::default().max_messages_per_sec,
        "a refused PUT changes nothing"
    );

    let res = put(Some(ADMIN), r#"{"max_messages_per_sec": 1234}"#)
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let got: serde_json::Value = http
        .get(&url)
        .header("Authorization", ADMIN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got["max_messages_per_sec"], 1234);
    let other = resources
        .get_or_create_org(&OrgId::new("org-b"))
        .get_quotas();
    assert_eq!(
        other.max_messages_per_sec,
        OrgQuotas::default().max_messages_per_sec
    );
}

#[tokio::test]
async fn without_an_admin_token_the_quota_api_is_closed() {
    let base = serve(ResourceManager::new(), false).await;
    let res = reqwest::Client::new()
        .get(format!("{base}/orgs/org-a/quotas"))
        .header("Authorization", "Bearer org-a-token")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

fn endless(org: &str) -> StreamConfig {
    StreamConfig::from_yaml_or_json(&format!(
        r#"
org: {org}
input:
  generate:
    interval: ""
    batch_size: 100
    mapping: 'root = {{"v": 1}}'
output:
  drop: {{}}
"#
    ))
    .unwrap()
}

/// Messages a stream takes in over `window`.
async fn received(handle: &ekiben_server::RunningStreamHandle, window: Duration) -> u64 {
    let before = handle.metrics.input_received.load(Ordering::Relaxed);
    tokio::time::sleep(window).await;
    handle.metrics.input_received.load(Ordering::Relaxed) - before
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_quota_lowered_through_the_api_slows_a_running_stream_and_raising_it_restores_it() {
    let resources = ResourceManager::new();
    let base = serve(resources.clone(), true).await;
    let reg = full_registry();
    let start = |org: &str| {
        spawn_stream(
            org.to_string(),
            endless(org),
            reg.clone(),
            resources.clone(),
            None,
            None,
            false,
        )
        .unwrap()
    };
    let live = start("org-live");
    let other = start("org-other");
    let put = |per_sec: u64| {
        reqwest::Client::new()
            .put(format!("{base}/orgs/org-live/quotas"))
            .header("Authorization", ADMIN)
            .body(format!(r#"{{"max_messages_per_sec": {per_sec}}}"#))
            .send()
    };

    tokio::time::sleep(Duration::from_millis(300)).await;
    let fast = received(&live, Duration::from_millis(500)).await;
    assert!(fast > 3_000, "before: {fast}");

    assert_eq!(put(1_000).await.unwrap().status(), StatusCode::OK);
    // The 1 s burst lets 1000 pass at once; from then on 1000/s.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let slow = received(&live, Duration::from_secs(1)).await;
    assert!(slow <= 1_500, "after lowering: {slow}");
    let neighbour = received(&other, Duration::from_millis(500)).await;
    assert!(
        neighbour > 3_000,
        "another organization slowed too: {neighbour}"
    );

    assert_eq!(put(100_000_000).await.unwrap().status(), StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let restored = received(&live, Duration::from_millis(500)).await;
    assert!(restored > 3_000, "after raising: {restored}");

    for h in [live, other] {
        let summary = h.stop(Duration::from_secs(5)).await.unwrap().unwrap();
        assert!(
            summary.success,
            "a paced stream never fails: {:?}",
            summary.error
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resident_stream_outlives_its_processing_time_budget_in_wall_time() {
    // The budget counts time inside the processors, not since the start: an idle resident
    // stream is never stopped by it.
    let resources = ResourceManager::new();
    resources.set_org_quotas(
        &OrgId::new("idle"),
        OrgQuotas {
            max_cpu_time_ms_per_run: 200,
            ..Default::default()
        },
    );
    let conf = StreamConfig::from_yaml_or_json(
        r#"
org: idle
input:
  generate:
    interval: 100ms
    mapping: 'root = {"v": 1}'
output:
  drop: {}
"#,
    )
    .unwrap();
    let handle = spawn_stream(
        "idle".to_string(),
        conf,
        full_registry(),
        resources,
        None,
        None,
        false,
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(handle.is_running());
    let summary = handle.stop(Duration::from_secs(5)).await.unwrap().unwrap();
    assert!(summary.success, "{:?}", summary.error);
}
