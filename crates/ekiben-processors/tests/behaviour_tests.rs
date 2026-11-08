//! Behaviour tests for every built-in processor and resource: configs Bento refuses, failures
//! driven for real (bad input, missing resources, shutdown mid-wait, archive bombs) and the
//! flow processors' loop, retry and close paths.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use ekiben_bloblang::{Environment, Value};
use ekiben_core::{
    Cache, ComponentConfig, CoreError, OrgId, OrgQuotas, Part, ProcContext, Processor, Registry,
    ResourceManager, StreamContext, StreamMetrics, SyncResponseStore,
};
use ekiben_processors::workflow::WorkflowProcessor;
use serde_json::json;
use tokio::sync::watch;

struct Env {
    reg: Arc<Registry>,
    sctx: StreamContext,
    ctx: ProcContext,
    stop: watch::Sender<bool>,
}

fn env() -> Env {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("o1"));
    let (stop, rx) = watch::channel(false);
    let sctx = StreamContext {
        stream_id: "s".into(),
        org,
        resources,
        registry: reg.clone(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("s", "o1"),
        label: String::new(),
        path: String::new(),
        file_root: None,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: rx,
    };
    let ctx = sctx.to_proc_context("pipeline", "", "pipeline.processors");
    Env {
        reg,
        sctx,
        ctx,
        stop,
    }
}

impl Env {
    fn proc(&self, v: serde_json::Value) -> Result<Arc<dyn Processor>, CoreError> {
        let c = ComponentConfig::from_json_value(&v, "noop", None)?;
        self.reg.build_processor(&c, &self.ctx, "0")
    }

    fn refused(&self, v: serde_json::Value) -> String {
        match self.proc(v.clone()) {
            Ok(_) => panic!("accepted {v}"),
            Err(e) => e.to_string(),
        }
    }

    async fn run(&self, v: serde_json::Value, input: Vec<Part>) -> Vec<Vec<Part>> {
        let p = self.proc(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
        let out = p.process_batch(&self.ctx, input).await.unwrap();
        p.close().await.unwrap();
        out
    }

    fn cache(&self, label: &str, v: serde_json::Value) -> Arc<dyn Cache> {
        let c = ComponentConfig::from_json_value(&v, "memory", None).unwrap();
        let cache = self.reg.build_cache(&c, &self.sctx).unwrap();
        self.sctx
            .resources
            .insert_cache(self.ctx.org_id(), label, cache.clone());
        cache
    }
}

fn parts(xs: &[&str]) -> Vec<Part> {
    xs.iter().map(|s| Part::from_str(s)).collect()
}

fn texts(b: &[Part]) -> Vec<String> {
    b.iter()
        .map(|p| String::from_utf8_lossy(&p.as_bytes_ref()).into_owned())
        .collect()
}

fn flat(out: &[Vec<Part>]) -> Vec<String> {
    out.iter().flat_map(|b| texts(b)).collect()
}

fn err_of(p: &Part) -> String {
    p.error_get().map(|e| e.message.clone()).unwrap_or_default()
}

// ---------- config refusals ----------

#[test]
fn configs_bento_refuses_are_refused_with_a_reason() {
    let e = env();
    for (cfg, want) in [
        (
            json!({"while": "x"}),
            "while processor config must be an object",
        ),
        (json!({"while": {"check": " "}}), "non-empty check"),
        (
            json!({"while": {"check": "root = ("}}),
            "failed to parse check query",
        ),
        (
            json!({"parallel": []}),
            "parallel processor config must be an object",
        ),
        (
            json!({"retry": 1}),
            "retry processor config must be an object",
        ),
        (json!({"retry": {}}), "at least one child processor"),
        (
            json!({"retry": {"backoff": {"initial_interval": "x"}, "processors": [{"noop": {}}]}}),
            "configuration error",
        ),
        (
            json!({"try": [{"nope": {}}]}),
            "not recognised as a processor",
        ),
        (json!({"try": [7]}), "expected object"),
        (json!({"branch": 1}), "branch config must be an object"),
        (
            json!({"branch": {"request_map": "root = (", "processors": [{"noop": {}}]}}),
            "failed to parse request mapping",
        ),
        (
            json!({"branch": {"result_map": "root = (", "processors": [{"noop": {}}]}}),
            "failed to parse result mapping",
        ),
        (
            json!({"branch": {}}),
            "requires at least one child processor",
        ),
        (json!({"branch": {"processors": []}}), "child processor"),
        (
            json!({"switch": {}}),
            "switch processor requires an array of cases",
        ),
        (json!({"switch": [1]}), "switch case '0' must be an object"),
        (
            json!({"switch": [{"check": "root = (", "processors": [{"noop": {}}]}]}),
            "case '0' parse error",
        ),
        (
            json!({"switch": [{"check": "true"}]}),
            "case '0' has no processors",
        ),
        (
            json!({"switch": [{"processors": []}]}),
            "case '0' has no processors",
        ),
        (json!({"switch": [{"processors": [5]}]}), "expected object"),
        (
            json!({"mapping": {"a": 1}}),
            "processor 'mapping' requires a string mapping",
        ),
        (
            json!({"bloblang": 1}),
            "processor 'bloblang' requires a string mapping",
        ),
        (
            json!({"mutation": 1}),
            "processor 'mutation' requires a string mapping",
        ),
        (json!({"workflow": 1}), "workflow config must be an object"),
        (
            json!({"workflow": {"branches": {"a b": {"processors": [{"noop": {}}]}}}}),
            "contains invalid characters",
        ),
        (
            json!({"workflow": {"order": [1]}}),
            "workflow order tier 0 must be an array",
        ),
        (
            json!({"workflow": {"order": [[]]}}),
            "explicit order tier '0' was empty",
        ),
        (
            json!({"workflow": {"order": [[1]]}}),
            "workflow order branch name must be a string",
        ),
        (
            json!({"workflow": {"branches": {"a": {"processors": [{"noop": {}}]}}, "order": [["a", "a"]]}}),
            "listed multiple times: a",
        ),
        (
            json!({"workflow": {"branches": {"a": {"processors": [{"noop": {}}]}}, "order": [["b"]]}}),
            "branch 'b' listed in order was not found",
        ),
        (
            json!({"workflow": {"branches": {"a": {"processors": [{"noop": {}}]}, "b": {"processors": [{"noop": {}}]}}, "order": [["a"]]}}),
            "missing from order: [\"b\"]",
        ),
        (
            json!({"sleep": {}}),
            "sleep processor requires a duration field",
        ),
        (
            json!({"resource": ""}),
            "resource processor requires a resource name string",
        ),
        (
            json!({"resource": "nowhere"}),
            "processor resource 'nowhere' was not found",
        ),
        (
            json!({"rate_limit": {}}),
            "rate_limit processor requires a resource field",
        ),
        (
            json!({"cache": 1}),
            "cache processor config must be an object",
        ),
        (
            json!({"cache": {}}),
            "cache processor requires a resource field",
        ),
        (
            json!({"cache": {"resource": "missing"}}),
            "cache resource 'missing' was not found",
        ),
        (json!({"dedupe": 1}), "dedupe config must be an object"),
        (json!({"dedupe": {}}), "dedupe requires a cache field"),
        (
            json!({"group_by_value": {}}),
            "group_by_value requires a value field",
        ),
        (json!({"archive": 1}), "archive config must be an object"),
        (json!({"archive": {}}), "archive requires a format field"),
        (
            json!({"archive": {"format": "rar"}}),
            "archive format not recognised: rar",
        ),
        (
            json!({"unarchive": {}}),
            "unarchive requires a format field",
        ),
        (
            json!({"unarchive": {"format": "rar"}}),
            "archive format not recognised: rar",
        ),
        (
            json!({"unarchive": {"format": "csv:"}}),
            "non-empty delimiter",
        ),
        (
            json!({"unarchive": {"format": "csv:;;"}}),
            "single character delimiter",
        ),
        (
            json!({"unarchive": {"format": "csv:é"}}),
            "single character delimiter",
        ),
        (
            json!({"xml": {"operator": "from_json"}}),
            "xml operator not recognised: from_json",
        ),
        (
            json!({"compress": {"algorithm": "brotli"}}),
            "compression algorithm not supported: brotli",
        ),
        (
            json!({"decompress": {"algorithm": "brotli"}}),
            "compression algorithm not supported: brotli",
        ),
    ] {
        let got = e.refused(cfg.clone());
        assert!(got.contains(want), "{cfg}: {got}");
    }

    e.cache("c", json!({"memory": {}}));
    for (cfg, want) in [
        (
            json!({"cache": {"resource": "c"}}),
            "cache processor requires an operator field",
        ),
        (
            json!({"cache": {"resource": "c", "operator": "get"}}),
            "cache processor requires a key field",
        ),
        (
            json!({"dedupe": {"cache": "c"}}),
            "dedupe requires a key field",
        ),
        (
            json!({"dedupe": {"cache": "c", "key": "k", "strategy": "RANDOM"}}),
            "dedupe strategy 'RANDOM' is not one of FIFO, LIFO",
        ),
    ] {
        let got = e.refused(cfg.clone());
        assert!(got.contains(want), "{cfg}: {got}");
    }
}

#[test]
fn resource_configs_bento_refuses() {
    let e = env();
    let build = |v: serde_json::Value| {
        let c = ComponentConfig::from_json_value(&v, "noop", None).unwrap();
        e.reg
            .build_rate_limit(&c, &e.sctx)
            .err()
            .map(|x| x.to_string())
    };
    assert!(build(json!({"local": {"count": -1}}))
        .unwrap()
        .contains("negative"));
    assert!(build(json!({"local": {"count": 0}}))
        .unwrap()
        .contains("larger than zero"));
    assert!(build(json!({"local": {"interval": "x"}})).is_some());
    assert!(build(json!({"local": {}})).is_none());

    let cache = |v: serde_json::Value| {
        let c = ComponentConfig::from_json_value(&v, "noop", None).unwrap();
        e.reg.build_cache(&c, &e.sctx).err().map(|x| x.to_string())
    };
    assert!(cache(json!({"memory": {"default_ttl": "soon"}})).is_some());
    assert!(cache(json!({"multilevel": {}}))
        .unwrap()
        .contains("requires an array of caches"));
    assert!(cache(json!({"multilevel": ["absent"]}))
        .unwrap()
        .contains("'absent' was not found"));
    assert!(cache(json!({"multilevel": [{"nope": {}}]}))
        .unwrap()
        .contains("not recognised as a cache"));
}

// ---------- flow ----------

#[tokio::test]
async fn try_catch_for_each_and_groups() {
    let e = env();
    let throw = json!({"mapping": "root = if this.bad == true { throw(\"bad\") } else { this }"});
    let out = e
        .run(
            json!({"try": [throw.clone(), {"mapping": "root = \"ran\""}]}),
            parts(&[r#"{"bad":true}"#, r#"{"bad":false}"#]),
        )
        .await;
    assert_eq!(flat(&out), vec![r#"{"bad":true}"#, "ran"]);
    assert_eq!(err_of(&out[0][0]), "failed assignment (line 1): bad");

    // catch runs only on errored parts and clears every flag on the way out.
    let mut errored = Part::from_str("x");
    errored.set_error_msg("earlier");
    let out = e
        .run(
            json!({"catch": [{"mapping": "root = error()"}]}),
            vec![errored, Part::from_str("ok")],
        )
        .await;
    assert_eq!(flat(&out), vec!["earlier", "ok"]);
    assert!(out[0].iter().all(|p| !p.is_errored()));

    // Children dropping everything yield no batch.
    let drop = json!({"mapping": "root = deleted()"});
    for kind in ["try", "catch", "for_each"] {
        let mut p = Part::from_str("x");
        if kind == "catch" {
            p.set_error_msg("e");
        }
        assert!(
            e.run(json!({ kind: [drop.clone()] }), vec![p])
                .await
                .is_empty(),
            "{kind}"
        );
    }
    assert!(e
        .run(
            json!({"parallel": {"processors": [drop.clone()]}}),
            parts(&["a"])
        )
        .await
        .is_empty());

    let out = e
        .run(
            json!({"for_each": [{"mapping": "root = content().uppercase()"}]}),
            parts(&["a", "b"]),
        )
        .await;
    assert_eq!(flat(&out), vec!["A", "B"]);
    let out = e
        .run(
            json!({"processors": [{"mapping": "root = content() + \"!\""}], "label": "g"}),
            parts(&["a"]),
        )
        .await;
    assert_eq!(flat(&out), vec!["a!"]);
    assert!(e
        .run(json!({"processors": [{"noop": {}}]}), vec![])
        .await
        .is_empty());
    // A non-array child list is no children.
    assert_eq!(
        flat(&e.run(json!({"try": "x"}), parts(&["a"])).await),
        vec!["a"]
    );
}

#[tokio::test]
async fn while_loops_until_the_check_fails_and_stops_at_its_limits() {
    let e = env();
    let inc = json!({"mapping": "root.n = this.n + 1"});
    let out = e
        .run(
            json!({"while": {"check": "this.n < 3", "processors": [inc.clone()]}}),
            parts(&[r#"{"n":0}"#]),
        )
        .await;
    assert_eq!(flat(&out), vec![r#"{"n":3}"#]);

    let out = e
        .run(json!({"while": {"check": "this.n < 0", "at_least_once": true, "processors": [inc.clone()]}}), parts(&[r#"{"n":0}"#]))
        .await;
    assert_eq!(flat(&out), vec![r#"{"n":1}"#]);

    let out = e
        .run(
            json!({"while": {"check": "true", "max_loops": 2, "processors": [inc.clone()]}}),
            parts(&[r#"{"n":0}"#]),
        )
        .await;
    assert_eq!(flat(&out), vec![r#"{"n":2}"#]);

    // Children that drop the message end the loop with nothing.
    let out = e
        .run(
            json!({"while": {"check": "true", "processors": [{"mapping": "root = deleted()"}]}}),
            parts(&["a"]),
        )
        .await;
    assert!(out.is_empty());

    // Without max_loops a check that never fails hits the hard safety limit: the batch is flagged.
    let out = e
        .run(json!({"while": {"check": "true"}}), parts(&["a"]))
        .await;
    assert!(
        err_of(&out[0][0]).contains("hard safety loop limit (10000)"),
        "{}",
        err_of(&out[0][0])
    );

    let p = e.proc(json!({"while": {"check": "true"}})).unwrap();
    e.stop.send(true).unwrap();
    assert!(matches!(
        p.process_batch(&e.ctx, parts(&["a"])).await,
        Err(CoreError::ComponentClosed)
    ));
}

#[tokio::test]
async fn parallel_keeps_order_under_a_cap() {
    let e = env();
    let out = e
        .run(json!({"parallel": {"cap": 2, "processors": [{"mapping": "root = content().uppercase()"}]}}), parts(&["a", "b", "c"]))
        .await;
    assert_eq!(flat(&out), vec!["A", "B", "C"]);
    assert!(e.run(json!({"parallel": {}}), vec![]).await.is_empty());
}

#[tokio::test]
async fn retry_reruns_failed_messages_and_tags_the_attempts() {
    let e = env();
    e.cache("c", json!({"memory": {}}));
    // The first attempt misses the key and then sets it; the second finds it.
    let children = json!([
        {"cache": {"resource": "c", "operator": "get", "key": "${! content() }"}},
        {"cache": {"resource": "c", "operator": "set", "key": "${! content() }", "value": "found"}}
    ]);
    for parallel in [false, true] {
        let out = e
            .run(
                json!({"retry": {"parallel": parallel, "backoff": {"initial_interval": "1ms", "max_interval": "2ms"}, "processors": children}}),
                parts(&[if parallel { "p" } else { "s" }]),
            )
            .await;
        let p = &out[0][0];
        assert!(!p.is_errored(), "{}", err_of(p));
        assert_eq!(texts(&out[0]), vec!["found"]);
        assert_eq!(p.meta_get("retry_count"), Some(&Value::Int(1)));
    }

    // An always failing child stops at max_retries, or when the backoff's elapsed budget is spent.
    let fail = json!([{"mapping": "root = throw(\"down\")"}]);
    for cfg in [
        json!({"max_retries": 2, "backoff": {"initial_interval": "1ms"}, "processors": fail}),
        json!({"backoff": {"initial_interval": "5ms", "max_elapsed_time": "8ms"}, "processors": fail}),
    ] {
        let out = e.run(json!({ "retry": cfg }), parts(&["x"])).await;
        assert!(err_of(&out[0][0]).ends_with("down"));
        assert!(matches!(out[0][0].meta_get("retry_count"), Some(Value::Int(n)) if *n >= 1));
    }
    assert!(e
        .run(
            json!({"retry": {"processors": [{"mapping": "root = deleted()"}]}}),
            parts(&["x"])
        )
        .await
        .is_empty());

    // Shutdown while waiting to retry ends the step.
    let p = e
        .proc(json!({"retry": {"backoff": {"initial_interval": "1ms"}, "processors": fail}}))
        .unwrap();
    let stop = e.stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        stop.send(true).unwrap();
    });
    assert!(matches!(
        p.process_batch(&e.ctx, parts(&["x"])).await,
        Err(CoreError::ComponentClosed)
    ));
}

// ---------- branch, switch, workflow ----------

#[tokio::test]
async fn branch_reports_a_count_mismatch_as_bento_does() {
    let e = env();
    let out = e
        .run(
            json!({"branch": {"processors": [{"insert_part": {"content": "extra"}}]}}),
            parts(&["a"]),
        )
        .await;
    let msg = err_of(&out[0][0]);
    assert!(
        msg.contains("started with 1 messages, finished with 2"),
        "{msg}"
    );
}

#[tokio::test]
async fn switch_flags_a_failing_check_and_keeps_order() {
    let e = env();
    let out = e
        .run(
            json!({"switch": [
                {"check": "this.n.number() > 1", "processors": [{"mapping": "root = \"big\""}]},
                {"processors": [{"mapping": "root = \"small\""}]}
            ]}),
            parts(&[r#"{"n":5}"#, r#"{"n":"x"}"#, r#"{"n":0}"#]),
        )
        .await;
    assert_eq!(flat(&out)[0], "big");
    assert_eq!(flat(&out)[2], "small");
    assert!(
        err_of(&out[0][1]).starts_with("failed to test case 0"),
        "{}",
        err_of(&out[0][1])
    );
    assert!(e
        .run(
            json!({"switch": [{"processors": [{"mapping": "root = deleted()"}]}]}),
            parts(&["a"])
        )
        .await
        .is_empty());
}

#[tokio::test]
async fn workflow_skips_applied_branches_and_flags_failures() {
    let e = env();
    let cfg = json!({
        "branches": {
            "a": {"request_map": "root = this", "processors": [{"mapping": "root = \"A\""}], "result_map": "root.a = content().string()"},
            "b": {"request_map": "root = this.n.number()", "processors": [{"noop": {}}], "result_map": "root.b = this"}
        }
    });
    let c = ComponentConfig::from_json_value(&json!({"workflow": cfg}), "noop", None).unwrap();
    let w = WorkflowProcessor::from_config(&c, &e.ctx).unwrap();
    let mut flow: Vec<String> = w.flow().iter().flatten().cloned().collect();
    flow.sort();
    assert_eq!(flow, vec!["a", "b"]);
    assert_eq!(w.processor_type(), "workflow");

    let p = e.proc(json!({"workflow": cfg})).unwrap();
    // `b` fails its request map; `apply` limits the second message to `b`, `succeeded` skips `a`.
    let out = p
        .process_batch(
            &e.ctx,
            parts(&[
                r#"{"n":"x"}"#,
                r#"{"n":1,"meta":{"workflow":{"apply":["b"]}}}"#,
                r#"{"n":2,"meta":{"workflow":{"succeeded":["a"]}}}"#,
                "not json",
            ]),
        )
        .await
        .unwrap();
    let docs: Vec<serde_json::Value> = out[0][..3]
        .iter()
        .map(|p| p.as_structured_ref().unwrap().to_json_value())
        .collect();
    assert_eq!(docs[0]["a"], "A");
    assert!(
        docs[0]["meta"]["workflow"]["failed"]["b"].is_string(),
        "{}",
        docs[0]
    );
    assert!(
        docs[1].get("a").is_none() && docs[1]["b"] == 1,
        "{}",
        docs[1]
    );
    assert_eq!(docs[1]["meta"]["workflow"]["skipped"], json!(["a"]));
    assert!(docs[2].get("a").is_none(), "{}", docs[2]);
    assert!(docs[2]["meta"]["workflow"]["previous"]["succeeded"].is_array());
    assert!(out[0][3].is_errored());
    p.close().await.unwrap();

    // Without a meta path a failed branch flags the message.
    let mut no_meta = cfg.clone();
    no_meta["meta_path"] = json!("");
    let out = e
        .run(json!({ "workflow": no_meta }), parts(&[r#"{"n":"x"}"#]))
        .await;
    assert!(
        err_of(&out[0][0]).contains("workflow branches failed: [\"b\"]"),
        "{}",
        err_of(&out[0][0])
    );
}

// ---------- utility ----------

#[tokio::test]
async fn sleep_waits_skips_bad_durations_and_stops_at_shutdown() {
    let e = env();
    let out = e
        .run(
            json!({"sleep": {"duration": "${! this.d }"}}),
            parts(&[r#"{"d":"1ms"}"#, r#"{"d":"-1s"}"#, r#"{"d":"soon"}"#, "x"]),
        )
        .await;
    assert_eq!(out[0].len(), 4);
    let p = e.proc(json!({"sleep": {"duration": "10s"}})).unwrap();
    let stop = e.stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        stop.send(true).unwrap();
    });
    assert!(matches!(
        p.process_batch(&e.ctx, parts(&["a"])).await,
        Err(CoreError::ComponentClosed)
    ));
}

#[tokio::test]
async fn log_passes_messages_through_at_every_level() {
    let e = env();
    for level in ["error", "warn", "debug", "trace", "info", "fatal"] {
        let out = e
            .run(
                json!({"log": {"level": level, "message": "${! this.n.number() }", "fields_mapping": "root.n = this.n"}}),
                parts(&[r#"{"n":1}"#, r#"{"n":"x"}"#]),
            )
            .await;
        assert_eq!(flat(&out), vec![r#"{"n":1}"#, r#"{"n":"x"}"#]);
        assert!(out[0].iter().all(|p| !p.is_errored()));
    }
    assert_eq!(e.run(json!({"log": {}}), parts(&["a"])).await[0].len(), 1);
}

#[tokio::test]
async fn resource_and_rate_limit_steps_resolve_in_the_org() {
    let e = env();
    let up = e
        .proc(json!({"mapping": "root = content().uppercase()"}))
        .unwrap();
    e.sctx.resources.insert_processor(e.ctx.org_id(), "up", up);
    assert_eq!(
        flat(&e.run(json!({"resource": "up"}), parts(&["a"])).await),
        vec!["A"]
    );

    // A resource declared later in processor_resources is accepted at build and refused at run.
    let mut late_ctx = e.ctx.clone();
    late_ctx.path = "processor_resources".into();
    let late_conf =
        ComponentConfig::from_json_value(&json!({"resource": "later"}), "noop", None).unwrap();
    let late = e.reg.build_processor(&late_conf, &late_ctx, "0").unwrap();
    let out = late.process_batch(&late_ctx, parts(&["a"])).await.unwrap();
    assert!(err_of(&out[0][0]).contains("processor resource 'later' was not found"));

    let out = e
        .run(json!({"rate_limit": "none_such"}), parts(&["a"]))
        .await;
    assert!(err_of(&out[0][0]).contains("rate limit resource 'none_such' was not found"));

    let c = ComponentConfig::from_json_value(
        &json!({"local": {"count": 1, "interval": "30ms"}}),
        "noop",
        None,
    )
    .unwrap();
    let rl = e.reg.build_rate_limit(&c, &e.sctx).unwrap();
    e.sctx
        .resources
        .insert_rate_limit(e.ctx.org_id(), "one", rl);
    let started = std::time::Instant::now();
    let out = e
        .run(
            json!({"rate_limit": {"resource": "one"}}),
            parts(&["a", "b"]),
        )
        .await;
    assert_eq!(out[0].len(), 2);
    assert!(
        started.elapsed() >= Duration::from_millis(20),
        "{:?}",
        started.elapsed()
    );

    let c = ComponentConfig::from_json_value(
        &json!({"local": {"count": 1, "interval": "10s"}}),
        "noop",
        None,
    )
    .unwrap();
    e.sctx.resources.insert_rate_limit(
        e.ctx.org_id(),
        "slow",
        e.reg.build_rate_limit(&c, &e.sctx).unwrap(),
    );
    let p = e.proc(json!({"rate_limit": "slow"})).unwrap();
    let stop = e.stop.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        stop.send(true).unwrap();
    });
    assert!(matches!(
        p.process_batch(&e.ctx, parts(&["a", "b"])).await,
        Err(CoreError::ComponentClosed)
    ));
}

#[tokio::test]
async fn cache_step_runs_every_operator_with_bento_errors() {
    let e = env();
    e.cache("c", json!({"memory": {}}));
    let op = |o: &str, extra: serde_json::Value| {
        let mut v = json!({"resource": "c", "operator": o, "key": "${! this.k }"});
        if let (Some(m), Some(x)) = (v.as_object_mut(), extra.as_object()) {
            m.extend(x.clone());
        }
        json!({ "cache": v })
    };
    let msg = |k: &str| Part::from_str(&format!(r#"{{"k":"{k}"}}"#));

    let out = e.run(op("set", json!({"ttl": "1h"})), vec![msg("a")]).await;
    assert!(!out[0][0].is_errored());
    let out = e
        .run(
            op("get", json!({})),
            vec![msg("a"), msg("zz"), Part::from_str("nojson")],
        )
        .await;
    assert_eq!(texts(&out[0])[0], r#"{"k":"a"}"#);
    assert_eq!(
        err_of(&out[0][1]),
        "operator failed for key 'zz': key does not exist"
    );
    assert!(out[0][2].is_errored());
    let out = e
        .run(op("add", json!({"value": "v"})), vec![msg("a"), msg("b")])
        .await;
    assert_eq!(err_of(&out[0][0]), "key already exists: a");
    assert!(!out[0][1].is_errored());
    let out = e.run(op("add", json!({})), vec![msg("c")]).await;
    assert!(!out[0][0].is_errored());
    let out = e
        .run(op("exists", json!({})), vec![msg("b"), msg("none")])
        .await;
    assert_eq!(texts(&out[0]), vec!["true", "false"]);
    let out = e.run(op("delete", json!({})), vec![msg("b")]).await;
    assert!(!out[0][0].is_errored());
    let out = e.run(op("exists", json!({})), vec![msg("b")]).await;
    assert_eq!(texts(&out[0]), vec!["false"]);
    let out = e.run(op("explode", json!({})), vec![msg("b")]).await;
    assert_eq!(err_of(&out[0][0]), "unknown cache operator 'explode'");
    for o in ["set", "add"] {
        let out = e
            .run(
                op(o, json!({"value": "${! this.v.number() }"})),
                vec![msg("q")],
            )
            .await;
        assert!(out[0][0].is_errored(), "{o}");
    }

    // A cache refusing writes flags the message with its reason.
    e.cache("full", json!({"multilevel": [{"memory": {}}]}));
    let ml_add = json!({"cache": {"resource": "full", "operator": "add", "key": "k"}});
    e.run(ml_add.clone(), parts(&["1"])).await;
    let out = e.run(ml_add, parts(&["2"])).await;
    assert_eq!(err_of(&out[0][0]), "key already exists: k");
}

#[tokio::test]
async fn dedupe_drops_repeats_and_honours_drop_on_err() {
    let e = env();
    e.cache("d", json!({"memory": {}}));
    let out = e
        .run(
            json!({"dedupe": {"cache": "d", "key": "${! this.id }"}}),
            parts(&[r#"{"id":1}"#, r#"{"id":1}"#, "bad"]),
        )
        .await;
    // A key that does not resolve drops the message (drop_on_err defaults to true, as in Bento).
    assert_eq!(flat(&out), vec![r#"{"id":1}"#]);

    e.cache("d2", json!({"memory": {}}));
    let out = e
        .run(json!({"dedupe": {"cache": "d2", "key": "${! this.id.number() }", "drop_on_err": false}}), parts(&[r#"{"id":"x"}"#]))
        .await;
    assert_eq!(out[0].len(), 1);
    e.cache("d3", json!({"memory": {}}));
    let out = e
        .run(
            json!({"dedupe": {"cache": "d3", "key": "${! this.id.number() }"}}),
            parts(&[r#"{"id":"x"}"#]),
        )
        .await;
    assert!(out.is_empty());
}

#[tokio::test]
async fn bounds_check_drops_the_whole_batch_when_one_part_is_out_of_bounds() {
    let e = env();
    let cfg = json!({"bounds_check": {"max_part_size": 3, "min_part_size": 1, "max_parts": 2, "min_parts": 1}});
    // Bento's bounds_check rejects the batch, it does not filter the offending part.
    assert!(e
        .run(cfg.clone(), parts(&["ok", "toolong"]))
        .await
        .is_empty());
    assert!(e.run(cfg.clone(), parts(&["a", "b", "c"])).await.is_empty());
    assert!(e.run(cfg.clone(), vec![]).await.is_empty());
    assert_eq!(
        flat(&e.run(cfg, parts(&["a", "bb"])).await),
        vec!["a", "bb"]
    );
}

#[tokio::test]
async fn part_selection_insertion_grouping_metrics_and_sync_response() {
    let e = env();
    let out = e
        .run(
            json!({"select_parts": {"parts": [-1, 0, 9]}}),
            parts(&["a", "b"]),
        )
        .await;
    assert_eq!(flat(&out), vec!["b", "a"]);
    assert!(e
        .run(json!({"select_parts": {"parts": [5]}}), parts(&["a"]))
        .await
        .is_empty());
    assert_eq!(
        flat(&e.run(json!({"select_parts": {}}), parts(&["a", "b"])).await),
        vec!["a"]
    );

    let out = e
        .run(
            json!({"insert_part": {"index": 0, "content": "x"}}),
            parts(&["a"]),
        )
        .await;
    assert_eq!(flat(&out), vec!["x", "a"]);
    let out = e
        .run(
            json!({"insert_part": {"index": -5, "content": "x"}}),
            parts(&["a"]),
        )
        .await;
    assert_eq!(flat(&out), vec!["x", "a"]);
    let out = e
        .run(json!({"insert_part": {"index": 9}}), parts(&["a"]))
        .await;
    assert_eq!(flat(&out), vec!["a", ""]);

    let out = e
        .run(
            json!({"group_by_value": {"value": "${! this.g }"}}),
            parts(&[r#"{"g":"x"}"#, r#"{"g":"y"}"#, r#"{"g":"x"}"#]),
        )
        .await;
    assert_eq!(out.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 1]);
    assert!(e
        .run(json!({"group_by_value": {"value": "x"}}), vec![])
        .await
        .is_empty());

    e.run(
        json!({"metric": {"name": "hits", "value": "${! this.n }"}}),
        parts(&[r#"{"n":3}"#, r#"{"n":"x"}"#]),
    )
    .await;
    e.run(json!({"metric": {}}), parts(&["a"])).await;
    assert_eq!(e.ctx.metrics.get_counter("hits"), 4);
    assert_eq!(e.ctx.metrics.get_counter("custom_metric"), 1);

    let store = SyncResponseStore::new();
    let mut p = Part::from_str("reply");
    p.with_sync_response_store(store.clone());
    e.run(json!({"sync_response": {}}), vec![p]).await;
    assert_eq!(texts(&store.read()[0]), vec!["reply"]);
    e.run(json!({"sync_response": {}}), parts(&["no store"]))
        .await;
}

// ---------- archive ----------

#[tokio::test]
async fn archive_and_unarchive_every_format() {
    let e = env();
    for fmt in ["tar", "zip"] {
        let out = e
            .run(json!({"archive": {"format": fmt, "path": "${! meta(\"name\") }${! content() }.txt"}}), parts(&["one", "two"]))
            .await;
        assert_eq!(
            out[0][0].collapsed_count(),
            2,
            "{fmt}: {:?}",
            out[0][0].error_get()
        );
        let back = e
            .run(json!({"unarchive": {"format": fmt}}), out[0].clone())
            .await;
        assert_eq!(texts(&back[0]), vec!["one", "two"]);
        assert_eq!(
            back[0][0].meta_get_str("archive_filename"),
            "nullone.txt",
            "{fmt}"
        );
    }
    let out = e
        .run(
            json!({"archive": {"format": "zip", "path": "same.txt"}}),
            parts(&["a", "b"]),
        )
        .await;
    assert_eq!(
        err_of(&out[0][0]),
        "zip archive member 'same.txt' failed: the name is used twice in this batch"
    );
    // A path that does not resolve names the member part_<index>, as Bento does.
    for fmt in ["tar", "zip"] {
        let out = e
            .run(
                json!({"archive": {"format": fmt, "path": "${! this.n.number() }"}}),
                parts(&["x"]),
            )
            .await;
        let back = e
            .run(json!({"unarchive": {"format": fmt}}), out[0].clone())
            .await;
        assert_eq!(
            back[0][0].meta_get_str("archive_filename"),
            "part_0",
            "{fmt}"
        );
    }
    let out = e
        .run(json!({"archive": {"format": "zip"}}), parts(&["z"]))
        .await;
    let back = e
        .run(json!({"unarchive": {"format": "zip"}}), out[0].clone())
        .await;
    assert_eq!(back[0][0].meta_get_str("archive_filename"), "part_0");

    let bin = e
        .run(json!({"archive": {"format": "binary"}}), parts(&["a", "b"]))
        .await;
    assert_eq!(
        texts(
            &e.run(json!({"unarchive": {"format": "binary"}}), bin[0].clone())
                .await[0]
        ),
        vec!["a", "b"]
    );
    assert_eq!(
        flat(
            &e.run(
                json!({"archive": {"format": "concatenate"}}),
                parts(&["a", "b"])
            )
            .await
        ),
        vec!["ab"]
    );
    assert_eq!(
        flat(
            &e.run(json!({"archive": {"format": "lines"}}), parts(&["a", "b"]))
                .await
        ),
        vec!["a\nb"]
    );
    assert_eq!(
        flat(
            &e.run(
                json!({"archive": {"format": "json_array"}}),
                parts(&["1", "{}"])
            )
            .await
        ),
        vec!["[1,{}]"]
    );
    assert!(e
        .run(json!({"archive": {"format": "lines"}}), vec![])
        .await
        .is_empty());

    // A part that is not JSON stops a json_array archive and is flagged.
    let out = e
        .run(
            json!({"archive": {"format": "json_array"}}),
            parts(&["1", "nope"]),
        )
        .await;
    assert_eq!(out[0].len(), 2);
    assert!(err_of(&out[0][1]).starts_with("failed to parse message as JSON"));

    // A tar member path climbing out of the archive is refused.
    let out = e
        .run(
            json!({"archive": {"format": "tar", "path": "../evil"}}),
            parts(&["x"]),
        )
        .await;
    assert!(out[0][0].is_errored());

    let out = e
        .run(
            json!({"unarchive": {"format": "json_map"}}),
            parts(&[r#"{"a":1,"b":2}"#]),
        )
        .await;
    assert_eq!(flat(&out), vec!["1", "2"]);
    assert_eq!(out[0][1].meta_get_str("archive_key"), "b");
    let out = e
        .run(
            json!({"unarchive": {"format": "json_documents"}}),
            parts(&["1 {\"a\":2}"]),
        )
        .await;
    assert_eq!(flat(&out), vec!["1", r#"{"a":2}"#]);
    let out = e
        .run(
            json!({"unarchive": {"format": "csv:;"}}),
            parts(&["a;b\n1;2"]),
        )
        .await;
    assert_eq!(flat(&out), vec![r#"{"a":"1","b":"2"}"#]);
    let out = e
        .run(
            json!({"unarchive": {"format": "xml_documents_to_json:cast"}}),
            parts(&["<a><b>1</b></a>"]),
        )
        .await;
    assert_eq!(flat(&out), vec![r#"{"a":{"b":1}}"#]);

    for (fmt, input, want) in [
        ("json_array", "{}", "invalid type 'object'"),
        ("json_array", "nope", "failed to parse message into JSON array"),
        ("json_map", "[]", "invalid type 'array'"),
        ("json_map", "nope", "failed to parse message into JSON map"),
        ("json_documents", "1 {", "EOF"),
        ("binary", "\u{0}\u{0}\u{0}\u{9}x", ""),
        ("tar", "this is not a tar archive but long enough to look at its header ..........................................................................................................................................................................................................................................................................................................................................................................................................................................................................................................................", ""),
        ("zip", "PK not a zip", ""),
        ("csv", "a,b\n1", "failed to parse message as csv"),
        ("xml_documents_to_json", "<a>", ""),
    ] {
        let out = e.run(json!({"unarchive": {"format": fmt}}), parts(&[input])).await;
        assert_eq!(out[0].len(), 1, "{fmt}");
        let msg = err_of(&out[0][0]);
        assert!(out[0][0].is_errored() && msg.contains(want), "{fmt}: {msg}");
    }
    assert!(e
        .run(json!({"unarchive": {"format": "lines"}}), vec![])
        .await
        .is_empty());
}

#[tokio::test]
async fn unarchive_member_caps_follow_the_org_quota() {
    let e = env();
    e.ctx.org.set_quotas(OrgQuotas {
        max_in_flight_messages: 1,
        ..OrgQuotas::default()
    });
    let many = (0..10_001).map(|i| i.to_string()).collect::<Vec<_>>();
    for (fmt, input) in [
        ("json_array", format!("[{}]", many.join(","))),
        ("json_documents", many.join(" ")),
        ("lines", many.join("\n")),
        ("csv", format!("h\n{}", many.join("\n"))),
    ] {
        let out = e
            .run(
                json!({"unarchive": {"format": fmt}}),
                vec![Part::from_str(&input)],
            )
            .await;
        assert!(
            err_of(&out[0][0]).contains("safety limit"),
            "{fmt}: {}",
            err_of(&out[0][0])
        );
    }
}

#[tokio::test]
async fn split_by_size_and_bytes() {
    let e = env();
    let out = e
        .run(json!({"split": {"size": 2}}), parts(&["a", "b", "c"]))
        .await;
    assert_eq!(out.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 1]);
    let out = e
        .run(
            json!({"split": {"size": 0, "byte_size": 3}}),
            parts(&["aa", "b", "cc"]),
        )
        .await;
    assert_eq!(out.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 1]);
    assert!(e.run(json!({"split": {}}), vec![]).await.is_empty());
}

#[tokio::test]
async fn xml_compress_and_decompress() {
    let e = env();
    let out = e
        .run(
            json!({"xml": {"cast": true}}),
            parts(&["<a>1</a>", "<broken"]),
        )
        .await;
    assert_eq!(texts(&out[0])[0], r#"{"a":1}"#);
    assert!(out[0][1].is_errored());

    for algo in ["gzip", "pgzip", "zlib", "flate"] {
        let packed = e
            .run(
                json!({"compress": {"algorithm": algo}}),
                parts(&["hello hello hello"]),
            )
            .await;
        let back = e
            .run(
                json!({"decompress": {"algorithm": algo}}),
                packed[0].clone(),
            )
            .await;
        assert_eq!(flat(&back), vec!["hello hello hello"], "{algo}");
        let bad = e
            .run(
                json!({"decompress": {"algorithm": algo}}),
                parts(&["not compressed at all"]),
            )
            .await;
        assert!(bad[0][0].is_errored(), "{algo}");
    }
    assert_eq!(
        flat(
            &e.run(
                json!({"decompress": {}}),
                e.run(json!({"compress": {}}), parts(&["d"])).await[0].clone()
            )
            .await
        ),
        vec!["d"]
    );
}

#[tokio::test]
async fn decompress_refuses_a_bomb() {
    let e = env();
    let zeros = Part::new(Bytes::from(vec![0u8; 129 * 1024 * 1024]));
    let packed = e
        .run(json!({"compress": {"algorithm": "zlib"}}), vec![zeros])
        .await;
    assert!(packed[0][0].as_bytes_ref().len() < 2 * 1024 * 1024);
    let out = e
        .run(
            json!({"decompress": {"algorithm": "zlib"}}),
            packed[0].clone(),
        )
        .await;
    assert!(
        err_of(&out[0][0]).contains("exceeds safety limit"),
        "{}",
        err_of(&out[0][0])
    );
}

// ---------- resources ----------

#[tokio::test]
async fn local_rate_limit_counts_messages_and_bytes() {
    let e = env();
    let build = |v: serde_json::Value| {
        let c = ComponentConfig::from_json_value(&v, "noop", None).unwrap();
        e.reg.build_rate_limit(&c, &e.sctx).unwrap()
    };
    let rl = build(json!({"local": {"count": 1, "interval": "30ms"}}));
    let p = parts(&["a"]);
    assert_eq!(rl.access(&p).await.unwrap(), Duration::ZERO);
    assert!(rl.access(&p).await.unwrap() > Duration::ZERO);
    tokio::time::sleep(Duration::from_millis(35)).await;
    assert_eq!(rl.access(&p).await.unwrap(), Duration::ZERO);

    let bytes = build(json!({"local": {"count": 0, "byte_size": 3, "interval": "1h"}}));
    assert_eq!(bytes.access(&parts(&["ab"])).await.unwrap(), Duration::ZERO);
    assert!(bytes.access(&parts(&["cd"])).await.unwrap() > Duration::ZERO);

    // A zero interval refills at once.
    let zero = build(json!({"local": {"count": 1, "interval": "0s"}}));
    assert_eq!(zero.access(&p).await.unwrap(), Duration::ZERO);
    assert_eq!(zero.access(&p).await.unwrap(), Duration::ZERO);
}

#[tokio::test]
async fn memory_noop_and_multilevel_caches() {
    let e = env();
    let m = e.cache(
        "m",
        json!({"memory": {"default_ttl": "20ms", "init_values": {"seed": "s"}}}),
    );
    assert_eq!(m.get("seed").await.unwrap(), Bytes::from_static(b"s"));
    m.add("k", Bytes::from_static(b"1"), None).await.unwrap();
    assert!(matches!(
        m.add("k", Bytes::new(), None).await,
        Err(CoreError::KeyAlreadyExists(_))
    ));
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(matches!(
        m.get("seed").await,
        Err(CoreError::KeyNotFound(_))
    ));
    m.add("k", Bytes::from_static(b"2"), Some(Duration::from_secs(60)))
        .await
        .unwrap();
    assert_eq!(m.get("k").await.unwrap(), Bytes::from_static(b"2"));
    m.delete("k").await.unwrap();
    assert!(!m.exists("k").await.unwrap());

    let capped = e.cache("capped", json!({"lru": {"cap": 1, "ttl": ""}}));
    capped
        .set("a", Bytes::from_static(b"1"), None)
        .await
        .unwrap();
    capped
        .set("b", Bytes::from_static(b"2"), None)
        .await
        .unwrap();
    capped
        .set("b", Bytes::from_static(b"3"), None)
        .await
        .unwrap();
    assert!(!capped.exists("a").await.unwrap());
    assert_eq!(capped.get("b").await.unwrap(), Bytes::from_static(b"3"));

    let noop = e.cache("n", json!({"noop": {}}));
    noop.set("a", Bytes::new(), None).await.unwrap();
    noop.add("a", Bytes::new(), None).await.unwrap();
    noop.delete("a").await.unwrap();
    assert!(matches!(
        noop.get("a").await,
        Err(CoreError::KeyNotFound(_))
    ));

    let ml = e.cache("ml", json!({"multilevel": ["m", {"memory": {}}]}));
    ml.set("x", Bytes::from_static(b"v"), None).await.unwrap();
    m.delete("x").await.unwrap();
    // A miss in the first tier is filled back from the second.
    assert_eq!(ml.get("x").await.unwrap(), Bytes::from_static(b"v"));
    assert_eq!(m.get("x").await.unwrap(), Bytes::from_static(b"v"));
    ml.add("y", Bytes::from_static(b"1"), None).await.unwrap();
    ml.delete("y").await.unwrap();
    assert!(matches!(ml.get("y").await, Err(CoreError::KeyNotFound(_))));
}

// ---------- mapping ----------

#[test]
fn optional_mappings_compile_only_when_present() {
    let e = env();
    let conf = |v: serde_json::Value| ComponentConfig {
        config: v,
        ..ComponentConfig::default()
    };
    let opt = ekiben_processors::mapping::compile_optional_mapping;
    assert!(opt(&conf(json!({})), "m", &e.ctx).unwrap().is_none());
    assert!(opt(&conf(json!({"m": "  "})), "m", &e.ctx)
        .unwrap()
        .is_none());
    assert!(opt(&conf(json!({"m": "root = 1"})), "m", &e.ctx)
        .unwrap()
        .is_some());
    assert!(opt(&conf(json!({"m": "root = ("})), "m", &e.ctx).is_err());
}

#[test]
fn every_processor_reports_its_type() {
    let e = env();
    e.cache("c", json!({"memory": {}}));
    let up = e.proc(json!({"noop": {}})).unwrap();
    e.sctx.resources.insert_processor(e.ctx.org_id(), "r", up);
    for (cfg, ty) in [
        (json!({"noop": {}}), "noop"),
        (json!({"try": []}), "try"),
        (json!({"catch": []}), "catch"),
        (json!({"for_each": []}), "for_each"),
        (json!({"processors": [{"noop": {}}]}), "processors"),
        (json!({"while": {"check": "false"}}), "while"),
        (json!({"parallel": {}}), "parallel"),
        (json!({"retry": {"processors": [{"noop": {}}]}}), "retry"),
        (json!({"branch": {"processors": [{"noop": {}}]}}), "branch"),
        (
            json!({"switch": [{"processors": [{"noop": {}}]}]}),
            "switch",
        ),
        (json!({"workflow": {}}), "workflow"),
        (json!({"mapping": "root = this"}), "mapping"),
        (json!({"bloblang": "root = this"}), "bloblang"),
        (json!({"mutation": "root.a = 1"}), "mutation"),
        (json!({"sleep": {"duration": "1ms"}}), "sleep"),
        (json!({"log": {}}), "log"),
        (json!({"resource": "r"}), "resource"),
        (json!({"rate_limit": "x"}), "rate_limit"),
        (
            json!({"cache": {"resource": "c", "operator": "get", "key": "k"}}),
            "cache",
        ),
        (
            json!({"dedupe": {"cache": "c", "key": "k", "strategy": "LIFO"}}),
            "dedupe",
        ),
        (json!({"bounds_check": {}}), "bounds_check"),
        (json!({"group_by_value": {"value": "x"}}), "group_by_value"),
        (json!({"insert_part": {}}), "insert_part"),
        (json!({"select_parts": {}}), "select_parts"),
        (json!({"metric": {}}), "metric"),
        (json!({"sync_response": {}}), "sync_response"),
        (json!({"archive": {"format": "lines"}}), "archive"),
        (json!({"unarchive": {"format": "lines"}}), "unarchive"),
        (json!({"split": {}}), "split"),
        (json!({"xml": {}}), "xml"),
        (json!({"compress": {}}), "compress"),
        (json!({"decompress": {}}), "decompress"),
    ] {
        assert_eq!(e.proc(cfg).unwrap().processor_type(), ty);
    }
}
