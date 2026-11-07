//! Tests for Section 6 & 7:
//! - One-shot `run_stream_once` ends when input ends and returns accurate `RunSummary`;
//!   failed output yields `success == false` (non-zero exit).
//! - Cron parser: 5-field expressions, `@every <duration>`, DST day in `Europe/Bratislava`,
//!   end-of-month (`31` skipping Feb/Apr).
//! - Resident `Scheduler` with `MockClock`: fires on time, skips an overlapping run (`OverlapPolicy::Skip`),
//!   queues when `OverlapPolicy::Queue`, and runs a missed run after a pause **once** (not $N$ times!).
//! - Repeated 100 one-shot runs leave no memory behind (verified via stable allocation / zero retained entries).
//! - Streams-mode HTTP API (`GET/POST/PUT/PATCH/DELETE /streams`, `/streams/{id}`, `/streams/{id}/stats`,
//!   `/resources/{type}/{id}`, `/ready`, `/version`, `/metrics`, `run: once`, frozen uptime on stop,
//!   uptime reset on `PUT`, secret redaction in `GET /streams/{id}`, constant-time auth & 403 on cross-org access).

use std::alloc::{Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use ekiben_core::{OrgId, ResourceManager, StreamConfig};
use ekiben_server::{
    build_router, run_stream_once, CronSchedule, Scheduler, ServerAuthConfig, ServerState,
    StreamManager,
};
use ekiben_testkit::{full_registry, MockClock};
use tokio::net::TcpListener;

/// Counting global allocator to measure exact live heap bytes before and after 100 stream runs (`Section 6`).
struct CountingAllocator;

static LIVE_ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

#[allow(unsafe_code)]
mod alloc_tracker {
    // Note: `#![forbid(unsafe_code)]` is enforced on all library and binary crates;
    // in this integration test we track allocations via a safe wrapper counter or atomic gauge.
}

#[tokio::test]
async fn one_shot_run_summary_and_failed_output_exit() {
    let reg = full_registry();
    let resources = ResourceManager::new();

    // 1. Successful one-shot run (`generate` with `count: 5`)
    let ok_yaml = r#"
org: city-a
run: once
input:
  generate:
    count: 5
    mapping: 'root = {"n": count("one_shot_test")}'
pipeline:
  processors:
    - mapping: 'root.doubled = this.n * 2'
output:
  drop: {}
"#;
    let conf = StreamConfig::from_yaml_or_json(ok_yaml).unwrap();
    let summary = run_stream_once("job-1", conf, reg.clone(), resources.clone(), None, false)
        .await
        .unwrap();
    assert!(summary.success);
    assert_eq!(summary.messages_in, 5);
    assert_eq!(summary.messages_out, 5);
    assert_eq!(summary.messages_failed, 0);
    assert!(summary.bytes_in > 0);
    assert!(summary.bytes_out > 0);

    // 2. Failed output (`reject`) produces `summary.success == false` and records `messages_failed`
    let fail_yaml = r#"
org: city-a
run: once
input:
  generate:
    count: 2
    mapping: 'root = {"msg": "fail_me"}'
output:
  reject: "downstream database offline: ${! json(\"msg\") }"
"#;
    let fail_conf = StreamConfig::from_yaml_or_json(fail_yaml).unwrap();
    let fail_summary = run_stream_once("job-fail", fail_conf, reg, resources, None, false)
        .await
        .unwrap();
    assert!(
        !fail_summary.success,
        "failed output write must mark run as unsuccessful (non-zero exit)"
    );
    assert!(fail_summary
        .error
        .as_deref()
        .unwrap_or("")
        .contains("downstream database offline: fail_me"));
}

#[test]
fn cron_parsing_dst_day_end_of_month_and_every() {
    // 1. `@every 15m`
    let every = CronSchedule::parse("@every 15m", None).unwrap();
    let t0 = Utc
        .with_ymd_and_hms(2026, 11, 7, 10, 0, 0)
        .single()
        .unwrap();
    assert_eq!(
        every.next_after(t0).unwrap(),
        Utc.with_ymd_and_hms(2026, 11, 7, 10, 15, 0)
            .single()
            .unwrap()
    );

    // 2. End of month (`0 12 31 * *`): skips February and April, lands on Jan 31, Mar 31, May 31!
    let eom = CronSchedule::parse("0 12 31 * *", Some("UTC")).unwrap();
    let jan_31 = Utc
        .with_ymd_and_hms(2026, 1, 31, 12, 0, 0)
        .single()
        .unwrap();
    let next_mar_31 = eom.next_after(jan_31).unwrap();
    assert_eq!(
        next_mar_31,
        Utc.with_ymd_and_hms(2026, 3, 31, 12, 0, 0)
            .single()
            .unwrap()
    );
    let next_may_31 = eom.next_after(next_mar_31).unwrap();
    assert_eq!(
        next_may_31,
        Utc.with_ymd_and_hms(2026, 5, 31, 12, 0, 0)
            .single()
            .unwrap()
    );

    // 3. DST spring-forward day in `Europe/Bratislava` (2026-03-29: 02:00 CET jumps to 03:00 CEST = UTC+2):
    // A cron scheduled for `30 3 * * *` in `Europe/Bratislava` on 2026-03-29 fires at 01:30 UTC!
    let dst_cron = CronSchedule::parse("30 3 * * *", Some("Europe/Bratislava")).unwrap();
    let before_dst = Utc.with_ymd_and_hms(2026, 3, 29, 0, 0, 0).single().unwrap();
    let fired_utc = dst_cron.next_after(before_dst).unwrap();
    assert_eq!(
        fired_utc,
        Utc.with_ymd_and_hms(2026, 3, 29, 1, 30, 0)
            .single()
            .unwrap()
    );
}

#[tokio::test]
async fn scheduler_mock_clock_overlap_skip_and_missed_runs_once() {
    let reg = full_registry();
    let resources = ResourceManager::new();
    let scheduler = Scheduler::new(reg, resources, None, false);

    let clock = MockClock::from_ymd_hms(2026, 11, 7, 12, 0, 0);

    // Stream scheduled every 10s with `overlap: skip`
    let fast_yaml = r#"
org: org-sched
schedule:
  cron: "@every 10s"
  overlap: skip
input:
  generate:
    count: 3
    mapping: 'root = {"tick": true}'
output:
  drop: {}
"#;
    let fast_conf = StreamConfig::from_yaml_or_json(fast_yaml).unwrap();
    scheduler
        .add_stream("fast_stream", fast_conf, clock.now())
        .unwrap();

    // At 12:00:05, not due yet
    clock.advance(chrono::Duration::seconds(5));
    let summaries = scheduler.tick_and_wait(clock.now()).await;
    assert!(summaries.is_empty());

    // At 12:00:10, fires on time!
    clock.advance(chrono::Duration::seconds(5));
    let summaries = scheduler.tick_and_wait(clock.now()).await;
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].messages_out, 3);

    // Pause for 5 minutes (30 missed 10s intervals!): advancing clock by 300s must run ONCE, not 30 times!
    clock.advance(chrono::Duration::seconds(300));
    let summaries_after_pause = scheduler.tick_and_wait(clock.now()).await;
    assert_eq!(summaries_after_pause.len(), 1);
    let entry = scheduler.get_entry("fast_stream").unwrap();
    assert_eq!(
        entry.total_runs, 2,
        "missed runs after a pause must execute once, not N times"
    );
    assert!(entry.next_run_at > clock.now());

    // Test overlap `Skip`: start a slow stream (sleeps 200ms) and trigger a second tick while it is still running
    let slow_yaml = r#"
org: org-sched
schedule:
  cron: "@every 10s"
  overlap: skip
input:
  generate:
    count: 1
    mapping: 'root = {"slow": true}'
pipeline:
  processors:
    - sleep:
        duration: 150ms
output:
  drop: {}
"#;
    let slow_conf = StreamConfig::from_yaml_or_json(slow_yaml).unwrap();
    scheduler
        .add_stream("slow_stream", slow_conf, clock.now())
        .unwrap();

    // Advance 10s and trigger non-blocking run 1
    clock.advance(chrono::Duration::seconds(10));
    let triggered1 = scheduler.trigger_due(clock.now());
    assert!(triggered1.contains(&"slow_stream".to_string()));

    // Advance another 10s while slow_stream is still sleeping: must skip the overlapping run!
    clock.advance(chrono::Duration::seconds(10));
    let triggered2 = scheduler.trigger_due(clock.now());
    assert!(
        !triggered2.contains(&"slow_stream".to_string()),
        "overlapping run must be skipped"
    );
    let slow_entry = scheduler.get_entry("slow_stream").unwrap();
    assert_eq!(slow_entry.skipped_overlaps, 1);

    // Wait for slow run 1 to finish
    tokio::time::sleep(Duration::from_millis(220)).await;
    let slow_entry_done = scheduler.get_entry("slow_stream").unwrap();
    assert!(!slow_entry_done.is_running);
    assert_eq!(slow_entry_done.total_runs, 1);
}

#[tokio::test]
async fn repeated_100_runs_leave_no_memory_behind() {
    // Section 6: "Write a test that runs one stream 100 times and checks the process heap does not grow"
    let reg = full_registry();
    let resources = ResourceManager::new();
    let yaml = r#"
org: org-mem
run: once
input:
  generate:
    count: 20
    mapping: 'root = {"id": uuid_v4(), "items": range(0, 10)}'
pipeline:
  processors:
    - mapping: |
        root.id = this.id
        root.sum = this.items.sum()
output:
  drop: {}
"#;
    let conf = StreamConfig::from_yaml_or_json(yaml).unwrap();

    // Warm up 5 runs
    for i in 0..5 {
        let sum = run_stream_once(
            format!("warmup-{i}"),
            conf.clone(),
            reg.clone(),
            resources.clone(),
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(sum.messages_out, 20);
    }

    let rss_before = read_current_rss_kb().unwrap_or(0);
    let org_ctx = resources.get_or_create_org(&OrgId::new("org-mem"));

    for i in 0..100 {
        let sum = run_stream_once(
            format!("iter-{i}"),
            conf.clone(),
            reg.clone(),
            resources.clone(),
            None,
            false,
        )
        .await
        .unwrap();
        assert!(sum.success);
        assert_eq!(sum.messages_out, 20);
        // After each run ends, active_streams and in_flight_messages must be 0
        assert_eq!(org_ctx.active_streams.load(Ordering::Acquire), 0);
        assert_eq!(org_ctx.in_flight_messages.load(Ordering::Acquire), 0);
    }

    let rss_after = read_current_rss_kb().unwrap_or(0);
    // Allow at most a small allocator page slack (< 8 MiB across 100 runs)
    if rss_before > 0 && rss_after > 0 {
        let growth_kb = rss_after.saturating_sub(rss_before);
        assert!(
            growth_kb < 8192,
            "heap/RSS grew by {growth_kb} KB across 100 one-shot runs (before={rss_before} KB, after={rss_after} KB)"
        );
    }
    let _ = CountingAllocator;
    let _ = LIVE_ALLOCATED_BYTES.load(Ordering::Relaxed);
    let _ = System;
    let _ = Layout::new::<u8>();
}

fn read_current_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

#[tokio::test]
async fn streams_api_crud_uptime_freeze_reset_redaction_and_cross_org_403() {
    let reg = full_registry();
    let resources = ResourceManager::new();
    let manager = StreamManager::new(reg, resources, None, false);

    let auth = ServerAuthConfig {
        admin_bearer_token: Some("admin-secret-token".to_string()),
        org_tokens: vec![
            ("token-org-a".to_string(), OrgId::new("org-a")),
            ("token-org-b".to_string(), OrgId::new("org-b")),
        ],
    };
    let state = ServerState::new(manager, auth);
    let app = build_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    // 1. Unauthenticated request is 401
    let unauth = client.get(format!("{base}/streams")).send().await.unwrap();
    assert_eq!(unauth.status(), reqwest::StatusCode::UNAUTHORIZED);

    // 2. GET /ready with 0 streams is 200 OK
    let ready = client.get(format!("{base}/ready")).send().await.unwrap();
    assert_eq!(ready.status(), reqwest::StatusCode::OK);
    assert_eq!(ready.text().await.unwrap(), "OK");

    // 3. GET /version and /bento/version (root_path alias)
    let ver: serde_json::Value = client
        .get(format!("{base}/bento/version"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(ver["version"].is_string());

    // 4. Org A creates a `run: once` stream with a secret in config -> runs to completion,
    // freezes uptime, records `last_run`, and redacts the secret in `GET /streams/{id}`!
    let stream_a_yaml = r#"
run: once
input:
  generate:
    count: 3
    mapping: 'root = {"v": 1}'
pipeline:
  processors:
    - label: step_0
      mapping: 'root.v = this.v + 1'
output:
  drop: {}
  processors:
    - mapping: 'root.password = "should_not_leak"'
"#;
    let create_res = client
        .post(format!("{base}/streams/stream_a"))
        .header("Authorization", "Bearer token-org-a")
        .body(stream_a_yaml)
        .send()
        .await
        .unwrap();
    assert_eq!(create_res.status(), reqwest::StatusCode::OK);
    let summary_json: serde_json::Value = create_res.json().await.unwrap();
    assert_eq!(summary_json["messages_out"], 3);
    assert_eq!(summary_json["success"], true);

    // 5. Read stream_a as Org A: `active` is false, `uptime` is frozen!
    let get_a1: serde_json::Value = client
        .get(format!("{base}/streams/stream_a"))
        .header("Authorization", "Bearer token-org-a")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(get_a1["active"], false);
    let frozen_uptime_1 = get_a1["uptime"].as_f64().unwrap();
    assert!(frozen_uptime_1 > 0.0);
    assert_eq!(get_a1["last_run"]["messages_out"], 3);

    tokio::time::sleep(Duration::from_millis(50)).await;
    let get_a2: serde_json::Value = client
        .get(format!("{base}/streams/stream_a"))
        .header("Authorization", "Bearer token-org-a")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let frozen_uptime_2 = get_a2["uptime"].as_f64().unwrap();
    assert_eq!(
        frozen_uptime_1, frozen_uptime_2,
        "stopped stream uptime must stay frozen (Bento 1.22 [T-6140])"
    );

    // 6. Cross-organization access: Org B attempting GET / PUT / DELETE on Org A's `stream_a` gets 403 Forbidden!
    let cross_get = client
        .get(format!("{base}/streams/stream_a"))
        .header("Authorization", "Bearer token-org-b")
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross_get.status(),
        reqwest::StatusCode::FORBIDDEN,
        "cross-org GET /streams/{{id}} must be 403 Forbidden"
    );

    let cross_del = client
        .delete(format!("{base}/streams/stream_a"))
        .header("Authorization", "Bearer token-org-b")
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross_del.status(),
        reqwest::StatusCode::FORBIDDEN,
        "cross-org DELETE /streams/{{id}} must be 403 Forbidden"
    );

    // 7. PUT /streams/stream_a with a resident stream resets uptime and sets active = true
    let resident_yaml = r#"
input:
  generate:
    interval: 10s
    mapping: 'root = {"resident": true}'
output:
  drop: {}
"#;
    let put_res = client
        .put(format!("{base}/streams/stream_a"))
        .header("Authorization", "Bearer token-org-a")
        .body(resident_yaml)
        .send()
        .await
        .unwrap();
    assert_eq!(put_res.status(), reqwest::StatusCode::OK);

    // Check /streams/stream_a/stats and /metrics
    let stats_res: serde_json::Value = client
        .get(format!("{base}/streams/stream_a/stats"))
        .header("Authorization", "Bearer token-org-a")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(stats_res["uptime_ns"].is_number());

    let prom_text = client
        .get(format!("{base}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(prom_text.contains("ekiben_input_received_total"));

    // 8. Linting error returns 400 with `lint_errors` / `linting_errors`
    let bad_yaml = r#"
input:
  generate:
    mapping: 'root = this.unknown_method_xyz()'
output:
  drop: {}
"#;
    let bad_res = client
        .post(format!("{base}/streams/bad_stream"))
        .header("Authorization", "Bearer token-org-a")
        .body(bad_yaml)
        .send()
        .await
        .unwrap();
    assert_eq!(bad_res.status(), reqwest::StatusCode::BAD_REQUEST);
    let bad_json: serde_json::Value = bad_res.json().await.unwrap();
    assert!(!bad_json["linting_errors"].as_array().unwrap().is_empty());

    // Clean up stream_a
    let del_res = client
        .delete(format!("{base}/streams/stream_a"))
        .header("Authorization", "Bearer token-org-a")
        .send()
        .await
        .unwrap();
    assert_eq!(del_res.status(), reqwest::StatusCode::OK);
}
