//! The composite and utility inputs and outputs (broker, batched, sequence, read_until, resource,
//! stdin, inproc; drop, stdout, reject, reject_errored, drop_on, retry, fallback, switch, broker,
//! resource, cache, sync_response) and the file input's and output's failure paths, built
//! through the registry as a stream builds them. Children are scripted doubles registered as
//! `scripted` inputs and `rec` outputs, so each test reads what a child got and answers.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use ekiben_bloblang::Environment;
use ekiben_core::{
    AckResponder, Batch, BatchError, Cache, ComponentConfig, CoreError, Input, OrgId, Output, Part,
    Registry, ResourceManager, StreamContext, StreamMetrics, SyncResponseStore,
};
use ekiben_io_file::inputs::Scanner;
use serde_json::{json, Value as JsonValue};
use tempfile::TempDir;
use tokio::sync::watch;

// ---------------------------------------------------------------------------------------------
// Doubles

/// What a scripted input answers per read: a batch of lines or an error.
type Step = Result<Vec<&'static str>, &'static str>;

#[derive(Default)]
struct Probe {
    acks: Mutex<Vec<Option<String>>>,
    connects: AtomicUsize,
    closes: AtomicUsize,
}

struct Scripted {
    steps: VecDeque<Step>,
    probe: Arc<Probe>,
    connected: bool,
}

#[async_trait]
impl Input for Scripted {
    fn input_type(&self) -> &'static str {
        "scripted"
    }

    async fn connect(&mut self, _ctx: &StreamContext) -> Result<(), CoreError> {
        self.probe.connects.fetch_add(1, Ordering::SeqCst);
        self.connected = true;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    async fn read_batch(
        &mut self,
        _ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        match self.steps.pop_front() {
            None => Err(CoreError::EndOfInput),
            Some(Err(e)) => Err(CoreError::other(e)),
            Some(Ok(lines)) => {
                let probe = self.probe.clone();
                let batch = lines.into_iter().map(Part::from_str).collect();
                let ack = AckResponder::from_fn(move |res| {
                    probe
                        .acks
                        .lock()
                        .expect("lock")
                        .push(res.map(|e| e.message));
                });
                Ok((batch, ack))
            }
        }
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        self.probe.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// How a recording output answers one write.
enum Fail {
    Other(&'static str),
    /// A batch error naming these indexes of the batch.
    Indexed(&'static str, Vec<usize>),
    /// A batch error over the batch sent twice, naming each copy of its first part.
    Doubled(&'static str),
}

#[derive(Default)]
struct Rec {
    /// Each write: per part its body and its `fallback_error`.
    writes: Mutex<Vec<Vec<(String, String)>>>,
    fails: Mutex<VecDeque<Fail>>,
    closes: AtomicUsize,
    close_fails: bool,
}

impl Rec {
    fn bodies(&self) -> Vec<Vec<String>> {
        let writes = self.writes.lock().expect("lock");
        writes
            .iter()
            .map(|w| w.iter().map(|(b, _)| b.clone()).collect())
            .collect()
    }

    fn marks(&self) -> Vec<Vec<String>> {
        let writes = self.writes.lock().expect("lock");
        writes
            .iter()
            .map(|w| w.iter().map(|(_, e)| e.clone()).collect())
            .collect()
    }
}

struct RecOutput(Arc<Rec>);

#[async_trait]
impl Output for RecOutput {
    fn output_type(&self) -> &'static str {
        "rec"
    }

    async fn close(&self) -> Result<(), CoreError> {
        self.0.closes.fetch_add(1, Ordering::SeqCst);
        if self.0.close_fails {
            return Err(CoreError::other("close failed"));
        }
        Ok(())
    }

    async fn write_batch(&self, _ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        self.0.writes.lock().expect("lock").push(
            batch
                .iter()
                .map(|p| {
                    (
                        String::from_utf8_lossy(&p.as_bytes_ref()).into_owned(),
                        p.meta_get_str("fallback_error"),
                    )
                })
                .collect(),
        );
        let fail = self.0.fails.lock().expect("lock").pop_front();
        match fail {
            None => Ok(()),
            Some(Fail::Other(m)) => Err(CoreError::other(m)),
            Some(Fail::Indexed(m, idx)) => {
                let mut e = BatchError::new(batch, m.to_string());
                for i in idx {
                    e.failed(i, format!("part {i} failed"));
                }
                Err(CoreError::Batch(e))
            }
            Some(Fail::Doubled(m)) => {
                let n = batch.len();
                let twice = batch.iter().chain(&batch).cloned().collect();
                let mut e = BatchError::new(twice, m.to_string());
                e.failed(0, "first copy");
                e.failed(n, "second copy");
                Err(CoreError::Batch(e))
            }
        }
    }
}

#[derive(Default)]
struct MemCache(Mutex<HashMap<String, Bytes>>);

#[async_trait]
impl Cache for MemCache {
    async fn get(&self, key: &str) -> Result<Bytes, CoreError> {
        let map = self.0.lock().expect("lock");
        map.get(key)
            .cloned()
            .ok_or_else(|| CoreError::KeyNotFound(key.to_string()))
    }
    async fn set(&self, key: &str, value: Bytes, _: Option<Duration>) -> Result<(), CoreError> {
        self.0.lock().expect("lock").insert(key.to_string(), value);
        Ok(())
    }
    async fn add(&self, key: &str, value: Bytes, ttl: Option<Duration>) -> Result<(), CoreError> {
        self.set(key, value, ttl).await
    }
    async fn delete(&self, key: &str) -> Result<(), CoreError> {
        self.0.lock().expect("lock").remove(key);
        Ok(())
    }
}

fn scripts() -> &'static Mutex<HashMap<String, Scripted>> {
    static S: OnceLock<Mutex<HashMap<String, Scripted>>> = OnceLock::new();
    S.get_or_init(Mutex::default)
}

fn recs() -> &'static Mutex<HashMap<String, Arc<Rec>>> {
    static R: OnceLock<Mutex<HashMap<String, Arc<Rec>>>> = OnceLock::new();
    R.get_or_init(Mutex::default)
}

/// A scripted input the registry builds from `{"scripted": "<name>"}`; names are per test.
fn script(name: &str, steps: Vec<Step>) -> Arc<Probe> {
    let probe = Arc::new(Probe::default());
    scripts().lock().expect("lock").insert(
        name.to_string(),
        Scripted {
            steps: steps.into(),
            probe: probe.clone(),
            connected: false,
        },
    );
    probe
}

/// A recording output the registry builds from `{"rec": "<name>"}`.
fn rec(name: &str, fails: Vec<Fail>, close_fails: bool) -> Arc<Rec> {
    let r = Arc::new(Rec {
        fails: Mutex::new(fails.into()),
        close_fails,
        ..Rec::default()
    });
    recs()
        .lock()
        .expect("lock")
        .insert(name.to_string(), r.clone());
    r
}

fn registry() -> Arc<Registry> {
    let reg = Arc::new(Registry::new());
    ekiben_processors::register_all(&reg);
    ekiben_io_file::register_all(&reg);
    reg.register_input("scripted", |conf, _ctx| {
        let name = conf.config.as_str().unwrap_or_default();
        let s = scripts().lock().expect("lock").remove(name);
        s.map(|s| Box::new(s) as Box<dyn Input>)
            .ok_or_else(|| CoreError::config(format!("no script {name}")))
    });
    reg.register_output("rec", |conf, _ctx| {
        let name = conf.config.as_str().unwrap_or_default();
        let r = recs().lock().expect("lock").get(name).cloned();
        r.map(|r| Arc::new(RecOutput(r)) as Arc<dyn Output>)
            .ok_or_else(|| CoreError::config(format!("no recorder {name}")))
    });
    reg
}

fn ctx_with(
    file_root: Option<std::path::PathBuf>,
    shutdown: watch::Receiver<bool>,
) -> StreamContext {
    let resources = ResourceManager::new();
    let org = resources.get_or_create_org(&OrgId::new("org-components"));
    StreamContext {
        stream_id: "components".to_string(),
        org,
        resources,
        registry: registry(),
        blobl_env: Environment::new(),
        metrics: StreamMetrics::new("components", "org-components"),
        label: "root".to_string(),
        path: "root".to_string(),
        file_root,
        env_vars: Arc::new(HashMap::new()),
        shutdown_rx: shutdown,
    }
}

fn ctx() -> StreamContext {
    ctx_with(None, watch::channel(false).1)
}

fn input(c: &StreamContext, conf: JsonValue) -> Result<Box<dyn Input>, CoreError> {
    let conf = ComponentConfig::from_json_value(&conf, "stdin", None)?;
    c.registry.build_input(&conf, c)
}

fn output(c: &StreamContext, conf: JsonValue) -> Result<Arc<dyn Output>, CoreError> {
    let conf = ComponentConfig::from_json_value(&conf, "drop", None)?;
    c.registry.build_output(&conf, c)
}

fn refusal<T>(r: Result<T, CoreError>) -> String {
    match r {
        Ok(_) => panic!("accepted"),
        Err(e) => e.to_string(),
    }
}

fn texts(b: &Batch) -> Vec<String> {
    b.iter()
        .map(|p| String::from_utf8_lossy(&p.as_bytes_ref()).into_owned())
        .collect()
}

fn parts(lines: &[&str]) -> Batch {
    lines.iter().map(|l| Part::from_str(l)).collect()
}

/// Every batch until the end of input, acking each.
async fn drain(i: &mut Box<dyn Input>, c: &StreamContext) -> Result<Vec<Vec<String>>, String> {
    let mut out = Vec::new();
    loop {
        match i.read_batch(c).await {
            Ok((b, ack)) => {
                out.push(texts(&b));
                ack.ack();
            }
            Err(CoreError::EndOfInput) => return Ok(out),
            Err(e) => return Err(e.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Inputs

#[tokio::test]
async fn every_input_refuses_a_config_it_cannot_build_and_names_the_field() {
    let c = ctx();
    let cases = [
        (
            json!({"generate": "x"}),
            "generate input config must be an object",
        ),
        (
            json!({"generate": {}}),
            "generate input requires a 'mapping' field",
        ),
        (
            json!({"generate": {"mapping": "root = 1", "interval": 5}}),
            "'interval' must be a string",
        ),
        (
            json!({"generate": {"mapping": "root = 1", "interval": "soon"}}),
            "or as cron expression",
        ),
        (json!({"file": "x"}), "file input config must be an object"),
        (json!({"file": {}}), "file input requires a 'paths' field"),
        (
            json!({"file": {"paths": ["/tmp/[x"]}}),
            "invalid glob pattern '/tmp/[x'",
        ),
        (json!({"file": {"paths": ["../x"]}}), "path traversal"),
        (json!({"csv": "x"}), "csv input config must be an object"),
        (
            json!({"csv": {"paths": "a.csv"}}),
            "csv input requires a 'paths' field",
        ),
        (
            json!({"broker": "x"}),
            "broker input config must be an object",
        ),
        (json!({"broker": {"inputs": []}}), "with no inputs"),
        (
            json!({"broker": {"inputs": [{"scripted": "missing"}]}}),
            "no script missing",
        ),
        (
            json!({"batched": "x"}),
            "batched input config must be an object",
        ),
        (
            json!({"batched": {}}),
            "batched input requires a child field",
        ),
        (
            json!({"batched": {"child": {"inproc": ""}}}),
            "batched input requires a policy field",
        ),
        (
            json!({"sequence": {}}),
            "sequence input requires an 'inputs' array",
        ),
        (
            json!({"sequence": {"inputs": [{"scripted": "missing"}]}}),
            "no script missing",
        ),
        (
            json!({"read_until": {}}),
            "read_until requires an 'input' field",
        ),
        (
            json!({"read_until": {"input": {"inproc": ""}}}),
            "read_until requires a 'check' field",
        ),
        (
            json!({"resource": ""}),
            "resource input requires a resource name",
        ),
    ];
    for (conf, want) in cases {
        let e = refusal(input(&c, conf.clone()));
        assert!(e.contains(want), "{conf}: {e}");
    }
    let confined = ctx_with(Some(std::env::temp_dir()), watch::channel(false).1);
    let e = refusal(input(&confined, json!({"stdin": {}})));
    assert!(e.contains("confined to a file root"), "{e}");
}

#[tokio::test]
async fn input_types_are_named_as_bento_names_them() {
    let c = ctx();
    script("names-a", vec![]);
    script("names-b", vec![]);
    script("names-c", vec![]);
    let cases = [
        (json!({"generate": {"mapping": "root = 1"}}), "generate"),
        (json!({"file": {"paths": []}}), "file"),
        (json!({"csv": {"paths": []}}), "file"),
        (
            json!({"broker": {"inputs": [{"scripted": "names-a"}]}}),
            "broker",
        ),
        (
            json!({"batched": {"child": {"inproc": ""}, "policy": {"count": 2}}}),
            "batched",
        ),
        (
            json!({"sequence": {"inputs": [{"scripted": "names-b"}]}}),
            "sequence",
        ),
        (
            json!({"read_until": {"input": {"scripted": "names-c"}, "check": "true"}}),
            "read_until",
        ),
        (json!({"resource": "r"}), "resource"),
        (json!({"stdin": {}}), "stdin"),
        (json!({"inproc": ""}), "inproc"),
    ];
    for (conf, want) in cases {
        assert_eq!(input(&c, conf).expect("builds").input_type(), want);
    }
}

#[tokio::test]
async fn an_inproc_input_ends_at_once() {
    let c = ctx();
    let mut i = input(&c, json!({"inproc": ""})).expect("builds");
    assert!(matches!(i.read_batch(&c).await, Err(CoreError::EndOfInput)));
}

#[tokio::test]
async fn a_broker_reads_its_inputs_in_turn_with_copies_and_ends_when_all_end() {
    let c = ctx();
    let a = script("broker-a", vec![Ok(vec!["a1"]), Ok(vec!["a2"])]);
    let b = script("broker-b", vec![Ok(vec!["b1"])]);
    let mut i = input(
        &c,
        json!({"broker": {"inputs": [{"scripted": "broker-a"}, {"scripted": "broker-b"}]}}),
    )
    .expect("builds");
    assert!(!i.is_connected());
    i.connect(&c).await.expect("connects");
    assert!(i.is_connected());
    assert_eq!(a.connects.load(Ordering::SeqCst), 1);
    let got = drain(&mut i, &c).await.expect("drains");
    assert_eq!(got, [vec!["a1"], vec!["b1"], vec!["a2"]]);
    // each finished input is closed once, and the broker's close reaches nothing left
    assert_eq!(a.closes.load(Ordering::SeqCst), 1);
    assert_eq!(b.closes.load(Ordering::SeqCst), 1);
    i.close().await.expect("closes");
    assert_eq!(a.acks.lock().expect("lock").len(), 2);
}

#[tokio::test]
async fn a_broker_close_closes_every_open_input_and_copies_build_each_input_again() {
    let c = ctx();
    script("copies-a", vec![Ok(vec!["x"])]);
    let copies = input(
        &c,
        json!({"broker": {"copies": 2, "inputs": [{"scripted": "copies-a"}]}}),
    );
    // a copy builds its input again: the scripted double exists once, so the second copy fails
    assert!(refusal(copies).contains("no script copies-a"));
    let p = script("close-a", vec![Ok(vec!["x"])]);
    let mut i =
        input(&c, json!({"broker": {"inputs": [{"scripted": "close-a"}]}})).expect("builds");
    i.close().await.expect("closes");
    assert_eq!(p.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_broker_batches_across_inputs_and_flushes_the_rest_when_they_end() {
    let c = ctx();
    let a = script("bb-a", vec![Ok(vec!["a1", "a2"]), Ok(vec!["a3"])]);
    let b = script("bb-b", vec![Ok(vec!["b1"])]);
    let mut i = input(
        &c,
        json!({"broker": {
            "inputs": [{"scripted": "bb-a"}, {"scripted": "bb-b"}],
            "batching": {"count": 3},
        }}),
    )
    .expect("builds");
    let (first, ack) = i.read_batch(&c).await.expect("full batch");
    assert_eq!(texts(&first), ["a1", "a2", "b1"]);
    ack.nack(BatchError::new(first, "output down".to_string()));
    // the nack reaches the acks of both inputs' batches
    assert_eq!(
        *a.acks.lock().expect("lock"),
        [Some("output down".to_string())]
    );
    assert_eq!(
        *b.acks.lock().expect("lock"),
        [Some("output down".to_string())]
    );
    let (rest, ack) = i.read_batch(&c).await.expect("flushed at the end");
    assert_eq!(texts(&rest), ["a3"]);
    ack.ack();
    assert_eq!(a.acks.lock().expect("lock").last(), Some(&None));
    assert!(matches!(i.read_batch(&c).await, Err(CoreError::EndOfInput)));
}

#[tokio::test]
async fn a_noop_batching_policy_passes_batches_through_and_errors_stop_the_broker() {
    let c = ctx();
    script("noop-a", vec![Ok(vec!["a1", "a2"]), Err("source down")]);
    let mut i = input(
        &c,
        json!({"broker": {"inputs": [{"scripted": "noop-a"}], "batching": {"count": 1}}}),
    )
    .expect("builds");
    let (b, _ack) = i.read_batch(&c).await.expect("batch");
    assert_eq!(texts(&b), ["a1", "a2"]);
    let e = refusal(i.read_batch(&c).await);
    assert!(e.contains("source down"), "{e}");
    // a batching broker whose inputs end with nothing pending ends too
    script("empty-a", vec![]);
    let mut i = input(
        &c,
        json!({"broker": {"inputs": [{"scripted": "empty-a"}], "batching": {"count": 5}}}),
    )
    .expect("builds");
    assert!(matches!(i.read_batch(&c).await, Err(CoreError::EndOfInput)));
}

#[tokio::test]
async fn a_batched_input_groups_its_child_and_answers_every_child_ack() {
    let c = ctx();
    let p = script(
        "batched-a",
        vec![
            Ok(vec!["1"]),
            Ok(vec!["2", "3"]),
            Ok(vec!["4"]),
            Err("child down"),
        ],
    );
    let mut i = input(
        &c,
        json!({"batched": {"child": {"scripted": "batched-a"}, "policy": {"count": 2}}}),
    )
    .expect("builds");
    i.connect(&c).await.expect("connects");
    assert_eq!(p.connects.load(Ordering::SeqCst), 1);
    let (b, ack) = i.read_batch(&c).await.expect("batch");
    assert_eq!(texts(&b), ["1", "2", "3"]);
    ack.nack(BatchError::new(b, "write failed".to_string()));
    assert_eq!(
        *p.acks.lock().expect("lock"),
        [
            Some("write failed".to_string()),
            Some("write failed".to_string())
        ]
    );
    let e = refusal(i.read_batch(&c).await);
    assert!(e.contains("child down"), "{e}");
    // the pending "4" is flushed when the child ends
    let (b, ack) = i.read_batch(&c).await.expect("flushed");
    assert_eq!(texts(&b), ["4"]);
    ack.ack();
    assert!(matches!(i.read_batch(&c).await, Err(CoreError::EndOfInput)));
    i.close().await.expect("closes");
    assert_eq!(p.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_sequence_reads_one_input_after_the_other_and_keeps_its_place_after_an_error() {
    let c = ctx();
    let a = script("seq-a", vec![Ok(vec!["a1"]), Err("hiccup"), Ok(vec!["a2"])]);
    let b = script("seq-b", vec![Ok(vec!["b1"])]);
    let mut i = input(
        &c,
        json!({"sequence": {"inputs": [{"scripted": "seq-a"}, {"scripted": "seq-b"}]}}),
    )
    .expect("builds");
    i.connect(&c).await.expect("connects");
    assert_eq!(a.connects.load(Ordering::SeqCst), 1);
    assert_eq!(b.connects.load(Ordering::SeqCst), 0);
    assert_eq!(texts(&i.read_batch(&c).await.expect("a1").0), ["a1"]);
    assert!(refusal(i.read_batch(&c).await).contains("hiccup"));
    assert_eq!(
        drain(&mut i, &c).await.expect("rest"),
        [vec!["a2"], vec!["b1"]]
    );
    // the first input is closed and the second connected as it takes over
    assert_eq!(a.closes.load(Ordering::SeqCst), 1);
    assert_eq!(b.connects.load(Ordering::SeqCst), 1);
    let mut empty = input(&c, json!({"sequence": {"inputs": []}})).expect("builds");
    empty.connect(&c).await.expect("nothing to connect");
    assert!(matches!(
        empty.read_batch(&c).await,
        Err(CoreError::EndOfInput)
    ));
}

#[tokio::test]
async fn read_until_stops_after_the_batch_that_passes_the_check() {
    let c = ctx();
    let p = script(
        "until-a",
        vec![
            Ok(vec!["go", "go"]),
            Ok(vec!["go", "stop"]),
            Ok(vec!["never"]),
        ],
    );
    let mut i = input(
        &c,
        json!({"read_until": {"input": {"scripted": "until-a"}, "check": "content() == \"stop\""}}),
    )
    .expect("builds");
    i.connect(&c).await.expect("connects");
    assert_eq!(p.connects.load(Ordering::SeqCst), 1);
    let got = drain(&mut i, &c).await.expect("drains");
    assert_eq!(got, [vec!["go", "go"], vec!["go", "stop"]]);
    script("until-b", vec![Err("child down")]);
    let mut i = input(
        &c,
        json!({"read_until": {"input": {"scripted": "until-b"}, "check": "true"}}),
    )
    .expect("builds");
    assert!(refusal(i.read_batch(&c).await).contains("child down"));
}

#[tokio::test]
async fn a_resource_input_reads_the_named_resource_and_refuses_a_missing_one() {
    let c = ctx();
    let probe = Arc::new(Probe::default());
    c.resources.insert_input(
        c.org_id(),
        "shared",
        Box::new(Scripted {
            steps: vec![Ok(vec!["r1"])].into(),
            probe,
            connected: true,
        }),
    );
    let mut i = input(&c, json!({"resource": "shared"})).expect("builds");
    assert_eq!(texts(&i.read_batch(&c).await.expect("r1").0), ["r1"]);
    assert!(matches!(i.read_batch(&c).await, Err(CoreError::EndOfInput)));
    let mut missing = input(&c, json!({"resource": "absent"})).expect("builds");
    assert!(refusal(missing.read_batch(&c).await).contains("absent"));
}

#[tokio::test]
async fn generate_refuses_a_cron_schedule_that_never_fires_and_stops_on_shutdown() {
    let c = ctx();
    // 30 February never comes
    let mut i = input(
        &c,
        json!({"generate": {"mapping": "root = 1", "interval": "0 0 30 2 *"}}),
    )
    .expect("builds");
    assert!(refusal(i.read_batch(&c).await).contains("never fires again"));

    let (tx, rx) = watch::channel(false);
    let c = ctx_with(None, rx);
    let mut i = input(
        &c,
        json!({"generate": {"mapping": "root = 1", "interval": "1h"}}),
    )
    .expect("builds");
    let _ = i.read_batch(&c).await.expect("the first tick is free");
    // a change that is not a stop leaves the wait running
    tx.send(false).expect("no stop");
    tokio::time::sleep(Duration::from_millis(50)).await;
    tx.send(true).expect("signal");
    let r = tokio::time::timeout(Duration::from_secs(5), i.read_batch(&c))
        .await
        .expect("ends at once");
    assert!(matches!(r, Err(CoreError::EndOfInput)));
}

#[tokio::test]
async fn generate_keeps_ticking_when_the_shutdown_sender_is_gone() {
    // a dropped sender is no stop signal: the tick still comes
    let c = ctx_with(None, watch::channel(false).1);
    let mut i = input(
        &c,
        json!({"generate": {"mapping": "root = \"x\"", "interval": "20ms", "count": 2}}),
    )
    .expect("builds");
    let got = drain(&mut i, &c).await.expect("drains");
    assert_eq!(got, [vec!["x"], vec!["x"]]);
}

#[tokio::test]
async fn generate_skips_a_deleted_message_and_errors_on_a_failing_mapping() {
    let c = ctx();
    let mut i = input(
        &c,
        json!({"generate": {"mapping": "root = deleted()", "interval": "", "count": 1}}),
    )
    .expect("builds");
    assert_eq!(
        drain(&mut i, &c).await.expect("drains"),
        [Vec::<String>::new()]
    );
    let mut i = input(
        &c,
        json!({"generate": {"mapping": "root = throw(\"bad mapping\")", "interval": ""}}),
    )
    .expect("builds");
    assert!(refusal(i.read_batch(&c).await).contains("bad mapping"));
}

#[tokio::test]
async fn a_file_input_names_a_file_it_cannot_open_or_read() {
    let tmp = TempDir::new().expect("tmp");
    let c = ctx();
    let missing = tmp.path().join("missing.txt");
    let mut i = input(&c, json!({"file": {"paths": missing.to_str()}})).expect("builds");
    let e = refusal(i.read_batch(&c).await);
    assert!(
        e.contains("failed to read") && e.contains("missing.txt"),
        "{e}"
    );
    // a directory opens but does not read: every scanner answers the read error
    let dir = tmp.path().to_str().expect("utf8");
    for scanner in [
        json!({"lines": {}}),
        json!({"to_the_end": {}}),
        json!({"csv": {}}),
        json!({"tar": {}}),
        json!({"chunker": {"size": 8}}),
        json!({"json_documents": {}}),
        json!({"skip_bom": {"into": {"lines": {}}}}),
        json!({"re_match": {"pattern": "x"}}),
    ] {
        let mut i =
            input(&c, json!({"file": {"paths": [dir], "scanner": scanner}})).expect("builds");
        let e = refusal(i.read_batch(&c).await);
        assert!(e.contains("directory"), "{scanner}: {e}");
    }
}

#[tokio::test]
async fn a_file_input_dropped_mid_file_stops_its_reader() {
    let tmp = TempDir::new().expect("tmp");
    let path = tmp.path().join("many.txt");
    std::fs::write(&path, "l\n".repeat(64)).expect("write");
    let c = ctx();
    let mut i = input(&c, json!({"file": {"paths": [path.to_str()]}})).expect("builds");
    assert_eq!(texts(&i.read_batch(&c).await.expect("first").0), ["l"]);
    drop(i);
    let docs = tmp.path().join("docs.json");
    std::fs::write(&docs, "{}".repeat(64)).expect("write");
    let mut i = input(
        &c,
        json!({"file": {"paths": [docs.to_str()], "scanner": {"json_documents": {}}}}),
    )
    .expect("builds");
    assert_eq!(texts(&i.read_batch(&c).await.expect("first").0), ["{}"]);
    drop(i);
    // the reader thread sees the closed channel and ends; the file is left as it was
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(path.exists());
}

#[tokio::test]
async fn delete_on_finish_removes_a_file_only_when_its_last_batch_is_acked() {
    let tmp = TempDir::new().expect("tmp");
    let keep = tmp.path().join("keep.txt");
    let gone = tmp.path().join("gone.txt");
    std::fs::write(&keep, "k1\nk2\n").expect("write");
    std::fs::write(&gone, "g1\n").expect("write");
    let c = ctx();
    let mut i = input(
        &c,
        json!({"file": {"paths": [keep.to_str(), gone.to_str()], "delete_on_finish": true}}),
    )
    .expect("builds");
    let (b, ack) = i.read_batch(&c).await.expect("k1");
    assert_eq!(texts(&b), ["k1"]);
    ack.ack();
    assert!(keep.exists(), "not the last batch yet");
    let (b, ack) = i.read_batch(&c).await.expect("k2");
    assert_eq!(texts(&b), ["k2"]);
    ack.nack(BatchError::new(b, "down".to_string()));
    assert!(keep.exists(), "a nacked file stays");
    let (b, ack) = i.read_batch(&c).await.expect("g1");
    assert_eq!(texts(&b), ["g1"]);
    ack.ack();
    assert!(!gone.exists(), "acked last batch removes the file");
    assert!(matches!(i.read_batch(&c).await, Err(CoreError::EndOfInput)));
}

#[test]
fn a_scanner_reads_every_kind_from_bytes() {
    let obj = |v: JsonValue| v.as_object().expect("object").clone();
    let scan = |conf: JsonValue, raw: &[u8], name: &str| {
        Scanner::from_config(&obj(conf))
            .and_then(|s| s.scan(raw, name))
            .map(|ps| texts(&ps))
    };
    assert_eq!(scan(json!({}), b"a\nb", "").expect("whole"), ["a\nb"]);
    assert_eq!(
        scan(json!({"scanner": {"chunker": {"size": 2}}}), b"abcde", "").expect("chunks"),
        ["ab", "cd", "e"]
    );
    // a size far past the data allocates only what arrives
    assert_eq!(
        scan(
            json!({"scanner": {"chunker": {"size": u64::MAX}}}),
            b"abc",
            ""
        )
        .expect("one"),
        ["abc"]
    );
    assert_eq!(
        scan(
            json!({"scanner": {"json_documents": {}}}),
            br#"{"a":1} [2]"#,
            ""
        )
        .expect("docs"),
        [r#"{"a":1}"#, "[2]"]
    );
    let e = scan(json!({"scanner": {"json_documents": {}}}), b"{\"a\":", "").expect_err("cut");
    assert!(e.to_string().contains("EOF"), "{e}");
    assert_eq!(
        scan(
            json!({"scanner": {"xml_documents": {"operator": "to_json"}}}),
            b"<a>1</a><b/>",
            ""
        )
        .expect("xml"),
        [r#"{"a":"1"}"#, r#"{"b":""}"#]
    );
    let mut tar = tar::Builder::new(Vec::new());
    for (name, body) in [("x.txt", "first"), ("y.txt", "second")] {
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_cksum();
        tar.append_data(&mut h, name, body.as_bytes())
            .expect("entry");
    }
    let tar = tar.into_inner().expect("tar");
    assert_eq!(
        scan(json!({"scanner": {"tar": {}}}), &tar, "").expect("tar"),
        ["first", "second"]
    );
    let e = scan(json!({"scanner": {"tar": {}}}), &[0xFF; 600], "").expect_err("not a tar");
    assert!(!e.to_string().is_empty());
    let e = scan(
        json!({"scanner": {"switch": [{"re_match_name": "\\.csv$", "scanner": {"csv": {}}}]}}),
        b"x",
        "a.txt",
    )
    .expect_err("no case");
    assert!(
        e.to_string().contains("did not match against any scanners"),
        "{e}"
    );
}

#[test]
fn scanner_configs_are_refused_with_the_field_named() {
    let obj = |v: JsonValue| v.as_object().expect("object").clone();
    let cases = [
        (
            json!({"switch": "csv"}),
            "switch scanner takes a list of cases",
        ),
        (
            json!({"switch": ["csv"]}),
            "switch scanner takes a list of cases",
        ),
        (
            json!({"switch": [{"re_match_name": "("}]}),
            "invalid re_match_name",
        ),
        (
            json!({"switch": [{"re_match_name": "x"}]}),
            "each case needs a scanner",
        ),
        (json!({"decompress": {}}), "field algorithm is required"),
        (
            json!({"decompress": {"algorithm": "rar"}}),
            "compression type not recognised: rar",
        ),
    ];
    for (conf, want) in cases {
        let e = Scanner::from_config(&obj(json!({ "scanner": conf }))).expect_err("refused");
        assert!(e.to_string().contains(want), "{conf}: {e}");
    }
}

#[test]
fn a_stream_scanner_defaults_to_lines_and_honours_the_older_codec() {
    let obj = |v: JsonValue| v.as_object().expect("object").clone();
    let scan = |conf: JsonValue, raw: &[u8]| {
        let s = Scanner::lines_by_default(&obj(conf)).expect("scanner");
        (s.is_lines(), texts(&s.scan(raw, "").expect("scan")))
    };
    assert_eq!(
        scan(json!({}), b"a\nb\n"),
        (true, vec!["a".into(), "b".into()])
    );
    assert_eq!(
        scan(json!({"codec": "lines"}), b"a\nb"),
        (true, vec!["a".into(), "b".into()])
    );
    assert_eq!(
        scan(json!({"codec": "all-bytes"}), b"a\nb"),
        (false, vec!["a\nb".into()])
    );
    assert_eq!(
        scan(json!({"codec": "to_the_end"}), b"a\nb"),
        (false, vec!["a\nb".into()])
    );
    assert_eq!(
        scan(json!({"codec": "csv"}), b"h\nv\n"),
        (false, vec![r#"{"h":"v"}"#.into()])
    );
    let (lines, got) = scan(json!({"codec": "tar"}), &[]);
    assert_eq!((lines, got.len()), (false, 0));
    // a custom delimiter is still the lines scanner, but not the newline split
    assert_eq!(
        scan(
            json!({"scanner": {"lines": {"custom_delimiter": "|"}}}),
            b"a|b"
        ),
        (false, vec!["a".into(), "b".into()])
    );
    assert_eq!(
        scan(json!({"scanner": {"to_the_end": {}}}), b"a\nb"),
        (false, vec!["a\nb".into()])
    );
}

// ---------------------------------------------------------------------------------------------
// Outputs

#[tokio::test]
async fn every_output_refuses_a_config_it_cannot_build_and_names_the_field() {
    let c = ctx();
    let cases = [
        (json!({"reject": ""}), "an error message must be provided"),
        (json!({"file": "x"}), "file output config must be an object"),
        (json!({"file": {}}), "file output requires a 'path' field"),
        (json!({"drop_on": {}}), "drop_on requires an 'output' field"),
        (
            json!({"retry": {}}),
            "retry output requires an 'output' field",
        ),
        (
            json!({"retry": {"output": {"drop": {}}, "backoff": {"initial_interval": "soon"}}}),
            "soon",
        ),
        (json!({"fallback": []}), "with no outputs"),
        (json!({"fallback": {}}), "with no outputs"),
        (
            json!({"fallback": [{"drop": {}}, {"rec": "absent"}]}),
            "no recorder absent",
        ),
        (
            json!({"switch": "x"}),
            "switch output config must be an object",
        ),
        (
            json!({"switch": {}}),
            "switch output requires a 'cases' array",
        ),
        (
            json!({"switch": {"cases": [{"output": {"drop": {}}}]}}),
            "fewer than 2 cases",
        ),
        (
            json!({"switch": {"cases": ["x", {}]}}),
            "switch case 0 must be an object",
        ),
        (
            json!({"switch": {"cases": [{"output": {"drop": {}}}, {}]}}),
            "switch case 1 requires an 'output' field",
        ),
        (
            json!({"broker": "x"}),
            "broker output config must be an object",
        ),
        (
            json!({"broker": {"pattern": "scatter", "outputs": [{"drop": {}}]}}),
            "broker pattern was not recognised: scatter",
        ),
        (json!({"broker": {"outputs": []}}), "with no outputs"),
        (
            json!({"resource": ""}),
            "resource output requires a resource name",
        ),
        (
            json!({"cache": "x"}),
            "cache output config must be an object",
        ),
        (
            json!({"cache": {}}),
            "cache output requires a 'target' field",
        ),
    ];
    for (conf, want) in cases {
        let e = output(&c, conf.clone())
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| panic!("{conf} accepted"));
        assert!(e.contains(want), "{conf}: {e}");
    }
}

#[tokio::test]
async fn output_types_are_named_as_bento_names_them() {
    let c = ctx();
    let cases = [
        (json!({"drop": {}}), "drop"),
        (json!({"inproc": ""}), "drop"),
        (json!({"stdout": {}}), "stdout"),
        (json!({"file": {"path": "x"}}), "file"),
        (json!({"reject": "no"}), "reject"),
        (json!({"reject_errored": {"drop": {}}}), "reject_errored"),
        (json!({"drop_on": {"output": {"drop": {}}}}), "drop_on"),
        (json!({"retry": {"output": {"drop": {}}}}), "retry"),
        (json!({"fallback": [{"drop": {}}]}), "fallback"),
        (
            json!({"switch": {"cases": [{"output": {"drop": {}}}, {"output": {"drop": {}}}]}}),
            "switch",
        ),
        (json!({"broker": {"outputs": [{"drop": {}}]}}), "broker"),
        (json!({"resource": "r"}), "resource"),
        (json!({"cache": {"target": "c"}}), "cache"),
        (json!({"sync_response": {}}), "sync_response"),
    ];
    for (conf, want) in cases {
        let o = output(&c, conf).expect("builds");
        assert_eq!(o.output_type(), want);
    }
    for conf in [json!({"drop": {}}), json!({"inproc": ""})] {
        output(&c, conf)
            .expect("builds")
            .write_batch(&c, parts(&["x"]))
            .await
            .expect("dropped");
    }
}

#[tokio::test]
async fn a_file_output_names_the_path_it_cannot_resolve_create_or_open() {
    let tmp = TempDir::new().expect("tmp");
    let c = ctx();
    let o = output(&c, json!({"file": {"path": "${! throw(\"no path\") }"}})).expect("builds");
    let e = refusal(o.write_batch(&c, parts(&["x"])).await);
    assert!(
        e.contains("path interpolation error") && e.contains("no path"),
        "{e}"
    );
    // a parent that is a file cannot become a directory
    let blocker = tmp.path().join("blocker");
    std::fs::write(&blocker, "").expect("write");
    let o = output(
        &c,
        json!({"file": {"path": blocker.join("x.txt").to_str()}}),
    )
    .expect("builds");
    assert!(!refusal(o.write_batch(&c, parts(&["x"])).await).is_empty());
    // a directory does not open as a file, the root directory included
    for dir in [tmp.path().to_str().expect("utf8"), "/"] {
        let o = output(&c, json!({"file": {"path": dir}})).expect("builds");
        assert!(refusal(o.write_batch(&c, parts(&["x"])).await).contains("failed to open"));
    }
    // a full device refuses the bytes
    let o = output(
        &c,
        json!({"file": {"path": "/dev/full", "codec": "all-bytes"}}),
    )
    .expect("builds");
    assert!(!refusal(o.write_batch(&c, parts(&["x"])).await).is_empty());
}

#[tokio::test]
async fn a_file_output_drops_a_held_file_whose_write_fails() {
    let tmp = TempDir::new().expect("tmp");
    let fifo = tmp.path().join("pipe");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo runs");
    assert!(made.success());
    // the reader takes the first batch and goes away, so the held file's next write breaks
    let reader_path = fifo.clone();
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut f = std::fs::File::open(reader_path).expect("open for reading");
        let mut buf = [0u8; 6];
        f.read_exact(&mut buf).expect("first batch");
        buf
    });
    let c = ctx();
    let o = output(
        &c,
        json!({"file": {"path": fifo.to_str(), "codec": "lines"}}),
    )
    .expect("builds");
    o.write_batch(&c, parts(&["first"]))
        .await
        .expect("first batch");
    assert_eq!(&reader.join().expect("reader"), b"first\n");
    let e = refusal(o.write_batch(&c, parts(&["second"])).await);
    assert!(e.contains("failed to write") && e.contains("pipe"), "{e}");
}

#[tokio::test]
async fn a_file_output_appends_lines_and_a_truncating_codec_keeps_the_last_message() {
    let tmp = TempDir::new().expect("tmp");
    let c = ctx_with(Some(tmp.path().to_path_buf()), watch::channel(false).1);
    let o = output(
        &c,
        json!({"file": {"path": "/out/${! content() }.txt", "codec": "append"}}),
    )
    .expect("builds");
    o.write_batch(&c, parts(&["a", "a", "b"]))
        .await
        .expect("written");
    o.write_batch(&c, parts(&["a"])).await.expect("written");
    // the next batch to the last file appends to the file held open
    o.write_batch(&c, parts(&["a"])).await.expect("written");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("out/a.txt")).expect("a"),
        "aaaa"
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("out/b.txt")).expect("b"),
        "b"
    );
    let o = output(
        &c,
        json!({"file": {"path": "last.txt", "codec": "all-bytes"}}),
    )
    .expect("builds");
    o.write_batch(&c, parts(&["one", "two"]))
        .await
        .expect("written");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("last.txt")).expect("last"),
        "two"
    );
}

#[tokio::test]
async fn reject_nacks_the_batch_with_its_interpolated_message() {
    let c = ctx();
    let o = output(&c, json!({"reject": "refused ${! content() }"})).expect("builds");
    match o.write_batch(&c, parts(&["m1"])).await {
        Err(CoreError::Batch(e)) => assert_eq!(e.message, "refused m1"),
        other => panic!("{other:?}"),
    }
    let o = output(&c, json!({"reject": "${! throw(\"boom\") }"})).expect("builds");
    let e = refusal(o.write_batch(&c, parts(&["m1"])).await);
    assert!(e.contains("reject message interpolation error"), "{e}");
}

#[tokio::test]
async fn reject_errored_sends_clean_parts_and_nacks_the_errored_ones() {
    let c = ctx();
    let r = rec("re-a", vec![], true);
    let o = output(&c, json!({"reject_errored": {"rec": "re-a"}})).expect("builds");
    let mut batch = parts(&["ok", "bad", "worse"]);
    batch[1].set_error_msg("first failure");
    batch[2].set_error_msg("second failure");
    match o.write_batch(&c, batch).await {
        Err(CoreError::Batch(e)) => {
            assert_eq!(e.message, "first failure");
            assert_eq!(e.indexed_errors(), 2);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(r.bodies(), [vec!["ok"]]);
    // only errored parts: nothing goes to the child
    let mut batch = parts(&["bad"]);
    batch[0].set_error_msg("failure");
    assert!(o.write_batch(&c, batch).await.is_err());
    assert_eq!(r.bodies().len(), 1);
    o.write_batch(&c, parts(&["clean"])).await.expect("clean");
    // a child's close failure is the composite's
    assert!(refusal(o.close().await).contains("close failed"));
    assert_eq!(r.closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn drop_on_swallows_the_errors_it_is_told_to_and_passes_the_rest() {
    let c = ctx();
    let r = rec(
        "dropon-a",
        vec![Fail::Other("timeout talking to x"), Fail::Other("refused")],
        false,
    );
    let o = output(
        &c,
        json!({"drop_on": {"error_patterns": ["timeout"], "output": {"rec": "dropon-a"}}}),
    )
    .expect("builds");
    o.write_batch(&c, parts(&["1"]))
        .await
        .expect("timeout dropped");
    assert!(refusal(o.write_batch(&c, parts(&["2"])).await).contains("refused"));
    o.write_batch(&c, parts(&["3"])).await.expect("written");
    assert_eq!(r.bodies(), [vec!["1"], vec!["2"], vec!["3"]]);
    o.close().await.expect("closes");
    assert_eq!(r.closes.load(Ordering::SeqCst), 1);

    rec("dropon-b", vec![Fail::Other("anything")], false);
    let o = output(
        &c,
        json!({"drop_on": {"error": true, "output": {"rec": "dropon-b"}}}),
    )
    .expect("builds");
    o.write_batch(&c, parts(&["1"]))
        .await
        .expect("every error dropped");
}

#[tokio::test]
async fn retry_tries_again_until_success_or_the_limit() {
    let c = ctx();
    let fast = json!({"initial_interval": "1ms", "max_interval": "2ms", "max_elapsed_time": "0s"});
    let r = rec(
        "retry-a",
        vec![Fail::Other("once"), Fail::Other("twice")],
        false,
    );
    let o = output(
        &c,
        json!({"retry": {"backoff": fast, "output": {"rec": "retry-a"}}}),
    )
    .expect("builds");
    o.write_batch(&c, parts(&["x"])).await.expect("third try");
    assert_eq!(r.bodies().len(), 3);
    o.close().await.expect("closes");
    assert_eq!(r.closes.load(Ordering::SeqCst), 1);

    let r = rec(
        "retry-b",
        vec![Fail::Other("1"), Fail::Other("2"), Fail::Other("3")],
        false,
    );
    let o = output(
        &c,
        json!({"retry": {"backoff": fast, "max_retries": 1, "output": {"rec": "retry-b"}}}),
    )
    .expect("builds");
    assert_eq!(refusal(o.write_batch(&c, parts(&["x"])).await), "2");
    assert_eq!(r.bodies().len(), 2);

    // the backoff's elapsed time runs out before the retries do
    let r = rec(
        "retry-c",
        ["1", "2", "3", "4"].map(Fail::Other).into_iter().collect(),
        false,
    );
    let short =
        json!({"initial_interval": "20ms", "max_interval": "20ms", "max_elapsed_time": "30ms"});
    let o = output(
        &c,
        json!({"retry": {"backoff": short, "output": {"rec": "retry-c"}}}),
    )
    .expect("builds");
    assert_eq!(refusal(o.write_batch(&c, parts(&["x"])).await), "3");
    assert_eq!(r.bodies().len(), 3);
}

#[tokio::test]
async fn retry_gives_up_when_the_stream_shuts_down() {
    let (tx, rx) = watch::channel(false);
    let c = ctx_with(None, rx);
    tx.send(true).expect("signal");
    rec("retry-stop", vec![Fail::Other("down")], false);
    let o = output(
        &c,
        json!({"retry": {"backoff": {"initial_interval": "1ms", "max_interval": "1ms"}, "output": {"rec": "retry-stop"}}}),
    )
    .expect("builds");
    assert!(matches!(
        o.write_batch(&c, parts(&["x"])).await,
        Err(CoreError::ComponentClosed)
    ));
}

#[tokio::test]
async fn fallback_hands_only_the_failed_parts_to_the_next_tier_with_the_error() {
    let c = ctx();
    let first = rec("fb-1", vec![Fail::Indexed("partial", vec![1])], false);
    let second = rec("fb-2", vec![Fail::Other("second down")], true);
    let third = rec("fb-3", vec![], false);
    let o = output(
        &c,
        json!({"fallback": [{"rec": "fb-1"}, {"rec": "fb-2"}, {"rec": "fb-3"}]}),
    )
    .expect("builds");
    o.write_batch(&c, parts(&["a", "b", "c"]))
        .await
        .expect("third tier takes it");
    assert_eq!(first.bodies(), [vec!["a", "b", "c"]]);
    assert_eq!(second.bodies(), [vec!["b"]]);
    assert_eq!(second.marks(), [vec!["part 1 failed"]]);
    assert_eq!(third.bodies(), [vec!["b"]]);
    assert_eq!(third.marks(), [vec!["second down"]]);
    // every child closes, and the first close failure is the answer
    assert!(refusal(o.close().await).contains("close failed"));
    assert_eq!(
        [&first, &second, &third].map(|r| r.closes.load(Ordering::SeqCst)),
        [1, 1, 1]
    );
}

#[tokio::test]
async fn fallback_marks_the_whole_batch_and_answers_the_last_tier_error() {
    let c = ctx();
    // a batch error without indexes, and one naming an index the batch of one cannot narrow
    rec("fbw-1", vec![Fail::Indexed("whole batch", vec![])], false);
    let second = rec("fbw-2", vec![Fail::Indexed("one part", vec![0])], false);
    let third = rec("fbw-3", vec![Fail::Other("last down")], false);
    let o = output(
        &c,
        json!({"fallback": [{"rec": "fbw-1"}, {"rec": "fbw-2"}, {"rec": "fbw-3"}]}),
    )
    .expect("builds");
    assert_eq!(
        refusal(o.write_batch(&c, parts(&["a", "b"])).await),
        "last down"
    );
    assert_eq!(second.marks(), [vec!["whole batch", "whole batch"]]);
    assert_eq!(third.bodies(), [vec!["a"]]);
    assert_eq!(third.marks(), [vec!["part 0 failed"]]);
    // an index past the batch narrows nothing: every part goes on with the batch error
    rec("fbo-1", vec![Fail::Indexed("out of range", vec![9])], false);
    let next = rec("fbo-2", vec![], false);
    let o = output(
        &c,
        json!({"fallback": [{"rec": "fbo-1"}, {"rec": "fbo-2"}]}),
    )
    .expect("builds");
    o.write_batch(&c, parts(&["a", "b"]))
        .await
        .expect("second tier");
    assert_eq!(next.marks(), [vec!["out of range", "out of range"]]);
    // a part the child sent twice goes on once
    rec("fbd-1", vec![Fail::Doubled("twice")], false);
    let next = rec("fbd-2", vec![], false);
    let o = output(
        &c,
        json!({"fallback": [{"rec": "fbd-1"}, {"rec": "fbd-2"}]}),
    )
    .expect("builds");
    o.write_batch(&c, parts(&["a", "b"]))
        .await
        .expect("second tier");
    assert_eq!(next.bodies(), [vec!["a"]]);
    assert_eq!(next.marks(), [vec!["first copy"]]);
    let only = rec("fbw-only", vec![], false);
    let o = output(&c, json!({"fallback": [{"rec": "fbw-only"}]})).expect("builds");
    o.write_batch(&c, parts(&["x"])).await.expect("first tier");
    assert_eq!(only.bodies(), [vec!["x"]]);
}

#[tokio::test]
async fn switch_routes_by_check_continues_when_told_and_refuses_in_strict_mode() {
    let c = ctx();
    let a = rec("sw-a", vec![], false);
    let b = rec("sw-b", vec![], false);
    let o = output(
        &c,
        json!({"switch": {"cases": [
            {"check": "content().has_prefix(\"a\")", "output": {"rec": "sw-a"}, "continue": true},
            {"check": " ", "output": {"rec": "sw-b"}},
        ]}}),
    )
    .expect("builds");
    o.write_batch(&c, parts(&["a1", "b1"]))
        .await
        .expect("routed");
    assert_eq!(a.bodies(), [vec!["a1"]]);
    assert_eq!(b.bodies(), [vec!["a1", "b1"]]);
    o.close().await.expect("closes");
    assert_eq!(a.closes.load(Ordering::SeqCst), 1);

    rec("sws-a", vec![], false);
    rec(
        "sws-b",
        vec![Fail::Other("b down"), Fail::Other("b down again")],
        false,
    );
    let o = output(
        &c,
        json!({"switch": {"strict_mode": true, "retry_until_success": true, "cases": [
            {"check": "content() == \"a\"", "output": {"rec": "sws-a"}},
            {"check": "content() == \"b\"", "output": {"rec": "sws-b"}},
        ]}}),
    )
    .expect("builds");
    let e = refusal(o.write_batch(&c, parts(&["c"])).await);
    assert!(e.contains("no switch output conditions were met"), "{e}");
}

#[tokio::test]
async fn switch_retries_a_failing_case_until_it_succeeds_when_told_to() {
    let c = ctx();
    let b = rec("swr-b", vec![Fail::Other("b down")], false);
    rec("swr-a", vec![], false);
    let o = output(
        &c,
        json!({"switch": {"retry_until_success": true, "cases": [
            {"check": "content() == \"a\"", "output": {"rec": "swr-a"}},
            {"output": {"rec": "swr-b"}},
        ]}}),
    )
    .expect("builds");
    o.write_batch(&c, parts(&["b"])).await.expect("retried");
    assert_eq!(b.bodies(), [vec!["b"], vec!["b"]]);
}

#[tokio::test]
async fn a_broker_fans_out_in_parallel_or_in_order_and_fails_fast_when_told() {
    let c = ctx();
    for pattern in ["fan_out_fail_fast", "fan_out_sequential_fail_fast"] {
        let ok = rec(&format!("{pattern}-ok"), vec![], false);
        rec(
            &format!("{pattern}-bad"),
            vec![Fail::Other("one down")],
            false,
        );
        let o = output(
            &c,
            json!({"broker": {"pattern": pattern, "outputs": [
                {"rec": format!("{pattern}-ok")}, {"rec": format!("{pattern}-bad")},
            ]}}),
        )
        .expect("builds");
        assert_eq!(
            refusal(o.write_batch(&c, parts(&["x"])).await),
            "one down",
            "{pattern}"
        );
        assert_eq!(ok.bodies(), [vec!["x"]], "{pattern}");
    }
    // the plain patterns retry each output until it takes the batch
    for pattern in ["fan_out", "fan_out_sequential"] {
        let r = rec(&format!("{pattern}-retry"), vec![Fail::Other("blip")], true);
        let o = output(
            &c,
            json!({"broker": {"pattern": pattern, "copies": 1, "outputs": [{"rec": format!("{pattern}-retry")}]}}),
        )
        .expect("builds");
        o.write_batch(&c, parts(&["y"])).await.expect("retried");
        assert_eq!(r.bodies().len(), 2, "{pattern}");
        assert!(refusal(o.close().await).contains("close failed"));
    }
}

#[tokio::test]
async fn a_round_robin_broker_takes_turns_and_tries_the_others_when_one_fails() {
    let c = ctx();
    for pattern in ["round_robin", "greedy"] {
        let a = rec(&format!("{pattern}-a"), vec![Fail::Other("a down")], false);
        let b = rec(
            &format!("{pattern}-b"),
            vec![Fail::Other("b down"), Fail::Other("b down")],
            false,
        );
        let o = output(
            &c,
            json!({"broker": {"pattern": pattern, "outputs": [
                {"rec": format!("{pattern}-a")}, {"rec": format!("{pattern}-b")},
            ]}}),
        )
        .expect("builds");
        // turn 1 starts at a (down), falls over to b (down): the last error answers
        assert_eq!(refusal(o.write_batch(&c, parts(&["1"])).await), "b down");
        // turn 2 starts at b (down again), a takes it
        o.write_batch(&c, parts(&["2"]))
            .await
            .expect("a takes turn 2");
        // turn 3 starts at a
        o.write_batch(&c, parts(&["3"]))
            .await
            .expect("a takes turn 3");
        assert_eq!(a.bodies(), [vec!["1"], vec!["2"], vec!["3"]], "{pattern}");
        assert_eq!(b.bodies(), [vec!["1"], vec!["2"]], "{pattern}");
    }
}

#[tokio::test]
async fn a_resource_output_writes_to_the_named_resource_and_refuses_a_missing_one() {
    let c = ctx();
    let r = Arc::new(Rec::default());
    c.resources
        .insert_output(c.org_id(), "shared", Arc::new(RecOutput(r.clone())));
    output(&c, json!({"resource": "shared"}))
        .expect("builds")
        .write_batch(&c, parts(&["x"]))
        .await
        .expect("written");
    assert_eq!(r.bodies(), [vec!["x"]]);
    let o = output(&c, json!({"resource": "absent"})).expect("builds");
    assert!(refusal(o.write_batch(&c, parts(&["x"])).await).contains("absent"));
}

#[tokio::test]
async fn a_cache_output_stores_each_part_under_its_key() {
    let c = ctx();
    let cache = Arc::new(MemCache::default());
    c.resources.insert_cache(c.org_id(), "store", cache.clone());
    let o = output(
        &c,
        json!({"cache": {"target": "store", "key": "k-${! content() }"}}),
    )
    .expect("builds");
    o.write_batch(&c, parts(&["a", "b"])).await.expect("stored");
    assert_eq!(cache.get("k-a").await.expect("a"), Bytes::from_static(b"a"));
    assert_eq!(cache.get("k-b").await.expect("b"), Bytes::from_static(b"b"));
    // the default key counts the stored items
    let o = output(&c, json!({"cache": {"target": "store"}})).expect("builds");
    o.write_batch(&c, parts(&["c"])).await.expect("stored");
    assert_eq!(cache.0.lock().expect("lock").len(), 3);
    let o = output(
        &c,
        json!({"cache": {"target": "store", "key": "${! throw(\"no key\") }"}}),
    )
    .expect("builds");
    assert!(refusal(o.write_batch(&c, parts(&["x"])).await).contains("no key"));
    let o = output(&c, json!({"cache": {"target": "absent"}})).expect("builds");
    assert!(refusal(o.write_batch(&c, parts(&["x"])).await).contains("absent"));
}

#[tokio::test]
async fn sync_response_hands_the_batch_to_the_store_of_its_first_part() {
    let c = ctx();
    let o = output(&c, json!({"sync_response": {}})).expect("builds");
    let store = SyncResponseStore::new();
    let mut batch = parts(&["reply", "more"]);
    batch[0].with_sync_response_store(store.clone());
    o.write_batch(&c, batch).await.expect("stored");
    let got: Vec<Vec<String>> = store.read().iter().map(texts).collect();
    assert_eq!(got, [vec!["reply", "more"]]);
    // no store and no parts: nothing to answer
    o.write_batch(&c, parts(&["x"])).await.expect("no store");
    o.write_batch(&c, Vec::new()).await.expect("empty");
    assert_eq!(store.read().len(), 1);
}

// ---------------------------------------------------------------------------------------------
// stdin and stdout: the test runs itself as a child with a piped stdin, the only way to give a
// test process a stdin of its own.

const CHILD: &str = "EKIBEN_IO_FILE_STDIO_CHILD";

#[tokio::test]
async fn stdin_reads_messages_and_stdout_writes_them_one_per_line() {
    if let Ok(mode) = std::env::var(CHILD) {
        let c = ctx();
        let conf = match mode.as_str() {
            "lines" => json!({"stdin": {}}),
            _ => json!({"stdin": {"scanner": {"lines": {"max_buffer_size": 4}}}}),
        };
        let mut i = input(&c, conf).expect("builds");
        let lines = output(&c, json!({"stdout": {}})).expect("builds");
        let raw = output(&c, json!({"stdout": {"codec": "all-bytes"}})).expect("builds");
        loop {
            match i.read_batch(&c).await {
                Ok((b, _)) => {
                    lines.write_batch(&c, b.clone()).await.expect("stdout");
                    raw.write_batch(&c, b).await.expect("stdout");
                }
                Err(CoreError::EndOfInput) => break,
                Err(e) => {
                    raw.write_batch(&c, parts(&[&format!("ERR {e}")]))
                        .await
                        .expect("stdout");
                    break;
                }
            }
        }
        // a second read after the end keeps answering the end
        if mode == "lines" {
            assert!(matches!(i.read_batch(&c).await, Err(CoreError::EndOfInput)));
        }
        return;
    }
    let run = |mode: &str, stdin: &[u8]| {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "stdin_reads_messages_and_stdout_writes_them_one_per_line",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("child");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(stdin)
            .expect("write");
        let out = child.wait_with_output().expect("child ends");
        assert!(out.status.success());
        String::from_utf8(out.stdout).expect("utf8")
    };
    let out = run("lines", b"one\ntwo\n");
    assert!(out.contains("one\nonetwo\ntwo"), "{out:?}");
    let out = run("capped", b"ok\ntoo long\n");
    assert!(
        out.contains("ok\nokERR failed to read stdin: bufio.Scanner: token too long"),
        "{out:?}"
    );
}
