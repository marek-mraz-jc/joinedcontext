//! The v0 API cases of Bento 1.22's `processor_test.go`, and the sandbox's refusals.

use std::sync::Arc;
use std::time::Instant;

use ekiben_core::{OrgContext, OrgId, OrgQuotas, ResourceManager, StreamMetrics};
use serde_json::json;

use super::*;

fn proc_ctx() -> ProcContext {
    ProcContext {
        stream_id: "s".into(),
        org: OrgContext::new(OrgId::default(), OrgQuotas::default()),
        resources: ResourceManager::new(),
        registry: Arc::new(Registry::new()),
        blobl_env: ekiben_bloblang::Environment::new(),
        metrics: StreamMetrics::new("s", "default"),
        label: String::new(),
        component_type: "javascript".into(),
        path: "pipeline.processors.0".into(),
        file_root: None,
        shutdown_rx: tokio::sync::watch::channel(false).1,
    }
}

fn processor(config: JsonValue) -> JavascriptProcessor {
    JavascriptProcessor::from_config(&ComponentConfig {
        label: String::new(),
        component_type: "javascript".into(),
        config,
        processors: Vec::new(),
    })
    .expect("builds")
}

async fn run(code: &str, parts: Vec<Part>) -> Batch {
    let p = processor(json!({ "code": code }));
    let mut out = p.process_batch(&proc_ctx(), parts).await.expect("runs");
    assert_eq!(out.len(), 1);
    out.remove(0)
}

fn text(p: &Part) -> String {
    String::from_utf8_lossy(&p.as_bytes_ref()).into_owned()
}

fn flag(p: &Part) -> String {
    p.error_get().map(|e| e.to_string()).unwrap_or_default()
}

#[tokio::test]
async fn structured_and_string_round_trips() {
    let out = run(
        r#"const t = bento.v0_msg_as_structured(); t.b = t.a * 2; bento.v0_msg_set_structured(t);"#,
        vec![Part::from_str(r#"{"a":2}"#)],
    )
    .await;
    assert_eq!(flag(&out[0]), "");
    assert_eq!(
        out[0].as_structured_ref().unwrap().to_json_value(),
        json!({"a": 2, "b": 4})
    );
    let out = run(
        "bento.v0_msg_set_string(bento.v0_msg_as_string().toUpperCase());",
        vec![Part::from_str("hello world")],
    )
    .await;
    assert_eq!(text(&out[0]), "HELLO WORLD");
}

#[tokio::test]
async fn metadata_reads_and_writes() {
    let mut part = Part::from_str("{}");
    part.meta_set_str("foo", "bar");
    let out = run(
        r#"
        if (!bento.v0_msg_exists_meta("foo") || bento.v0_msg_exists_meta("nope")) throw new Error("exists");
        bento.v0_msg_set_meta("copy", bento.v0_msg_get_meta("foo") + "!");
        bento.v0_msg_set_meta("n", 5);
        console.log("seen", { foo: bento.v0_msg_get_meta("foo") });
        "#,
        vec![part],
    )
    .await;
    assert_eq!(flag(&out[0]), "");
    assert_eq!(out[0].meta_get_str("copy"), "bar!");
    assert_eq!(out[0].meta_get_str("n"), "5");

    let out = run(
        r#"bento.v0_msg_get_meta("missing");"#,
        vec![Part::from_str("{}")],
    )
    .await;
    assert!(flag(&out[0]).contains("key not found"), "{}", flag(&out[0]));
}

#[tokio::test]
async fn a_message_that_is_not_json_is_flagged_and_the_rest_go_on() {
    let out = run(
        "bento.v0_msg_set_structured(bento.v0_msg_as_structured());",
        vec![Part::from_str("not json"), Part::from_str(r#"{"ok":true}"#)],
    )
    .await;
    assert!(flag(&out[0]).contains("failed to parse message as JSON"));
    assert_eq!(flag(&out[1]), "");
}

#[tokio::test]
async fn no_state_passes_between_messages() {
    let out = run(
        "globalThis.n = (globalThis.n || 0) + 1; bento.v0_msg_set_string(String(globalThis.n));",
        vec![
            Part::from_str("a"),
            Part::from_str("b"),
            Part::from_str("c"),
        ],
    )
    .await;
    assert!(out.iter().all(|p| text(p) == "1"));
}

#[tokio::test]
async fn hostile_scripts_are_flagged_within_their_limits() {
    let cases = [
        ("while (true) {}", "ran past its limit of 50 ms"),
        ("function f() { return f(); } f();", "stack"),
        ("let s = 'x'; while (true) { s = s + s; }", "MiB|too long"),
        (
            "const a = []; while (true) { a.push(new Array(100000).fill(1)); }",
            "MiB",
        ),
        ("require('fs');", "require"),
        ("fetch('http://example.com');", "fetch"),
        ("Function('return this')().process;", "Function"),
        (
            "(function () {}).constructor('return 1')();",
            "not a function",
        ),
        ("eval('1');", "eval"),
        (
            "bento.v0_fetch('http://example.com', {}, 'GET', '');",
            "not a function",
        ),
        ("setTimeout(() => {}, 1);", "setTimeout"),
        ("throw 'plain';", "plain"),
    ];
    for (code, expected) in cases {
        let started = Instant::now();
        let out = run(code, vec![Part::from_str("{}"), Part::from_str("next")]).await;
        let elapsed = started.elapsed();
        assert!(
            expected.split('|').any(|e| flag(&out[0]).contains(e)),
            "{code}: {}",
            flag(&out[0])
        );
        // Each message carries its own deadline; two of them stay far under a second.
        assert!(elapsed.as_millis() < 1500, "{code}: {elapsed:?}");
        assert_eq!(
            text(&out[1]),
            "next",
            "{code}: the next message is untouched"
        );
    }
}

#[tokio::test]
async fn async_code_and_unrun_promises_end_cleanly() {
    let out = run(
        "async function f() { await 1; bento.v0_msg_set_string('late'); } f(); Promise.resolve().then(() => {}); bento.v0_msg_set_string('now');",
        vec![Part::from_str("x")],
    )
    .await;
    assert_eq!(flag(&out[0]), "");
    assert_eq!(text(&out[0]), "now", "a promise job never runs");
}

#[tokio::test]
async fn a_good_message_after_a_timeout_still_runs() {
    let out = run(
        "if (bento.v0_msg_as_string() === 'spin') { while (true) {} } bento.v0_msg_set_string('done');",
        vec![Part::from_str("spin"), Part::from_str("x")],
    )
    .await;
    assert!(flag(&out[0]).contains("ms"));
    assert_eq!(text(&out[1]), "done");
    assert_eq!(flag(&out[1]), "");
}

fn budget_ctx(cpu_ms: u64) -> ProcContext {
    let mut c = proc_ctx();
    c.org = OrgContext::new(
        OrgId::default(),
        OrgQuotas {
            max_cpu_time_ms_per_run: cpu_ms,
            ..OrgQuotas::default()
        },
    );
    c
}

#[tokio::test]
async fn script_time_stops_at_the_runs_processing_budget() {
    // 120 ms for the run: two spins of 50 ms, the third cut at what is left, the other 17 unrun
    // (run, the 20 would take a second).
    let p = processor(json!({ "code": "while (true) {}", "timeout_ms": 50 }));
    let parts = (0..20).map(|_| Part::from_str("x")).collect();
    let started = Instant::now();
    let out = p
        .process_batch(&budget_ctx(120), parts)
        .await
        .expect("runs")
        .remove(0);
    let took = started.elapsed();
    assert!(took < Duration::from_millis(600), "{took:?}");
    assert!(flag(&out[0]).contains("ran past its limit of 50 ms"));
    assert!(flag(&out[1]).contains("ran past its limit of 50 ms"));
    for p in &out[2..] {
        assert!(flag(p).contains("max_cpu_time_ms_per_run"), "{}", flag(p));
    }
}

#[tokio::test]
async fn a_spent_run_budget_runs_no_script() {
    let ctx = budget_ctx(10);
    ctx.metrics.charge_processing(10_000_000);
    let p = processor(json!({ "code": "bento.v0_msg_set_string('ran')" }));
    let out = p
        .process_batch(&ctx, vec![Part::from_str("x")])
        .await
        .expect("runs")
        .remove(0);
    assert_eq!(text(&out[0]), "x");
    assert!(flag(&out[0]).contains("max_cpu_time_ms_per_run"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripts_take_turns_for_a_quarter_of_the_cores() {
    // Twice the slots plus one spins of 50 ms each: run all at once they end in about 50 ms,
    // in turns of `slots` they need three turns.
    let slots = script_slots().available_permits();
    let p = Arc::new(processor(
        json!({ "code": "while (true) {}", "timeout_ms": 50 }),
    ));
    let started = Instant::now();
    let runs: Vec<_> = (0..2 * slots + 1)
        .map(|_| {
            let p = p.clone();
            tokio::spawn(async move {
                p.process_batch(&proc_ctx(), vec![Part::from_str("x")])
                    .await
            })
        })
        .collect();
    for r in runs {
        r.await.expect("joins").expect("runs");
    }
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(140),
        "{slots} slots, {took:?}"
    );
}

#[test]
fn lint_names_what_ekiben_refuses() {
    let errs = lint(&json!({"file": "a.js", "global_folders": ["/x"], "timeout_ms": 500}));
    let all = errs.join("\n");
    assert!(all.contains("field file"), "{all}");
    assert!(all.contains("global_folders"), "{all}");
    assert!(all.contains("timeout_ms"), "{all}");
    assert!(all.contains("code"), "{all}");
    assert!(lint(&json!({"code": "1", "timeout_ms": 10, "memory_mb": 64})).is_empty());
    assert!(!lint(&json!({"code": "1", "memory_mb": 65})).is_empty());
}
