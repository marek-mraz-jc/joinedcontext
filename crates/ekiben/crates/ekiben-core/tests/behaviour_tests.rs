//! Behaviour tests for the engine pieces the other suites reach only indirectly: config parsing,
//! the registry and its lint, org-scoped resources, the processor wrappers, the batcher, the
//! worker pool and its ack bookkeeping, metrics, redaction and the message helpers.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use ekiben_bloblang::{Environment, MessageBatch, PartMetaMut, Value};
use ekiben_core::config::{
    build_blobl_env_for_stream, extract_metadata_filter, merge_json_patch, parse_duration_str,
};
use ekiben_core::{
    interpolate_env_bytes, redact_config_value, AckResponder, BackoffConfig, Batch, BatchError,
    BatchPolicyConfig, Batcher, Buffer, Cache, ComponentConfig, CoreError, DetailedError, Input,
    MetricsRegistry, OrgId, OrgQuotas, Output, Part, PartMetaAdapter, PartsView, PipelineConfig,
    PipelineWorkerPool, ProcContext, Processor, RateLimit, Registry, ResourceManager, RunMode,
    SecretString, SortGroup, StreamConfig, StreamContext, StreamMetrics, SyncResponseStore,
    TrackedTransaction, Transaction,
};
use serde_json::json;
use tokio::sync::{mpsc, watch};

// ---------- test components ----------

/// Upper-cases every part.
struct Upper;
#[async_trait]
impl Processor for Upper {
    fn processor_type(&self) -> &'static str {
        "upper"
    }
    async fn process_batch(&self, _: &ProcContext, mut b: Batch) -> Result<Vec<Batch>, CoreError> {
        for p in &mut b {
            let s = String::from_utf8_lossy(&p.as_bytes()).to_uppercase();
            p.set_bytes(s);
        }
        Ok(vec![b])
    }
}

/// Splits a batch into one batch per part.
struct Split;
#[async_trait]
impl Processor for Split {
    fn processor_type(&self) -> &'static str {
        "split_each"
    }
    async fn process_batch(&self, _: &ProcContext, b: Batch) -> Result<Vec<Batch>, CoreError> {
        Ok(b.into_iter().map(|p| vec![p]).collect())
    }
}

/// Drops every batch.
struct Dropper;
#[async_trait]
impl Processor for Dropper {
    fn processor_type(&self) -> &'static str {
        "drop_all"
    }
    async fn process_batch(&self, _: &ProcContext, _: Batch) -> Result<Vec<Batch>, CoreError> {
        Ok(Vec::new())
    }
}

/// Fails with the error it was built with.
struct Fail(fn() -> CoreError);
#[async_trait]
impl Processor for Fail {
    fn processor_type(&self) -> &'static str {
        "fail"
    }
    async fn process_batch(&self, _: &ProcContext, _: Batch) -> Result<Vec<Batch>, CoreError> {
        Err((self.0)())
    }
}

/// Counts close calls; keeps every trait default otherwise.
struct Closing(Arc<AtomicUsize>);
#[async_trait]
impl Processor for Closing {
    fn processor_type(&self) -> &'static str {
        "closing"
    }
    async fn process_batch(&self, _: &ProcContext, b: Batch) -> Result<Vec<Batch>, CoreError> {
        Ok(vec![b])
    }
    async fn close(&self) -> Result<(), CoreError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Uses every `Processor` default.
struct Plain;
#[async_trait]
impl Processor for Plain {
    fn processor_type(&self) -> &'static str {
        "plain"
    }
    async fn process_batch(&self, _: &ProcContext, b: Batch) -> Result<Vec<Batch>, CoreError> {
        Ok(vec![b])
    }
}

/// Input serving a fixed queue of batches, acking into a shared log; trait defaults otherwise.
struct QueueInput {
    batches: VecDeque<Batch>,
    acks: Arc<Mutex<Vec<Option<String>>>>,
}
#[async_trait]
impl Input for QueueInput {
    fn input_type(&self) -> &'static str {
        "queue"
    }
    async fn read_batch(&mut self, _: &StreamContext) -> Result<(Batch, AckResponder), CoreError> {
        let b = self.batches.pop_front().ok_or(CoreError::EndOfInput)?;
        let acks = self.acks.clone();
        Ok((
            b,
            AckResponder::from_fn(move |e| acks.lock().unwrap().push(e.map(|e| e.to_string()))),
        ))
    }
}

/// Output collecting what it was given; trait defaults otherwise.
#[derive(Default)]
struct Collect(Mutex<Vec<Batch>>);
#[async_trait]
impl Output for Collect {
    fn output_type(&self) -> &'static str {
        "collect"
    }
    async fn write_batch(&self, _: &StreamContext, b: Batch) -> Result<(), CoreError> {
        self.0.lock().unwrap().push(b);
        Ok(())
    }
}

/// Map cache with the trait's `exists` and `close` defaults.
#[derive(Default)]
struct MapCache(Mutex<HashMap<String, Bytes>>);
#[async_trait]
impl Cache for MapCache {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError> {
        if key == "broken" {
            return Err(CoreError::Timeout);
        }
        self.0
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or_else(|| CoreError::KeyNotFound(key.to_string()))
    }
    async fn set(&self, key: &str, value: Bytes, _: Option<Duration>) -> Result<(), CoreError> {
        self.0.lock().unwrap().insert(key.to_string(), value);
        Ok(())
    }
    async fn add(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError> {
        self.set(key, value, ttl).await
    }
    async fn delete(&self, key: &str) -> Result<(), CoreError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

struct NoWait;
#[async_trait]
impl RateLimit for NoWait {
    async fn access(&self, _: &[Part]) -> Result<Duration, CoreError> {
        Ok(Duration::ZERO)
    }
}

struct NoBuffer;
#[async_trait]
impl Buffer for NoBuffer {
    async fn push(&self, _: Transaction) -> Result<(), CoreError> {
        Ok(())
    }
    async fn pop(&self, _: &StreamContext) -> Result<Transaction, CoreError> {
        Err(CoreError::EndOfInput)
    }
}

// ---------- helpers ----------

fn registry() -> Arc<Registry> {
    let reg = Registry::new();
    reg.register_processor("upper", |_, _| Ok(Arc::new(Upper) as Arc<dyn Processor>));
    reg.register_processor("split_each", |_, _| {
        Ok(Arc::new(Split) as Arc<dyn Processor>)
    });
    reg.register_processor("drop_all", |_, _| {
        Ok(Arc::new(Dropper) as Arc<dyn Processor>)
    });
    reg.register_processor("mapping", |_, _| Ok(Arc::new(Plain) as Arc<dyn Processor>));
    reg.register_processor("branch", |_, _| Ok(Arc::new(Plain) as Arc<dyn Processor>));
    reg.register_processor("catch", |_, _| Ok(Arc::new(Plain) as Arc<dyn Processor>));
    reg.register_processor("try", |_, _| Ok(Arc::new(Plain) as Arc<dyn Processor>));
    reg.register_processor("resource", |_, _| Ok(Arc::new(Plain) as Arc<dyn Processor>));
    reg.register_processor("refuse", |c, _| {
        Err(CoreError::config(format!("refused {}", c.label)))
    });
    reg.register_input("generate", |_, _| {
        Ok(Box::new(QueueInput {
            batches: VecDeque::new(),
            acks: Arc::default(),
        }) as Box<dyn Input>)
    });
    reg.register_output("collect", |_, _| {
        Ok(Arc::new(Collect::default()) as Arc<dyn Output>)
    });
    reg.register_cache("memory", |_, _| {
        Ok(Arc::new(MapCache::default()) as Arc<dyn Cache>)
    });
    reg.register_rate_limit("local", |_, _| Ok(Arc::new(NoWait) as Arc<dyn RateLimit>));
    reg.register_buffer("none", |_, _| Ok(Arc::new(NoBuffer) as Arc<dyn Buffer>));
    Arc::new(reg)
}

fn stream_ctx(reg: Arc<Registry>) -> (watch::Sender<bool>, StreamContext) {
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("o1"));
    let (tx, rx) = watch::channel(false);
    (
        tx,
        StreamContext {
            stream_id: "s1".into(),
            org,
            resources,
            registry: reg,
            blobl_env: Environment::new(),
            metrics: StreamMetrics::new("s1", "o1"),
            label: "root".into(),
            path: String::new(),
            file_root: None,
            env_vars: Arc::default(),
            shutdown_rx: rx,
        },
    )
}

fn proc_ctx() -> (watch::Sender<bool>, ProcContext) {
    let (tx, s) = stream_ctx(registry());
    let p = s.to_proc_context("pipeline", "", "pipeline.processors");
    (tx, p)
}

fn texts(b: &[Part]) -> Vec<String> {
    b.iter()
        .map(|p| String::from_utf8_lossy(&p.as_bytes_ref()).into_owned())
        .collect()
}

fn parts(xs: &[&str]) -> Batch {
    xs.iter().map(|s| Part::from_str(s)).collect()
}

fn conf(v: serde_json::Value) -> ComponentConfig {
    ComponentConfig::from_json_value(&v, "noop", None).unwrap()
}

// ---------- config ----------

#[test]
fn component_config_follows_bento_type_inference() {
    // A component with explicit `type` takes its config from the key named by the type.
    let c = conf(json!({"type": "http_client", "http_client": {"url": "x"}, "label": "a"}));
    assert_eq!(
        (c.component_type.as_str(), c.config.clone()),
        ("http_client", json!({"url": "x"}))
    );
    // ... or from `plugin`
    let c = conf(json!({"type": "custom", "plugin": {"k": 1}}));
    assert_eq!(c.config, json!({"k": 1}));
    // ... or from the remaining fields.
    let c = conf(json!({"type": "stdout", "codec": "lines", "processors": [{"upper": {}}]}));
    assert_eq!(c.config, json!({"codec": "lines"}));
    assert_eq!(c.processors[0].component_type, "upper");

    // An explicit type the known-types check refuses falls back to key inference.
    let known = |t: &str| t == "mapping";
    let c = ComponentConfig::from_json_value(
        &json!({"type": "nonsense", "mapping": "root = this"}),
        "noop",
        Some(&known),
    )
    .unwrap();
    assert_eq!(c.component_type, "mapping");

    // `processors` alone names the composite processor.
    let c = conf(json!({"processors": [{"upper": {}}], "label": "p"}));
    assert_eq!(
        (c.component_type.as_str(), c.label.as_str()),
        ("processors", "p")
    );
    assert!(c.processors.is_empty());

    // An empty object is the default type; a non-object is refused with what it got.
    assert_eq!(conf(json!({})).component_type, "noop");
    let e = ComponentConfig::from_json_value(&json!(5), "noop", None).unwrap_err();
    assert!(e.to_string().contains("expected object"), "{e}");

    // `processors` beside a type must be an array.
    let e = ComponentConfig::from_json_value(&json!({"upper": {}, "processors": 3}), "noop", None)
        .unwrap_err();
    assert!(e.to_string().contains("must be an array"), "{e}");

    // Two candidate keys are refused unless the known-types check picks exactly one.
    let e = ComponentConfig::from_json_value(&json!({"a": 1, "b": 2}), "noop", None).unwrap_err();
    assert!(e.to_string().contains("unable to infer"), "{e}");
    let known = |t: &str| t == "b";
    let c =
        ComponentConfig::from_json_value(&json!({"a": 1, "b": 2}), "noop", Some(&known)).unwrap();
    assert_eq!(
        (c.component_type.as_str(), c.config.clone()),
        ("b", json!(2))
    );

    let d = ComponentConfig::default();
    assert_eq!(
        (d.component_type.as_str(), d.config.clone()),
        ("noop", json!({}))
    );
}

#[test]
fn pipeline_and_batch_policy_defaults_match_bento() {
    let p: PipelineConfig = serde_json::from_value(json!({})).unwrap();
    assert_eq!(p.threads, -1);
    assert_eq!(PipelineConfig::default().threads, -1);

    let noop = BatchPolicyConfig::from_json_value(&json!("not an object")).unwrap();
    assert!(noop.is_noop() && !noop.is_limited());
    let b = BatchPolicyConfig::from_json_value(&json!({
        "count": 3, "byte_size": 10, "period": "1s", "jitter": 0.5, "check": "root = true",
        "processors": [{"upper": {}}]
    }))
    .unwrap();
    assert_eq!((b.count, b.byte_size, b.period.as_str()), (3, 10, "1s"));
    assert!(!b.is_noop() && b.is_limited());
    assert_eq!(b.processors[0].component_type, "upper");
    // A count of one with nothing else is Bento's no-op policy.
    let one = BatchPolicyConfig::from_json_value(&json!({"count": 1})).unwrap();
    assert!(one.is_noop() && one.is_limited());

    let e = BatchPolicyConfig::from_json_value(&json!({"jitter": -1.0})).unwrap_err();
    assert!(e.to_string().contains("jitter"), "{e}");
    let e = BatchPolicyConfig::from_json_value(&json!({"processors": [7]})).unwrap_err();
    assert!(e.to_string().contains("expected object"), "{e}");
}

#[test]
fn backoff_grows_doubles_caps_and_gives_up_after_max_elapsed() {
    let d = BackoffConfig::default();
    assert_eq!(d.initial_interval, Duration::from_millis(500));

    let b = BackoffConfig::from_json_value(
        &json!({"initial_interval": "100ms", "max_interval": "1s", "max_elapsed_time": "5s"}),
        Duration::from_secs(30),
    )
    .unwrap();
    assert_eq!(
        b.next_backoff(0, Duration::ZERO),
        Some(Duration::from_millis(100))
    );
    assert_eq!(
        b.next_backoff(2, Duration::ZERO),
        Some(Duration::from_millis(400))
    );
    assert_eq!(
        b.next_backoff(30, Duration::ZERO),
        Some(Duration::from_secs(1))
    );
    assert_eq!(b.next_backoff(1, Duration::from_secs(5)), None);

    let keep = BackoffConfig::from_json_value(&json!(null), Duration::from_secs(9)).unwrap();
    assert_eq!(keep.max_elapsed_time, Duration::from_secs(9));
    let e = BackoffConfig::from_json_value(&json!({"max_interval": "soon"}), Duration::ZERO)
        .unwrap_err();
    assert!(matches!(e, CoreError::Config(_)), "{e}");
}

#[test]
fn durations_parse_as_go_does() {
    assert_eq!(parse_duration_str(" ").unwrap(), Duration::ZERO);
    assert_eq!(parse_duration_str("0s").unwrap(), Duration::ZERO);
    assert_eq!(
        parse_duration_str("1m30s").unwrap(),
        Duration::from_secs(90)
    );
    assert_eq!(parse_duration_str("-5s").unwrap(), Duration::ZERO);
    assert!(parse_duration_str("five").is_err());
}

#[test]
fn stream_config_parses_every_section() {
    let raw = r#"
org_id: city
project: p1
run: once
schedule: "@every 5m"
secret_refs: [a, 7, b]
input:
  generate: {mapping: "root = 1"}
buffer:
  memory: {}
pipeline:
  threads: 2
  processors:
    - upper: {}
output:
  collect: {}
cache_resources:
  - label: c
    memory: {}
rate_limit_resources:
  - label: r
    local: {}
"#;
    let c = StreamConfig::from_yaml_or_json(raw).unwrap();
    assert_eq!(c.org, OrgId::new("city"));
    assert_eq!(c.project.as_deref(), Some("p1"));
    assert_eq!(c.run, RunMode::Once);
    let s = c.schedule.clone().unwrap();
    assert_eq!(s.cron.as_deref(), Some("@every 5m"));
    assert_eq!(c.secret_refs, vec!["a", "b"]);
    assert_eq!(c.buffer.component_type, "memory");
    assert_eq!(c.pipeline.threads, 2);
    assert_eq!(c.resources.cache_resources[0].label, "c");
    assert_eq!(c.resources.rate_limit_resources[0].component_type, "local");

    // A period-shaped schedule string is a period, anything else a cron line.
    let p = StreamConfig::from_json_value(&json!({"schedule": "30s"})).unwrap();
    assert_eq!(p.schedule.unwrap().period.as_deref(), Some("30s"));
    let cron = StreamConfig::from_json_value(&json!({"schedule": "0 * * * *"})).unwrap();
    assert_eq!(cron.schedule.unwrap().cron.as_deref(), Some("0 * * * *"));
    let e =
        StreamConfig::from_json_value(&json!({"schedule": {"nope": 1, "period": 5}})).unwrap_err();
    assert!(e.to_string().contains("invalid schedule config"), "{e}");

    // Missing sections take Bento's defaults; a non-object is refused.
    let d = StreamConfig::from_json_value(&json!({})).unwrap();
    assert_eq!(
        (
            d.input.component_type.as_str(),
            d.output.component_type.as_str()
        ),
        ("inproc", "stdout")
    );
    assert_eq!(d.run, RunMode::Resident);
    assert!(StreamConfig::from_json_value(&json!([])).is_err());
    let def = StreamConfig::default();
    assert_eq!(def.input.component_type, "stdin");

    // A bad resource list item is refused.
    let e = StreamConfig::from_json_value(&json!({"processor_resources": [1]})).unwrap_err();
    assert!(e.to_string().contains("expected object"), "{e}");
}

#[test]
fn stream_config_env_lint_and_yaml_errors() {
    let raw = "input:\n  generate: {mapping: \"${UNSET_T6652}\"}\n";
    let e = StreamConfig::parse_str_with_env(raw, false, |_| None).unwrap_err();
    assert!(e.to_string().contains("UNSET_T6652"), "{e}");
    let (c, lints) = StreamConfig::parse_str_with_env(raw, true, |_| None).unwrap();
    assert_eq!(lints.len(), 1);
    assert_eq!(c.input.component_type, "generate");

    let e = StreamConfig::parse_str_with_env("input: [unclosed", true, |_| None).unwrap_err();
    assert!(e.to_string().contains("failed to parse YAML/JSON"), "{e}");
}

#[test]
fn an_interpolated_secret_never_reaches_a_parse_error() {
    // The password comes from the environment; the YAML around it is broken.
    let raw = "output:\n  http_client: {url: \"https://u:${PW}@host\"}\n  [broken";
    let e = StreamConfig::parse_str_with_env(raw, false, |k| {
        (k == "PW").then(|| "hunter2-t6652".to_string())
    })
    .unwrap_err();
    assert!(!e.to_string().contains("hunter2-t6652"), "{e}");
}

#[test]
fn metadata_filter_includes_excludes_and_patterns() {
    let f = extract_metadata_filter(&json!({
        "include_prefixes": ["kafka_", 3],
        "include_patterns": ["^x-.*$", "("],
        "exclude_prefixes": ["kafka_key", ""]
    }));
    assert!(!f.is_empty());
    assert!(f.matches("kafka_topic"));
    assert!(!f.matches("kafka_key"));
    assert!(f.matches("x-trace"));
    assert!(!f.matches("other"));
    let all = extract_metadata_filter(&json!({"include_prefixes": ["*"]}));
    assert!(all.matches("anything"));
    assert!(extract_metadata_filter(&json!(null)).is_empty());
}

#[test]
fn json_patch_merges_objects_and_replaces_the_rest() {
    let mut d = json!({"a": {"b": 1, "c": 2}, "x": [1]});
    merge_json_patch(&mut d, &json!({"a": {"b": 9}, "x": "s", "n": 1}));
    assert_eq!(d, json!({"a": {"b": 9, "c": 2}, "x": "s", "n": 1}));
    let mut s = json!(1);
    merge_json_patch(&mut s, &json!({"k": 1}));
    assert_eq!(s, json!({"k": 1}));
}

#[test]
fn a_stream_bloblang_env_reads_only_the_stream_variables() {
    let vars = HashMap::from([("CITY".to_string(), "praha".to_string())]);
    let env = build_blobl_env_for_stream(&vars, false);
    let exec = env.compile_mapping(r#"root = env("CITY")"#).unwrap();
    let batch = parts(&["{}"]);
    let mut meta = batch[0].meta_all().clone();
    let v = exec
        .map_part(0, &PartsView(&batch), &mut meta, None)
        .unwrap()
        .unwrap();
    assert_eq!(v, Value::String("praha".into()));
}

// ---------- registry ----------

#[test]
fn registry_lists_and_builds_every_kind() {
    let reg = registry();
    let names = reg.component_names();
    assert_eq!(names["buffers"], vec!["none"]);
    assert_eq!(names["caches"], vec!["memory"]);
    assert_eq!(names["rate-limits"], vec!["local"]);
    assert!(reg.has_processor("upper") && !reg.has_processor("nope"));
    assert!(reg.has_input("generate") && reg.has_output("collect"));
    assert!(reg.has_cache("memory") && reg.has_rate_limit("local"));

    let (_tx, s) = stream_ctx(reg.clone());
    let unknown = conf(json!({"nope": {}}));
    let e = reg.build_input(&unknown, &s).err().unwrap();
    assert!(e.to_string().contains("not recognised as an input"), "{e}");
    let e = reg.build_output(&unknown, &s).err().unwrap();
    assert!(e.to_string().contains("not recognised as an output"), "{e}");
    let e = reg.build_cache(&unknown, &s).err().unwrap();
    assert!(e.to_string().contains("not recognised as a cache"), "{e}");
    let e = reg.build_rate_limit(&unknown, &s).err().unwrap();
    assert!(
        e.to_string().contains("not recognised as a rate_limit"),
        "{e}"
    );
    let p = s.to_proc_context("pipeline", "", "");
    let e = reg.build_processor(&unknown, &p, "0").err().unwrap();
    assert!(
        e.to_string().contains("not recognised as a processor"),
        "{e}"
    );
    let e = reg
        .build_processor_list(&[conf(json!({"label": "x", "refuse": {}}))], &p)
        .err()
        .unwrap();
    assert!(e.to_string().contains("refused x"), "{e}");

    let i = reg
        .build_input(&conf(json!({"label": "in", "generate": {}})), &s)
        .unwrap();
    assert_eq!(i.input_type(), "queue");
    let o = reg.build_output(&conf(json!({"collect": {}})), &s).unwrap();
    assert_eq!(o.output_type(), "collect");
    assert!(reg.build_cache(&conf(json!({"memory": {}})), &s).is_ok());
    assert!(reg
        .build_rate_limit(&conf(json!({"local": {}})), &s)
        .is_ok());

    // With processors the input and output come back wrapped, keeping the inner type and label.
    let i = reg
        .build_input(
            &conf(json!({"generate": {}, "processors": [{"upper": {}}]})),
            &s,
        )
        .unwrap();
    assert_eq!((i.input_type(), i.label()), ("queue", ""));
    assert!(i.is_connected());
    let o = reg
        .build_output(
            &conf(json!({"collect": {}, "processors": [{"upper": {}}]})),
            &s,
        )
        .unwrap();
    assert_eq!((o.output_type(), o.label()), ("collect", ""));
    assert!(o.is_connected());
    let e = reg
        .build_output(
            &conf(json!({"collect": {}, "processors": [{"nope": {}}]})),
            &s,
        )
        .err()
        .unwrap();
    assert!(
        e.to_string().contains("not recognised as a processor"),
        "{e}"
    );
}

#[test]
fn lint_reports_what_bento_lint_reports() {
    let reg = registry();
    let raw = json!({
        "input": {"label": "dup", "generate": {"mapping": "root = ", "check": "root = true"}},
        "pipeline": {"processors": [
            {"label": "dup", "upper": {}},
            {"nope": {}},
            {"mapping": "root = this.("},
            {"mapping": {"not": "a string"}},
            {"branch": {"request_map": "root = (", "result_map": "", "processors": [{"nope2": {}}]}},
            {"catch": [{"try": []}]},
            {"catch": [{"catch": []}, {"try": [{"mapping": "root = )"}]}]},
            {"label": "r", "resource": "x"}
        ]},
        "output": {"unknown_out": {}}
    });
    let c = StreamConfig::from_json_value(&raw).unwrap();
    let r = reg.lint_stream_config(&c, vec!["environment variable 'X' was not defined".into()]);
    assert!(!r.is_ok());
    assert_eq!(r.lint_errors, r.linting_errors);
    let all = r.lint_errors.join("\n");
    for want in [
        "environment variable 'X'",
        "duplicate component label 'dup'",
        "input generate.mapping",
        "processor: type 'nope' was not recognised",
        "processor mapping:",
        "processor mapping requires a string mapping",
        "branch.request_map",
        "processor: type 'nope2' was not recognised",
        "'catch' block contains a 'try' block",
        "a 'resource' component cannot have a 'label' field",
        "output: type 'unknown_out' was not recognised",
    ] {
        assert!(all.contains(want), "missing {want:?} in\n{all}");
    }
    // The catch that clears errors first lets its try run: only one try warning.
    assert_eq!(all.matches("'catch' block contains").count(), 1, "{all}");
    // A try's children are linted too.
    assert_eq!(all.matches("processor mapping:").count(), 2, "{all}");

    let clean = StreamConfig::from_json_value(&json!({
        "input": {"generate": {}}, "output": {"collect": {}}
    }))
    .unwrap();
    assert!(reg.lint_stream_config(&clean, vec![]).is_ok());
}

// ---------- org resources ----------

#[tokio::test]
async fn org_resources_are_scoped_by_org() {
    let rm = ResourceManager::new();
    let a = OrgId::new("a");
    let b = OrgId::new("b");
    assert_eq!(OrgId::default().as_str(), "default");

    rm.insert_cache(&a, "c", Arc::new(MapCache::default()));
    rm.insert_rate_limit(&a, "r", Arc::new(NoWait));
    rm.insert_processor(&a, "p", Arc::new(Plain));
    rm.insert_output(&a, "o", Arc::new(Collect::default()));
    rm.insert_input(
        &a,
        "i",
        Box::new(QueueInput {
            batches: VecDeque::from([parts(&["x"])]),
            acks: Arc::default(),
        }),
    );

    assert!(rm.has_cache(&a, "c") && !rm.has_cache(&b, "c"));
    assert!(rm.has_rate_limit(&a, "r") && !rm.has_rate_limit(&b, "r"));
    assert!(rm.has_processor(&a, "p") && !rm.has_processor(&b, "p"));
    assert!(rm.has_input(&a, "i") && !rm.has_input(&b, "i"));
    assert!(rm.has_output(&a, "o") && !rm.has_output(&b, "o"));

    assert!(rm.get_processor(&a, "p").is_ok());
    assert!(rm.get_output(&a, "o").is_ok());
    let (_tx, s) = stream_ctx(registry());
    let input = rm.get_input(&a, "i").unwrap();
    let (got, _) = input.lock().await.read_batch(&s).await.unwrap();
    assert_eq!(texts(&got), vec!["x"]);

    for (kind, e) in [
        ("cache", rm.get_cache(&b, "c").err().unwrap()),
        ("rate limit", rm.get_rate_limit(&b, "r").err().unwrap()),
        ("processor", rm.get_processor(&b, "p").err().unwrap()),
        ("input", rm.get_input(&b, "i").err().unwrap()),
        ("output", rm.get_output(&b, "o").err().unwrap()),
    ] {
        let m = e.to_string();
        assert!(m.contains(kind) && m.contains("organization 'b'"), "{m}");
    }

    let org = rm.set_org_quotas(
        &a,
        OrgQuotas {
            max_concurrent_streams: 7,
            ..OrgQuotas::default()
        },
    );
    assert_eq!(org.get_quotas().max_concurrent_streams, 7);
    org.set_secret("db", "s3cr3t");
    assert_eq!(org.resolve_secret("db").as_deref(), Some("s3cr3t"));
    assert_eq!(org.resolve_secret("other"), None);
    org.clear_secrets();
    assert_eq!(org.resolve_secret("db"), None);
}

// ---------- traits ----------

#[tokio::test]
async fn trait_defaults_and_context_helpers() {
    let (tx, s) = stream_ctx(registry());
    assert_eq!(s.org_id().as_str(), "o1");
    assert!(!s.is_shutting_down());
    let p = s.to_proc_context("mapping", "m", "pipeline.processors.0");
    assert_eq!(
        (p.label.as_str(), p.path.as_str()),
        ("m", "pipeline.processors.0")
    );

    let plain = Plain;
    assert_eq!(plain.label(), "");
    assert!(plain.close().await.is_ok());

    let mut q = QueueInput {
        batches: VecDeque::new(),
        acks: Arc::default(),
    };
    assert_eq!(q.label(), "");
    assert!(q.connect(&s).await.is_ok());
    assert!(q.is_connected());
    assert!(matches!(q.read_batch(&s).await, Err(CoreError::EndOfInput)));
    assert!(q.close().await.is_ok());

    let out = Collect::default();
    assert_eq!(out.label(), "");
    assert!(out.connect(&s).await.is_ok());
    assert!(out.is_connected());
    assert!(out.close().await.is_ok());

    let cache = MapCache::default();
    cache
        .set("k", Bytes::from_static(b"v"), None)
        .await
        .unwrap();
    assert!(cache.exists("k").await.unwrap());
    assert!(!cache.exists("missing").await.unwrap());
    assert!(matches!(
        cache.exists("broken").await,
        Err(CoreError::Timeout)
    ));
    assert!(cache.close().await.is_ok());

    assert!(NoWait.close().await.is_ok());
    assert!(NoBuffer.close().await.is_ok());
    assert!(NoBuffer
        .push(Transaction::new(vec![], AckResponder::noop()))
        .await
        .is_ok());
    assert!(NoBuffer.pop(&s).await.is_err());

    tx.send(true).unwrap();
    assert!(s.is_shutting_down());
}

// ---------- pipeline ----------

#[tokio::test]
async fn observed_processor_marks_a_failed_batch_and_passes_quota_and_close_through() {
    let (_tx, p) = proc_ctx();
    let reg = p.registry.clone();
    let closes = Arc::new(AtomicUsize::new(0));
    let c2 = closes.clone();
    reg.register_processor("closing", move |_, _| {
        Ok(Arc::new(Closing(c2.clone())) as Arc<dyn Processor>)
    });
    reg.register_processor("bad", |_, _| {
        Ok(Arc::new(Fail(|| CoreError::other("upstream 503"))) as Arc<dyn Processor>)
    });
    reg.register_processor("quota", |_, _| {
        Ok(Arc::new(Fail(|| CoreError::QuotaExceeded {
            org: "o1".into(),
            reason: "r".into(),
        })) as Arc<dyn Processor>)
    });
    reg.register_processor("closed", |_, _| {
        Ok(Arc::new(Fail(|| CoreError::ComponentClosed)) as Arc<dyn Processor>)
    });

    let obs = reg
        .build_processor(&conf(json!({"label": "c", "closing": {}})), &p, "0")
        .unwrap();
    assert_eq!((obs.processor_type(), obs.label()), ("closing", "c"));
    obs.close().await.unwrap();
    assert_eq!(closes.load(Ordering::SeqCst), 1);

    // A batch-wide error marks every part, keeps the content and counts one error per part.
    let bad = reg
        .build_processor(&conf(json!({"label": "b", "bad": {}})), &p, "1")
        .unwrap();
    let out = bad.process_batch(&p, parts(&["a", "b"])).await.unwrap();
    assert_eq!(texts(&out[0]), vec!["a", "b"]);
    let err = out[0][0].error_get().unwrap();
    assert_eq!(
        (
            err.message.as_str(),
            err.label.as_str(),
            err.component_type.as_str()
        ),
        ("upstream 503", "b", "bad")
    );
    assert_eq!(err.path, "pipeline.processors.1");
    assert_eq!(p.metrics.get_counter("b.processor_error"), 2);

    let quota = reg
        .build_processor(&conf(json!({"quota": {}})), &p, "2")
        .unwrap();
    assert!(matches!(
        quota.process_batch(&p, parts(&["a"])).await,
        Err(CoreError::QuotaExceeded { .. })
    ));
    let closed = reg
        .build_processor(&conf(json!({"closed": {}})), &p, "3")
        .unwrap();
    assert!(matches!(
        closed.process_batch(&p, parts(&["a"])).await,
        Err(CoreError::ComponentClosed)
    ));

    // A batch over the org's memory quota is refused before the processor runs.
    p.org.set_quotas(OrgQuotas {
        max_memory_bytes_per_run: 3,
        ..OrgQuotas::default()
    });
    assert!(matches!(
        obs.process_batch(&p, parts(&["toolong"])).await,
        Err(CoreError::QuotaExceeded { .. })
    ));
}

#[tokio::test]
async fn input_with_processors_splits_drops_and_acks_once() {
    let reg = registry();
    let (_tx, s) = stream_ctx(reg.clone());
    let acks = Arc::new(Mutex::new(Vec::new()));
    let acks2 = acks.clone();
    let queue = Mutex::new(Some(VecDeque::from([
        parts(&["drop-me"]),
        parts(&["a", "b"]),
        parts(&["c"]),
    ])));
    reg.register_input("queue", move |_, _| {
        Ok(Box::new(QueueInput {
            batches: queue.lock().unwrap().take().unwrap_or_default(),
            acks: acks2.clone(),
        }) as Box<dyn Input>)
    });
    reg.register_processor("drop_first", |_, _| {
        struct DropFirst(AtomicUsize);
        #[async_trait]
        impl Processor for DropFirst {
            fn processor_type(&self) -> &'static str {
                "drop_first"
            }
            async fn process_batch(
                &self,
                _: &ProcContext,
                b: Batch,
            ) -> Result<Vec<Batch>, CoreError> {
                if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Ok(vec![]);
                }
                Ok(vec![b])
            }
        }
        Ok(Arc::new(DropFirst(AtomicUsize::new(0))) as Arc<dyn Processor>)
    });

    let mut input = reg
        .build_input(
            &conf(json!({"queue": {}, "processors": [{"drop_first": {}}, {"split_each": {}}, {"upper": {}}]})),
            &s,
        )
        .unwrap();
    input.connect(&s).await.unwrap();

    // The dropped batch is acked at once; the split one comes back part by part.
    let (b1, a1) = input.read_batch(&s).await.unwrap();
    assert_eq!(acks.lock().unwrap().len(), 1);
    let (b2, a2) = input.read_batch(&s).await.unwrap();
    assert_eq!(
        (texts(&b1), texts(&b2)),
        (vec!["A".into()], vec!["B".into()])
    );
    a1.ack();
    assert_eq!(
        acks.lock().unwrap().len(),
        1,
        "acked before every split part was"
    );
    a2.ack();
    assert_eq!(acks.lock().unwrap().len(), 2);

    let (b3, a3) = input.read_batch(&s).await.unwrap();
    assert_eq!(texts(&b3), vec!["C"]);
    a3.nack(BatchError::new(b3, "write failed"));
    assert_eq!(acks.lock().unwrap()[2].as_deref(), Some("write failed"));

    assert!(matches!(
        input.read_batch(&s).await,
        Err(CoreError::EndOfInput)
    ));
    input.close().await.unwrap();
}

fn batched(
    reg: &Arc<Registry>,
    s: &StreamContext,
    policy: serde_json::Value,
) -> (Arc<dyn Output>, Arc<Collect>) {
    let collected = Arc::new(Collect::default());
    let c2 = collected.clone();
    reg.register_output("shared", move |_, _| Ok(c2.clone() as Arc<dyn Output>));
    let out = reg
        .build_output(&conf(json!({"shared": {"batching": policy}})), s)
        .unwrap();
    (out, collected)
}

fn written(c: &Collect) -> Vec<Vec<String>> {
    c.0.lock().unwrap().iter().map(|b| texts(b)).collect()
}

#[tokio::test]
async fn an_output_batching_policy_gathers_writes_cuts_by_count_and_runs_its_processors() {
    let reg = registry();
    let (_tx, s) = stream_ctx(reg.clone());
    // The Portal's outcome post: `http_client.batching` with `count`, `period` and processors.
    let (out, collected) = batched(
        &reg,
        &s,
        json!({"count": 3, "period": "1h",
        "processors": [{"split_each": {}}, {"upper": {}}]}),
    );
    out.write_batch(&s, parts(&["a"])).await.unwrap();
    out.write_batch(&s, parts(&["b"])).await.unwrap();
    assert!(written(&collected).is_empty(), "two of three wait");
    out.write_batch(&s, parts(&["c", "d"])).await.unwrap();
    // Bento's policy Flush joins the processors' batches into one (`policy.go`).
    assert_eq!(written(&collected), vec![vec!["A", "B", "C"]]);
    // What still waits goes out when the output closes.
    out.close().await.unwrap();
    assert_eq!(written(&collected), vec![vec!["A", "B", "C"], vec!["D"]]);
}

#[tokio::test]
async fn an_output_batching_period_sends_what_waits() {
    let reg = registry();
    let (_tx, s) = stream_ctx(reg.clone());
    let (out, collected) = batched(&reg, &s, json!({"count": 100, "period": "50ms"}));
    out.write_batch(&s, parts(&["a"])).await.unwrap();
    out.write_batch(&s, parts(&["b"])).await.unwrap();
    assert!(written(&collected).is_empty());
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(written(&collected), vec![vec!["a", "b"]]);
    out.write_batch(&s, parts(&["c"])).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(written(&collected), vec![vec!["a", "b"], vec!["c"]]);
}

#[tokio::test]
async fn an_idle_batched_output_waits_for_its_next_period_tick() {
    // Bento's period ticks from the output's start, flushing empty batches too: a message that
    // comes two and a half periods after the start waits half a period for the third tick and
    // goes out with whatever joins it before then.
    let reg = registry();
    let (_tx, s) = stream_ctx(reg.clone());
    let (out, collected) = batched(&reg, &s, json!({"count": 100, "period": "200ms"}));
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    out.write_batch(&s, parts(&["a"])).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    out.write_batch(&s, parts(&["b"])).await.unwrap();
    assert!(written(&collected).is_empty(), "{:?}", written(&collected));
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(written(&collected), vec![vec!["a", "b"]]);
}

/// Refuses every write.
struct Refuses;
#[async_trait]
impl Output for Refuses {
    fn output_type(&self) -> &'static str {
        "refuses"
    }
    async fn write_batch(&self, _: &StreamContext, _: Batch) -> Result<(), CoreError> {
        Err(CoreError::Other("the target refused".into()))
    }
}

#[tokio::test]
async fn a_batched_write_that_fails_later_is_counted_and_answered_by_the_next_write() {
    let reg = registry();
    reg.register_output("refuses", |_, _| Ok(Arc::new(Refuses) as Arc<dyn Output>));
    let (_tx, s) = stream_ctx(reg.clone());
    let out = reg
        .build_output(
            &conf(json!({"refuses": {"batching": {"count": 100, "period": "300ms"}}})),
            &s,
        )
        .unwrap();
    out.write_batch(&s, parts(&["a"])).await.unwrap();
    // The timed flush fails; the next write comes inside the following period, so only the
    // remembered failure can answer it.
    let errors = || {
        s.metrics
            .output_error
            .load(std::sync::atomic::Ordering::Relaxed)
    };
    for _ in 0..500 {
        if errors() > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(errors(), 1);
    let err = out
        .write_batch(&s, parts(&["b"]))
        .await
        .expect_err("the failure surfaces");
    assert!(err.to_string().contains("the target refused"), "{err}");
}

#[tokio::test]
async fn output_with_processors_writes_each_processed_batch() {
    let reg = registry();
    let collected = Arc::new(Collect::default());
    let c2 = collected.clone();
    reg.register_output("shared", move |_, _| Ok(c2.clone() as Arc<dyn Output>));
    let (_tx, s) = stream_ctx(reg.clone());
    let out = reg
        .build_output(
            &conf(json!({"label": "o", "shared": {}, "processors": [{"split_each": {}}, {"upper": {}}]})),
            &s,
        )
        .unwrap();
    out.connect(&s).await.unwrap();
    out.write_batch(&s, parts(&["x", "y"])).await.unwrap();
    let got: Vec<Vec<String>> = collected
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|b| texts(b))
        .collect();
    assert_eq!(got, vec![vec!["X".to_string()], vec!["Y".to_string()]]);

    // Processors that drop everything write nothing.
    let none = reg
        .build_output(
            &conf(json!({"shared": {}, "processors": [{"drop_all": {}}]})),
            &s,
        )
        .unwrap();
    none.write_batch(&s, parts(&["z"])).await.unwrap();
    assert_eq!(collected.0.lock().unwrap().len(), 2);
    none.close().await.unwrap();
}

#[tokio::test]
async fn batcher_flushes_on_count_size_check_and_period() {
    let (_tx, p) = proc_ctx();

    let mut by_count = Batcher::new(
        &BatchPolicyConfig::from_json_value(&json!({"count": 2})).unwrap(),
        p.clone(),
    )
    .unwrap();
    assert!(by_count.until_next().is_none());
    assert!(by_count.flush().await.unwrap().is_none());
    assert!(!by_count.add(Part::from_str("a")));
    assert!(by_count.add(Part::from_str("b")));
    assert_eq!(by_count.count(), 2);
    assert_eq!(
        texts(&by_count.flush().await.unwrap().unwrap()),
        vec!["a", "b"]
    );
    assert_eq!(by_count.count(), 0);

    let mut by_size = Batcher::new(
        &BatchPolicyConfig::from_json_value(&json!({"byte_size": 4})).unwrap(),
        p.clone(),
    )
    .unwrap();
    assert!(!by_size.add(Part::from_str("ab")));
    assert!(by_size.add(Part::from_str("cd")));

    let mut by_check = Batcher::new(
        &BatchPolicyConfig::from_json_value(&json!({"check": r#"this.last == true"#})).unwrap(),
        p.clone(),
    )
    .unwrap();
    assert!(!by_check.add(Part::from_str(r#"{"last":false}"#)));
    assert!(!by_check.add(Part::from_str("not json")));
    assert!(by_check.add(Part::from_str(r#"{"last":true}"#)));

    let mut by_period = Batcher::new(
        &BatchPolicyConfig::from_json_value(&json!({"period": "20ms"})).unwrap(),
        p.clone(),
    )
    .unwrap();
    assert!(by_period.until_next().unwrap() <= Duration::from_millis(20));
    assert!(!by_period.add(Part::from_str("a")));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(by_period.add(Part::from_str("b")));
    let zero = Batcher::new(
        &BatchPolicyConfig::from_json_value(&json!({"period": "0s", "count": 5})).unwrap(),
        p.clone(),
    )
    .unwrap();
    assert!(zero.until_next().is_none());

    // Batch processors run on flush and see the whole batch; a drop-all flush yields nothing.
    let mut with_procs = Batcher::new(
        &BatchPolicyConfig::from_json_value(
            &json!({"count": 2, "processors": [{"split_each": {}}, {"upper": {}}]}),
        )
        .unwrap(),
        p.clone(),
    )
    .unwrap();
    with_procs.add(Part::from_str("a"));
    with_procs.add(Part::from_str("b"));
    assert_eq!(
        texts(&with_procs.flush().await.unwrap().unwrap()),
        vec!["A", "B"]
    );
    let mut dropping = Batcher::new(
        &BatchPolicyConfig::from_json_value(&json!({"count": 1, "processors": [{"drop_all": {}}]}))
            .unwrap(),
        p.clone(),
    )
    .unwrap();
    dropping.add(Part::from_str("a"));
    assert!(dropping.flush().await.unwrap().is_none());

    for bad in [
        json!({"period": "never"}),
        json!({"check": "this.("}),
        json!({"processors": [{"nope": {}}]}),
    ] {
        let c = BatchPolicyConfig::from_json_value(&bad).unwrap();
        assert!(Batcher::new(&c, p.clone()).is_err(), "{bad}");
    }
}

async fn run_pool(
    threads: isize,
    procs: Vec<Arc<dyn Processor>>,
    p: ProcContext,
    batches: Vec<Batch>,
) -> (
    Result<(), CoreError>,
    Vec<Transaction>,
    Vec<tokio::sync::oneshot::Receiver<Option<BatchError>>>,
) {
    let (in_tx, in_rx) = mpsc::channel(8);
    let (out_tx, mut out_rx) = mpsc::channel(8);
    let mut acks = Vec::new();
    for b in batches {
        let (ack, rx) = AckResponder::channel();
        acks.push(rx);
        in_tx.send(Transaction::new(b, ack)).await.unwrap();
    }
    drop(in_tx);
    let res = PipelineWorkerPool::new(threads, procs, p)
        .run(in_rx, out_tx)
        .await;
    let mut out = Vec::new();
    while let Some(t) = out_rx.recv().await {
        out.push(t);
    }
    (res, out, acks)
}

#[tokio::test]
async fn worker_pool_maps_a_split_batch_failure_back_to_the_source_part() {
    let (_tx, p) = proc_ctx();
    let closes = Arc::new(AtomicUsize::new(0));
    let procs: Vec<Arc<dyn Processor>> = vec![Arc::new(Split), Arc::new(Closing(closes.clone()))];
    let (res, outs, mut acks) = run_pool(1, procs, p.clone(), vec![parts(&["a", "b", "c"])]).await;
    res.unwrap();
    assert_eq!(closes.load(Ordering::SeqCst), 1);
    assert_eq!(outs.len(), 3);
    let mut outs = outs.into_iter();
    outs.next().unwrap().ack.ack();
    let t = outs.next().unwrap();
    let failed = t.payload.clone();
    t.ack.nack(BatchError::new(failed, "write b failed"));
    assert!(
        acks[0].try_recv().is_err(),
        "resolved before every part answered"
    );
    outs.next().unwrap().ack.ack();
    let err = acks[0].try_recv().unwrap().expect("a nack");
    assert_eq!(err.indexed_errors(), 1);
    assert_eq!(err.to_string(), "write b failed");
}

#[tokio::test]
async fn worker_pool_with_threads_acks_drops_and_forwards() {
    let (_tx, p) = proc_ctx();
    let procs: Vec<Arc<dyn Processor>> = vec![Arc::new(Upper)];
    let (res, outs, _acks) = run_pool(
        3,
        procs,
        p.clone(),
        vec![parts(&["a"]), parts(&["b"]), parts(&["c"])],
    )
    .await;
    res.unwrap();
    let mut got: Vec<String> = outs.iter().flat_map(|t| texts(&t.payload)).collect();
    got.sort();
    assert_eq!(got, vec!["A", "B", "C"]);

    // A processor dropping everything acks the source transaction.
    let (res, outs, mut acks) =
        run_pool(2, vec![Arc::new(Dropper)], p.clone(), vec![parts(&["a"])]).await;
    res.unwrap();
    assert!(outs.is_empty());
    assert!(acks[0].try_recv().unwrap().is_none());

    // No processors: transactions pass through untouched; zero threads means one per core.
    let (res, outs, _) = run_pool(0, vec![], p.clone(), vec![parts(&["x"])]).await;
    res.unwrap();
    assert_eq!(texts(&outs[0].payload), vec!["x"]);
}

#[tokio::test]
async fn worker_pool_nacks_and_stops_on_a_fatal_processor_error() {
    let (_tx, p) = proc_ctx();
    for threads in [1, 2] {
        let procs: Vec<Arc<dyn Processor>> = vec![Arc::new(Fail(|| CoreError::ComponentClosed))];
        let (res, outs, mut acks) = run_pool(threads, procs, p.clone(), vec![parts(&["a"])]).await;
        assert!(
            matches!(res, Err(CoreError::ComponentClosed)),
            "threads {threads}"
        );
        assert!(outs.is_empty());
        let err = acks[0].try_recv().unwrap().expect("a nack");
        assert_eq!(err.to_string(), "component closed");
    }
}

#[tokio::test]
async fn processors_stop_at_shutdown() {
    let (tx, p) = proc_ctx();
    tx.send(true).unwrap();
    let procs: Vec<Arc<dyn Processor>> = vec![Arc::new(Upper)];
    for res in [
        ekiben_core::execute_all(&p, &procs, vec![parts(&["a"])]).await,
        ekiben_core::execute_try_all(&p, &procs, vec![parts(&["a"])]).await,
        ekiben_core::execute_catch_all(&p, &procs, vec![parts(&["a"])]).await,
    ] {
        assert!(matches!(res, Err(CoreError::ComponentClosed)));
    }
}

// ---------- transactions ----------

#[test]
fn transactions_and_sync_responses() {
    AckResponder::noop().ack();
    AckResponder::noop().nack(BatchError::new(vec![], "x"));
    let (ack, mut rx) = AckResponder::channel();
    let t = TrackedTransaction::new(parts(&["m"]), ack);
    assert_eq!(texts(t.message()), vec!["m"]);
    let (msg, group, ack) = t.into_parts();
    assert_eq!(group.get_index(&msg[0]), Some(0));
    ack.respond(None);
    assert!(rx.try_recv().unwrap().is_none());

    let store = SyncResponseStore::new();
    let mut p = Part::from_str("req");
    p.with_sync_response_store(store.clone());
    store.add(&parts(&["resp1", "resp2"]));
    let read = p.sync_response_store().unwrap().read();
    assert_eq!(texts(&read[0]), vec!["resp1", "resp2"]);
    store.clear();
    assert!(store.read().is_empty());
}

// ---------- messages ----------

#[test]
fn part_copies_metadata_and_errors() {
    let mut p = Part::from_json(json!({"a": [1, 2]}));
    p.meta_set_str("k", "v");
    p.meta_set_str("z", "w");
    let deep = p.deep_copy();
    p.meta_delete("k");
    assert_eq!(deep.meta_get_str("k"), "v");
    assert_eq!(p.meta_get_str("k"), "");
    p.meta_clear();
    assert!(p.meta_all().is_empty());
    let raw_deep = Part::from_str("raw").deep_copy();
    assert_eq!(texts(&[raw_deep]), vec!["raw"]);

    assert!(Part::new(Bytes::new()).is_empty());
    assert!(!Part::from_str("x").is_empty());
    assert!(!Part::from_json(json!(null)).is_empty());

    let mut e = Part::from_str("x");
    e.set_error_msg("boom");
    assert_eq!(e.error_get().unwrap().to_string(), "boom");

    let (_g, tagged) = SortGroup::new(parts(&["a"]));
    let top = SortGroup::top_level_group(&tagged[0]).unwrap();
    // The top group recovers the original index of a tagged part.
    assert_eq!(top.get_index(&tagged[0]), Some(0));
    assert!(SortGroup::top_level_group(&Part::from_str("untagged")).is_none());

    let s = Part::from_json(json!({"arr": [1, "two", [true]]}));
    assert!(s.size_estimate() > 10);
}

#[test]
fn parts_view_reports_parse_errors_and_the_meta_adapter_edits_the_part() {
    let batch = vec![
        Part::from_str("{not json"),
        Part::from_str("{\"a\":1} {\"b\":2}"),
    ];
    let view = PartsView(&batch);
    let e = view.part_structured(0).unwrap_err();
    assert!(e.starts_with("parse as json"), "{e}");
    let e = view.part_structured(1).unwrap_err();
    assert!(e.contains("multiple valid documents"), "{e}");
    assert_eq!(view.part_structured(9).unwrap_err(), "message is empty");

    let mut p = Part::from_str("x");
    let mut m = PartMetaAdapter(&mut p);
    m.meta_set("a".into(), Value::String("1".into()));
    m.meta_set("b".into(), Value::String("2".into()));
    m.meta_delete("a");
    assert_eq!(m.meta_all().len(), 1);
    m.meta_clear();
    assert!(m.meta_all().is_empty());
}

// ---------- metrics ----------

#[test]
fn metrics_snapshot_and_prometheus_text() {
    let m = StreamMetrics::new("s\"1", "org");
    m.incr_processor_received("map", 3);
    m.incr_processor_sent("map", 2, 1);
    m.incr_processor_error("map", 1);
    m.incr_processor_received("", 1);
    m.incr_counter("custom", 4);
    for ns in [10, 20, 30, 40] {
        m.record_processor_latency("map", ns);
    }
    m.record_processor_latency("", 5);
    m.output_sent.fetch_add(2, Ordering::Relaxed);

    assert_eq!(m.get_counter("map.processor_received"), 3);
    assert_eq!(m.get_counter("output_sent"), 2);
    assert_eq!(m.get_counter("input_received"), 0);
    assert_eq!(m.get_counter("output_batch_sent"), 0);
    assert_eq!(m.get_counter("output_error"), 0);
    assert_eq!(m.get_counter("never"), 0);

    let snap = m.snapshot_json(99);
    assert_eq!(snap["uptime_ns"], 99);
    assert_eq!(snap["custom"], 4);
    assert_eq!(snap["map.processor_latency_ns"]["p50"], 30.0);

    // E2: `/metrics` carries Bento's names and labels (no `ekiben_*`), fed by the components'
    // namespaces; the stream's local counters above stay for the stats API only.
    let reg = MetricsRegistry::new();
    reg.register_stream(m.clone());
    let ns = m
        .component("map", "root.pipeline.processors.0")
        .expect("a registered stream is attached to the exporter");
    ns.counter("processor_received").incr(3);
    ns.timer("processor_latency_ns").timing(40);
    let text = reg.render_prometheus();
    assert!(
        text.contains("# TYPE processor_latency_ns summary"),
        "{text}"
    );
    assert!(
        text.contains(
            r#"processor_received{label="map",org="org",path="root.pipeline.processors.0",stream="s\"1"} 3"#
        ),
        "{text}"
    );
    assert!(text.contains(r#"quantile="0.99"} 40"#), "{text}");
    assert!(!text.contains("ekiben_"), "{text}");
    assert!(!text.contains("custom"));
    reg.stream_deleted("org", "s\"1");
    let after = reg.render_prometheus();
    assert!(
        after.contains("processor_received{"),
        "a deleted stream's counters stay: {after}"
    );
    assert!(
        !after.contains("processor_latency_ns"),
        "a deleted stream's timings go: {after}"
    );
}

// ---------- redaction and errors ----------

#[test]
fn secrets_never_print() {
    let s = SecretString::new("pw-t6652");
    assert_eq!(s.expose(), "pw-t6652");
    assert!(!s.is_empty() && SecretString::default().is_empty());
    assert_eq!(format!("{s}"), "REDACTED");
    assert_eq!(format!("{s:?}"), "\"REDACTED\"");

    let v =
        redact_config_value(&json!({"list": [{"password": "pw-t6652"}, "https://u:pw-t6652@h/"]}));
    let text = v.to_string();
    assert!(!text.contains("pw-t6652"), "{text}");
}

#[test]
fn errors_format_their_message() {
    let d = DetailedError::new("bad", "mapping", "m", "pipeline.processors.0");
    assert_eq!(d.to_string(), "bad");
    let b = BatchError::new(parts(&["a"]), "batch bad");
    assert_eq!(b.to_string(), "batch bad");
    assert_eq!(CoreError::other("x").to_string(), "x");

    let out = interpolate_env_bytes(b"a ${V:dflt} b", |_| None).unwrap();
    assert_eq!(out, b"a dflt b");
}

#[test]
fn collapsed_counts_errors_and_the_batch_view() {
    let mut p = Part::default();
    assert!(p.is_empty());
    assert_eq!(p.collapsed_count(), 1);
    p.add_collapsed_count(3);
    assert_eq!(
        ekiben_core::message::batch_collapsed_count(&[p.clone(), Part::from_str("x")]),
        4
    );

    let mut s = Part::from_str(r#"{"a":1}"#);
    assert_eq!(s.as_structured().unwrap().to_json_value(), json!({"a": 1}));
    s.set_error_msg("boom");
    let batch = vec![s.clone()];
    let view = PartsView(&batch);
    assert_eq!(view.part_bytes(0), br#"{"a":1}"#);
    assert_eq!(view.part_meta_get(0, "none"), None);
    assert_eq!(view.part_error(0).unwrap().message, "boom");
    assert!(view.part_error(5).is_none());
    s.clear_error();
    assert!(!s.is_errored());

    let est = Part::from_structured(Value::from_json_value(json!([1, "two", {"k": [true]}])));
    assert!(est.size_estimate() > 10);

    let (_tx, p) = proc_ctx();
    assert_eq!(p.org_id().as_str(), "o1");

    // The host-environment lookup of from_yaml_or_json falls back to the default.
    let c = StreamConfig::from_yaml_or_json(
        "input:\n  generate: {mapping: \"${T6652_NEVER_SET:root = 1}\"}\n",
    )
    .unwrap();
    assert_eq!(c.input.config["mapping"], "root = 1");
}
