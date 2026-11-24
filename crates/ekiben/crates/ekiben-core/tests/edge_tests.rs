//! Edge cases of the engine pieces the other suites leave out: the stream buffer and its feeder,
//! paged execution, the try/catch executors, host-target admission, scoped secrets, metric
//! config refusals, the build paths of nested components, the message part's byte/structured
//! views and the wire format's malformed inputs (T-7685).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use ekiben_bloblang::{Environment, MessageBatch, Value};
use ekiben_core::buffer::{BufferedInput, MemoryBuffer, DEFAULT_MEMORY_BUFFER_LIMIT};
use ekiben_core::config::extract_metadata_filter;
use ekiben_core::paging::page_cuts;
use ekiben_core::pipeline::{metric_path, BatchedOutput};
use ekiben_core::prometheus::{escape_label_value, Gauge, MetricsConfig, Namespaced, PromExporter};
use ekiben_core::{
    deserialize_bytes, execute_catch_all, execute_paged, execute_try_all, interpolate_env_str,
    lint_fields, redact_config_value, redact_url_str, AckResponder, BackoffConfig, Batch,
    BatchError, BatchPolicyConfig, Batcher, Buffer, CombinedAcker, ComponentConfig, CoreError,
    HostTarget, Input, MetricsRegistry, ObservedProcessor, OrgContext, OrgId, OrgQuotas, Output,
    Paging, Part, PartsView, ProcContext, Processor, Registry, ResourceManager, SortGroup,
    StreamConfig, StreamContext, StreamMetrics, TrackedTransaction, Transaction,
};
use serde_json::json;
use tokio::sync::watch;

// ---------- helpers ----------

fn stream_ctx_with(reg: Arc<Registry>, metrics: Arc<StreamMetrics>) -> (watch::Sender<bool>, StreamContext) {
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
            metrics,
            label: "root".into(),
            path: String::new(),
            file_root: None,
            env_vars: Arc::default(),
            shutdown_rx: rx,
        },
    )
}

fn stream_ctx() -> (watch::Sender<bool>, StreamContext) {
    stream_ctx_with(Arc::new(Registry::new()), StreamMetrics::new("s1", "o1"))
}

/// A stream context whose metrics are attached to a fresh exporter, returned for rendering.
fn observed_ctx(reg: Arc<Registry>) -> (watch::Sender<bool>, StreamContext, Arc<PromExporter>) {
    let exporter = Arc::new(PromExporter::with_defaults());
    let metrics = StreamMetrics::new("s1", "o1");
    metrics.attach(Namespaced::bare(exporter.clone()).with_labels(&[("stream", "s1")]));
    let (tx, ctx) = stream_ctx_with(reg, metrics);
    (tx, ctx, exporter)
}

fn proc_ctx() -> (watch::Sender<bool>, ProcContext) {
    let (tx, s) = stream_ctx();
    (tx, s.to_proc_context("pipeline", "", "pipeline.processors"))
}

fn parts(xs: &[&str]) -> Batch {
    xs.iter().map(|s| Part::from_str(s)).collect()
}

fn texts(b: &[Part]) -> Vec<String> {
    b.iter()
        .map(|p| String::from_utf8_lossy(&p.as_bytes_ref()).into_owned())
        .collect()
}

fn conf(v: serde_json::Value) -> ComponentConfig {
    ComponentConfig::from_json_value(&v, "noop", None).unwrap()
}

type AckLog = Arc<Mutex<Vec<Option<String>>>>;

fn logged(log: &AckLog) -> AckResponder {
    let log = log.clone();
    AckResponder::from_fn(move |e| log.lock().unwrap().push(e.map(|e| e.message)))
}

/// Appends `!` to every part.
struct Bang;
#[async_trait]
impl Processor for Bang {
    fn processor_type(&self) -> &'static str {
        "bang"
    }
    async fn process_batch(&self, _: &ProcContext, mut b: Batch) -> Result<Vec<Batch>, CoreError> {
        for p in &mut b {
            let s = format!("{}!", String::from_utf8_lossy(&p.as_bytes()));
            p.set_bytes(s);
        }
        Ok(vec![b])
    }
}

/// Fails every batch.
struct Boom;
#[async_trait]
impl Processor for Boom {
    fn processor_type(&self) -> &'static str {
        "boom"
    }
    async fn process_batch(&self, _: &ProcContext, _: Batch) -> Result<Vec<Batch>, CoreError> {
        Err(CoreError::other("boom"))
    }
}

/// Expands each message into `n` copies, page by page through `pages`, counting the pages made.
struct Expand {
    n: usize,
    made: Arc<AtomicUsize>,
}
#[async_trait]
impl Processor for Expand {
    fn processor_type(&self) -> &'static str {
        "unarchive"
    }
    async fn process_batch(&self, _: &ProcContext, b: Batch) -> Result<Vec<Batch>, CoreError> {
        let out = b
            .iter()
            .flat_map(|p| (0..self.n).map(move |_| p.shallow_copy()))
            .collect();
        Ok(vec![out])
    }
    fn pages(&self, _: &ProcContext, b: Batch, rows: usize) -> Result<Box<ekiben_core::traits::Pages>, Batch> {
        let all: Vec<Part> = b
            .iter()
            .flat_map(|p| (0..self.n).map(move |_| p.shallow_copy()))
            .collect();
        let made = self.made.clone();
        let chunks: Vec<Batch> = all.chunks(rows).map(<[Part]>::to_vec).collect();
        Ok(Box::new(chunks.into_iter().inspect(move |_| {
            made.fetch_add(1, Ordering::SeqCst);
        })))
    }
}

/// Input serving a scripted list of answers, then `EndOfInput`.
struct Script {
    answers: VecDeque<Result<Batch, CoreError>>,
    acks: AckLog,
    closed: Arc<AtomicBool>,
}
#[async_trait]
impl Input for Script {
    fn input_type(&self) -> &'static str {
        "script"
    }
    async fn read_batch(&mut self, _: &StreamContext) -> Result<(Batch, AckResponder), CoreError> {
        match self.answers.pop_front() {
            Some(Ok(b)) => Ok((b, logged(&self.acks))),
            Some(Err(e)) => Err(e),
            None => Err(CoreError::EndOfInput),
        }
    }
    async fn close(&mut self) -> Result<(), CoreError> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

fn script(answers: Vec<Result<Batch, CoreError>>) -> (Box<dyn Input>, AckLog, Arc<AtomicBool>) {
    let acks = AckLog::default();
    let closed = Arc::new(AtomicBool::new(false));
    (
        Box::new(Script {
            answers: answers.into(),
            acks: acks.clone(),
            closed: closed.clone(),
        }),
        acks,
        closed,
    )
}

/// A buffer holding transactions with their acks untouched, so the reader decides when the
/// input's transactions are acknowledged; it uses the trait's `end_of_input` and `close`.
#[derive(Default)]
struct Holding {
    queue: Mutex<VecDeque<Transaction>>,
    refuse: bool,
}
#[async_trait]
impl Buffer for Holding {
    async fn push(&self, tx: Transaction) -> Result<(), CoreError> {
        if self.refuse {
            return Err(CoreError::other("buffer full"));
        }
        self.queue.lock().unwrap().push_back(tx);
        Ok(())
    }
    async fn pop(&self, _: &StreamContext) -> Result<Transaction, CoreError> {
        self.queue
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(CoreError::Timeout)
    }
}

/// Output collecting what it was given and counting its connects.
#[derive(Default)]
struct Collect {
    got: Mutex<Vec<Batch>>,
    connects: AtomicUsize,
}
#[async_trait]
impl Output for Collect {
    fn output_type(&self) -> &'static str {
        "collect"
    }
    fn label(&self) -> &str {
        "sink"
    }
    async fn connect(&self, _: &StreamContext) -> Result<(), CoreError> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn write_batch(&self, _: &StreamContext, b: Batch) -> Result<(), CoreError> {
        self.got.lock().unwrap().push(b);
        Ok(())
    }
    fn is_connected(&self) -> bool {
        true
    }
}

// ---------- buffer ----------

#[tokio::test]
async fn a_buffered_input_passes_every_batch_then_ends_after_the_input() {
    let (_stop, ctx) = stream_ctx();
    let (input, acks, closed) = script(vec![Ok(parts(&["a"])), Err(CoreError::Timeout), Ok(parts(&["b", "c"]))]);
    let buffer = Arc::new(MemoryBuffer::new(1024, Gauge::noop()));
    let mut buffered = BufferedInput::new(input, buffer);
    assert_eq!(buffered.input_type(), "script");
    buffered.connect(&ctx).await.unwrap();
    // A second connect has no input left to start.
    buffered.connect(&ctx).await.unwrap();
    let (b1, ack1) = buffered.read_batch(&ctx).await.unwrap();
    let (b2, ack2) = buffered.read_batch(&ctx).await.unwrap();
    assert_eq!((texts(&b1), texts(&b2)), (vec!["a".to_string()], vec!["b".into(), "c".into()]));
    ack1.ack();
    ack2.ack();
    assert!(matches!(buffered.read_batch(&ctx).await, Err(CoreError::EndOfInput)));
    // The memory buffer acknowledges each batch at the input once stored.
    assert_eq!(*acks.lock().unwrap(), [None, None]);
    assert!(closed.load(Ordering::SeqCst));
    buffered.close().await.unwrap();
}

#[tokio::test]
async fn a_failed_read_ends_the_buffered_input_with_that_error() {
    let (_stop, ctx) = stream_ctx();
    let (input, _, _) = script(vec![Ok(parts(&["a"])), Err(CoreError::other("socket reset"))]);
    let mut buffered = BufferedInput::new(input, Arc::new(MemoryBuffer::new(1024, Gauge::noop())));
    buffered.connect(&ctx).await.unwrap();
    assert_eq!(texts(&buffered.read_batch(&ctx).await.unwrap().0), ["a"]);
    let err = buffered.read_batch(&ctx).await.unwrap_err();
    assert!(err.to_string().contains("socket reset"), "{err}");
    // The error is reported once; the next read is a plain end.
    assert!(matches!(buffered.read_batch(&ctx).await, Err(CoreError::EndOfInput)));
}

#[tokio::test]
async fn the_buffer_hears_of_the_end_only_after_every_input_transaction_is_acked() {
    let (_stop, ctx) = stream_ctx();
    let (input, acks, closed) = script(vec![Ok(parts(&["a"]))]);
    let buffer = Arc::new(Holding::default());
    let mut buffered = BufferedInput::new(input, buffer.clone());
    buffered.connect(&ctx).await.unwrap();
    let (batch, ack) = loop {
        match buffered.read_batch(&ctx).await {
            Ok(got) => break got,
            Err(CoreError::Timeout) => tokio::time::sleep(Duration::from_millis(5)).await,
            Err(e) => panic!("{e}"),
        }
    };
    assert_eq!(texts(&batch), ["a"]);
    tokio::time::sleep(Duration::from_millis(30)).await;
    // The input is exhausted but its transaction waits: the feeder has not closed it yet.
    assert!(!closed.load(Ordering::SeqCst));
    ack.nack(BatchError::new(batch, "rejected downstream"));
    for _ in 0..100 {
        if closed.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(closed.load(Ordering::SeqCst));
    assert_eq!(*acks.lock().unwrap(), [Some("rejected downstream".to_string())]);
}

#[tokio::test]
async fn a_stopping_stream_releases_a_feeder_waiting_on_acks() {
    let (stop, ctx) = stream_ctx();
    // The buffer refuses the write and drops the ack, so the transaction is never acked.
    let (input, _, closed) = script(vec![Ok(parts(&["lost"]))]);
    let buffer = Arc::new(Holding {
        refuse: true,
        ..Holding::default()
    });
    let mut buffered = BufferedInput::new(input, buffer);
    buffered.connect(&ctx).await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!closed.load(Ordering::SeqCst));
    stop.send(true).unwrap();
    for _ in 0..100 {
        if closed.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(closed.load(Ordering::SeqCst));
    buffered.close().await.unwrap();
}

#[tokio::test]
async fn a_memory_buffer_reads_its_limit_and_caps_it_by_the_org_quota() {
    let (_stop, ctx, exporter) = observed_ctx(Arc::new(Registry::new()));
    let default = MemoryBuffer::from_config(&conf(json!({"memory": {}})), &ctx).unwrap();
    // The default limit is Bento's 500 MB, capped by the organization's run memory quota.
    let quota = ctx.org.get_quotas().max_memory_bytes_per_run;
    let big = Transaction::new(
        vec![Part::new(vec![0u8; DEFAULT_MEMORY_BUFFER_LIMIT.min(quota) + 1])],
        AckResponder::noop(),
    );
    assert!(default.write(big).await.is_err());
    let small = MemoryBuffer::from_config(&conf(json!({"memory": {"limit": 3}})), &ctx).unwrap();
    small
        .write(Transaction::new(parts(&["ab"]), AckResponder::noop()))
        .await
        .unwrap();
    assert!(exporter.render(&|_| true).contains("buffer_backlog"));
    let (_s, rx) = watch::channel(true);
    assert_eq!(texts(&small.read(rx.clone()).await.unwrap().payload), ["ab"]);
    // Nothing left and the stream stopping: the read ends.
    assert!(matches!(small.read(rx).await, Err(CoreError::EndOfInput)));
    for bad in [json!({"limit": "lots"}), json!({"limit": -1}), json!({"limit": 0})] {
        let err = MemoryBuffer::from_config(&conf(json!({ "memory": bad })), &ctx).err().unwrap();
        assert!(err.to_string().contains("buffer memory: limit"), "{err}");
    }
    let null = MemoryBuffer::from_config(&conf(json!({"memory": {"limit": null}})), &ctx);
    assert!(null.is_ok());
}

#[tokio::test]
async fn a_waiting_memory_read_ends_when_the_stream_stops() {
    let buf = Arc::new(MemoryBuffer::new(8, Gauge::noop()));
    let (stop, rx) = watch::channel(false);
    let reader = buf.clone();
    let pending = tokio::spawn(async move { reader.read(rx).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!pending.is_finished());
    stop.send(true).unwrap();
    assert!(matches!(pending.await.unwrap(), Err(CoreError::EndOfInput)));
}

#[tokio::test]
async fn the_registry_builds_no_buffer_for_none_and_refuses_an_unknown_one() {
    let reg = Registry::new();
    reg.register_buffer("memory", |c, ctx| {
        Ok(Arc::new(MemoryBuffer::from_config(c, ctx)?) as Arc<dyn Buffer>)
    });
    let (_stop, ctx) = stream_ctx();
    assert!(reg.build_buffer(&conf(json!({"none": {}})), &ctx).unwrap().is_none());
    assert!(reg.build_buffer(&conf(json!({"memory": {}})), &ctx).unwrap().is_some());
    let err = reg.build_buffer(&conf(json!({"disk": {}})), &ctx).err().unwrap();
    assert_eq!(err.to_string(), "type 'disk' was not recognised as a buffer");
    // A registered constructor's own refusal comes back unchanged.
    let err = reg
        .build_buffer(&conf(json!({"memory": {"limit": 0}})), &ctx)
        .err()
        .unwrap();
    assert!(err.to_string().contains("limit must be above 0"));
}

// ---------- paging ----------

fn procs_conf(v: serde_json::Value) -> Vec<ComponentConfig> {
    v.as_array().unwrap().iter().map(|c| conf(c.clone())).collect()
}

#[test]
fn page_cuts_stop_at_batch_wide_steps_and_restart_after_a_split() {
    let cuts = page_cuts(&procs_conf(json!([
        {"mapping": "root = this"},
        {"archive": {"format": "lines"}},
        {"split": {"size": 1}},
        {"mapping": "root = batch_size()"},
        {"jq": {"query": "."}}
    ])));
    // Before `archive` the batch stays whole; `split` regroups; `batch_size()` is batch-wide.
    assert_eq!(cuts, [false, true, false, true, true]);
    // `cuts[0]` of `[noop, X]` says whether X is page safe.
    let safe = |x: serde_json::Value| page_cuts(&procs_conf(json!([{"noop": {}}, x])))[0];
    for ok in [
        json!({"bloblang": "root = this"}),
        json!({"try": [{"mapping": "root = this"}]}),
        json!({"for_each": [{"noop": {}}]}),
        json!({"catch": [{"noop": {}}]}),
        json!({"http": {"url": "http://x"}}),
        json!({"branch": {"request_map": "root = this", "processors": [{"jq": {"query": "."}}]}}),
        json!({"branch": {}}),
        json!({"switch": [{"check": "this.a", "processors": [{"noop": {}}]}, {"processors": [{"log": {}}]}]}),
        json!({"processors": [{"label": "x", "log": {"message": "hi"}}]}),
    ] {
        assert!(safe(ok.clone()), "{ok}");
    }
    for refused in [
        json!({"http": {"url": "http://x", "batch_as_multipart": true}}),
        json!({"http": {"url": "http://x/${! batch_index() }"}}),
        json!({"branch": {"result_map": "root = from_all(\"x\")"}}),
        json!({"branch": {"processors": "not a list"}}),
        json!({"branch": {"processors": [{"split": {}}]}}),
        json!({"switch": [{"check": "batch_index() == 0"}]}),
        json!({"switch": [{"processors": [{"archive": {}}]}]}),
        json!({"switch": {"not": "a list"}}),
        json!({"processors": [{"split": {}}]}),
        json!({"processors": [{"a": {}, "b": {}}]}),
        json!({"processors": [{"label": "only"}]}),
        json!({"processors": ["not an object"]}),
        json!({"mapping": "root = batch_size()"}),
        json!({"mystery": {}}),
        json!({"mapping": "root = this", "processors": [{"noop": {}}]}),
    ] {
        assert!(!safe(refused.clone()), "{refused}");
    }
}

#[test]
fn a_paging_of_zero_rows_holds_one_message_per_page() {
    let p = Paging::new(&procs_conf(json!([{"noop": {}}])), 0);
    assert_eq!(p.rows, 1);
    assert_eq!(p.cuts, [true]);
}

#[tokio::test]
async fn paged_execution_emits_pages_in_order_and_makes_them_one_at_a_time() {
    let (_stop, ctx) = proc_ctx();
    let made = Arc::new(AtomicUsize::new(0));
    let procs: Vec<Arc<dyn Processor>> = vec![
        Arc::new(Expand {
            n: 3,
            made: made.clone(),
        }),
        Arc::new(Bang),
    ];
    let paging = Paging {
        rows: 2,
        cuts: vec![true, true],
    };
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let sink = emitted.clone();
    let probe = made.clone();
    execute_paged(&ctx, &procs, parts(&["a", "b"]), &paging, |b| {
        let sink = sink.clone();
        let seen = probe.load(Ordering::SeqCst);
        async move {
            sink.lock().unwrap().push((seen, texts(&b)));
            Ok(())
        }
    })
    .await
    .unwrap();
    let got = emitted.lock().unwrap().clone();
    // Six messages in pages of two; page k exists only once page k-1 was written.
    assert_eq!(
        got,
        [
            (1, vec!["a!".to_string(), "a!".into()]),
            (2, vec!["a!".into(), "b!".into()]),
            (3, vec!["b!".into(), "b!".into()]),
        ]
    );
}

#[tokio::test]
async fn paged_execution_cuts_a_processor_without_pages_after_it_ran() {
    let (_stop, ctx) = proc_ctx();
    let procs: Vec<Arc<dyn Processor>> = vec![Arc::new(Bang)];
    let paging = Paging {
        rows: 2,
        cuts: vec![true],
    };
    let emitted = Arc::new(Mutex::new(Vec::new()));
    let sink = emitted.clone();
    execute_paged(&ctx, &procs, parts(&["1", "2", "3", "4", "5"]), &paging, |b| {
        let sink = sink.clone();
        async move {
            sink.lock().unwrap().push(b.len());
            Ok(())
        }
    })
    .await
    .unwrap();
    assert_eq!(*emitted.lock().unwrap(), [2, 2, 1]);
    // Not cut: the batch stays whole.
    let whole = Paging {
        rows: 2,
        cuts: vec![false],
    };
    let sink = emitted.clone();
    emitted.lock().unwrap().clear();
    execute_paged(&ctx, &procs, parts(&["1", "2", "3"]), &whole, |b| {
        let sink = sink.clone();
        async move {
            sink.lock().unwrap().push(b.len());
            Ok(())
        }
    })
    .await
    .unwrap();
    assert_eq!(*emitted.lock().unwrap(), [3]);
}

#[tokio::test]
async fn paged_execution_stops_on_a_failure_shutdown_or_refused_emit() {
    let (_stop, ctx) = proc_ctx();
    let paging = Paging {
        rows: 10,
        cuts: vec![],
    };
    let failing: Vec<Arc<dyn Processor>> = vec![Arc::new(Boom)];
    let err = execute_paged(&ctx, &failing, parts(&["a"]), &paging, |_| async { Ok(()) })
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), CoreError::other("boom").to_string());
    let refused = execute_paged(&ctx, &[], parts(&["a"]), &paging, |_| async {
        Err(CoreError::other("sink down"))
    })
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("sink down"));
    let (stop, ctx) = proc_ctx();
    stop.send(true).unwrap();
    let procs: Vec<Arc<dyn Processor>> = vec![Arc::new(Bang)];
    let closed = execute_paged(&ctx, &procs, parts(&["a"]), &paging, |_| async { Ok(()) }).await;
    assert!(matches!(closed, Err(CoreError::ComponentClosed)));
}

// ---------- try / catch executors ----------

fn errored(text: &str) -> Part {
    let mut p = Part::from_str(text);
    p.set_error_msg("earlier failure");
    p
}

#[tokio::test]
async fn try_all_skips_failed_batches_and_stops_once_nothing_is_left() {
    let (_stop, ctx) = proc_ctx();
    let procs: Vec<Arc<dyn Processor>> = vec![Arc::new(Bang), Arc::new(Bang)];
    let out = execute_try_all(&ctx, &procs, vec![parts(&["ok"]), vec![errored("bad")]])
        .await
        .unwrap();
    assert_eq!(out.iter().map(|b| texts(b)).collect::<Vec<_>>(), [vec!["ok!!".to_string()], vec!["bad".into()]]);
    assert!(execute_try_all(&ctx, &procs, vec![]).await.unwrap().is_empty());
    let failing: Vec<Arc<dyn Processor>> = vec![Arc::new(Boom)];
    assert!(execute_try_all(&ctx, &failing, vec![parts(&["x"])]).await.is_err());
    let (stop, ctx) = proc_ctx();
    stop.send(true).unwrap();
    let closed = execute_try_all(&ctx, &procs, vec![parts(&["x"])]).await;
    assert!(matches!(closed, Err(CoreError::ComponentClosed)));
}

#[tokio::test]
async fn catch_all_runs_only_on_batches_that_entered_failed() {
    let (_stop, ctx) = proc_ctx();
    let procs: Vec<Arc<dyn Processor>> = vec![Arc::new(Bang)];
    let out = execute_catch_all(&ctx, &procs, vec![parts(&["ok"]), vec![errored("bad")], vec![]])
        .await
        .unwrap();
    assert_eq!(out.iter().map(|b| texts(b)).collect::<Vec<_>>(), [vec!["ok".to_string()], vec!["bad!".into()], vec![]]);
    let failing: Vec<Arc<dyn Processor>> = vec![Arc::new(Boom)];
    assert!(execute_catch_all(&ctx, &failing, vec![vec![errored("bad")]]).await.is_err());
    let (stop, ctx) = proc_ctx();
    stop.send(true).unwrap();
    let closed = execute_catch_all(&ctx, &procs, vec![vec![errored("bad")]]).await;
    assert!(matches!(closed, Err(CoreError::ComponentClosed)));
}

#[test]
fn metric_paths_are_rooted_once() {
    assert_eq!(metric_path("root"), "root");
    assert_eq!(metric_path("root.pipeline"), "root.pipeline");
    assert_eq!(metric_path(""), "root");
    assert_eq!(metric_path("pipeline.processors.0"), "root.pipeline.processors.0");
}

#[tokio::test]
async fn an_observed_processor_counts_pages_and_refuses_paging_over_the_memory_quota() {
    let (_stop, sctx, exporter) = observed_ctx(Arc::new(Registry::new()));
    let ctx = sctx.to_proc_context("unarchive", "fan", "pipeline.processors.0");
    let made = Arc::new(AtomicUsize::new(0));
    let observed = ObservedProcessor::new(
        Arc::new(Expand {
            n: 2,
            made: made.clone(),
        }),
        ctx.clone(),
    );
    let pages: Vec<Batch> = observed.pages(&ctx, parts(&["a", "b"]), 3).ok().unwrap().collect();
    assert_eq!(pages.iter().map(Vec::len).collect::<Vec<_>>(), [3, 1]);
    let page = exporter.render(&|_| true);
    let line = |name: &str| {
        page.lines()
            .find(|l| l.starts_with(name) && l.contains("label=\"fan\""))
            .map(|l| l.rsplit(' ').next().unwrap().to_string())
    };
    assert_eq!(line("processor_received").as_deref(), Some("2"));
    assert_eq!(line("processor_batch_received").as_deref(), Some("1"));
    assert_eq!(line("processor_sent").as_deref(), Some("4"));
    assert_eq!(line("processor_batch_sent").as_deref(), Some("2"));
    // A processor without pages hands the batch back.
    let plain = ObservedProcessor::new(Arc::new(Bang), ctx.clone());
    assert_eq!(plain.pages(&ctx, parts(&["a"]), 3).err().map(|b| b.len()), Some(1));
    // Over the organization's memory quota, the batch goes back to be refused by process_batch.
    ctx.org.set_quotas(OrgQuotas {
        max_memory_bytes_per_run: 1,
        ..OrgQuotas::default()
    });
    assert!(observed.pages(&ctx, parts(&["abc"]), 3).is_err());
    assert_eq!(made.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_split_batch_nack_maps_only_tracked_messages_back_to_the_source() {
    /// Splits every message into its own batch and adds one message the input never sent.
    struct SplitAndAdd;
    #[async_trait]
    impl Processor for SplitAndAdd {
        fn processor_type(&self) -> &'static str {
            "split_add"
        }
        async fn process_batch(&self, _: &ProcContext, b: Batch) -> Result<Vec<Batch>, CoreError> {
            let mut out: Vec<Batch> = b.into_iter().map(|p| vec![p]).collect();
            out.push(vec![Part::from_str("new")]);
            Ok(out)
        }
    }
    let (_stop, ctx) = proc_ctx();
    let pool = ekiben_core::PipelineWorkerPool::new(1, vec![Arc::new(SplitAndAdd)], ctx);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(4);
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(8);
    let run = tokio::spawn(async move { pool.run(in_rx, out_tx).await });
    let (ack, result) = AckResponder::channel();
    in_tx.send(Transaction::new(parts(&["a", "b"]), ack)).await.unwrap();
    drop(in_tx);
    let mut got = Vec::new();
    while let Some(tx) = out_rx.recv().await {
        got.push(tx);
    }
    assert_eq!(got.len(), 3);
    for tx in got {
        let body = texts(&tx.payload);
        if body == ["new"] {
            tx.ack.nack(BatchError::new(tx.payload, "rejected"));
        } else {
            tx.ack.ack();
        }
    }
    let err = result.await.unwrap().expect("the source batch is nacked");
    assert_eq!(err.message, "rejected");
    // The added message has no index in the source batch, so no message is marked failed.
    assert_eq!(err.indexed_errors(), 0);
    run.await.unwrap().unwrap();
}

// ---------- batched output ----------

#[tokio::test]
async fn a_batched_output_passes_identity_and_connection_through() {
    let (_stop, sctx) = stream_ctx();
    let inner = Arc::new(Collect::default());
    let policy = BatchPolicyConfig::from_json_value(&json!({"count": 2})).unwrap();
    let batcher = Batcher::new(&policy, sctx.to_proc_context("collect", "", "output.batching")).unwrap();
    let out = BatchedOutput::new(inner.clone(), batcher);
    assert_eq!((out.output_type(), out.label()), ("collect", "sink"));
    out.connect(&sctx).await.unwrap();
    assert_eq!(inner.connects.load(Ordering::SeqCst), 1);
    assert!(out.is_connected());
    // Closing with nothing waiting writes nothing.
    out.close().await.unwrap();
    assert!(inner.got.lock().unwrap().is_empty());
}

// ---------- org ----------

fn target(prefixes: &[&str]) -> HostTarget {
    HostTarget {
        addr: "127.0.0.1:8080".parse().unwrap(),
        path_prefixes: prefixes.iter().map(|s| (*s).to_string()).collect(),
        late: None,
    }
}

fn url(s: &str) -> url::Url {
    url::Url::parse(s).unwrap()
}

#[test]
fn a_host_target_admits_only_plain_http_to_its_address_under_whole_prefix_segments() {
    let t = target(&["/broker/", "/x"]);
    assert!(t.admits(&url("http://127.0.0.1:8080/broker")));
    assert!(t.admits(&url("http://127.0.0.1:8080/broker/ngsi-ld/v1")));
    assert!(t.admits(&url("http://127.0.0.1:8080/x/y")));
    for refused in [
        "http://127.0.0.1:8080/brokerx",
        "http://127.0.0.1:8080/",
        "https://127.0.0.1:8080/broker",
        "http://user@127.0.0.1:8080/broker",
        "http://user:pw@127.0.0.1:8080/broker",
        "http://localhost:8080/broker",
        "http://127.0.0.2:8080/broker",
        "http://127.0.0.1:8081/broker",
        "http://127.0.0.1/broker",
        "http://[::1]:8080/broker",
        "http://127.0.0.1:8080/broker/%2e%2e/admin",
        "http://127.0.0.1:8080/broker/a%2Fb",
        "http://127.0.0.1:8080/broker/a%5cb",
    ] {
        assert!(!t.admits(&url(refused)), "{refused}");
    }
    // An empty or relative prefix admits nothing.
    let odd = target(&["", "/", "broker"]);
    assert!(!odd.admits(&url("http://127.0.0.1:8080/broker")));
    // An IPv6 listener matches its own literal.
    let v6 = HostTarget {
        addr: "[::1]:9000".parse().unwrap(),
        path_prefixes: vec!["/p".into()],
        late: None,
    };
    assert!(v6.admits(&url("http://[::1]:9000/p/q")));
}

#[test]
fn a_late_listener_admits_nothing_until_its_address_is_known() {
    let cell = Arc::new(OnceLock::new());
    let t = HostTarget {
        late: Some((cell.clone(), vec!["/internal".into()])),
        ..target(&["/broker"])
    };
    let late_url = url("http://127.0.0.1:9100/internal/report");
    assert!(!t.admits(&late_url));
    cell.set("127.0.0.1:9100".parse().unwrap()).unwrap();
    assert!(t.admits(&late_url));
    // The late listener's prefixes do not open the main listener, nor the reverse.
    assert!(!t.admits(&url("http://127.0.0.1:8080/internal/report")));
    assert!(!t.admits(&url("http://127.0.0.1:9100/broker")));
    let resources = ResourceManager::new().with_host_target(t.clone());
    let org = resources.get_or_create_org(&OrgId::new("o"));
    assert_eq!(org.host_target(), Some(&t));
    assert_eq!(OrgContext::new(OrgId::new("p"), OrgQuotas::default()).host_target(), None);
}

#[test]
fn scoped_secrets_reach_only_streams_of_their_scope_and_the_org_wins_a_shared_name() {
    let org = OrgContext::new(OrgId::new("o"), OrgQuotas::default());
    org.set_secret("SHARED", "org");
    org.set_secret("ORG_ONLY", "o");
    org.replace_scoped_secrets(HashMap::from([
        (
            "team".to_string(),
            HashMap::from([("SHARED".to_string(), "team".to_string()), ("TEAM_ONLY".to_string(), "t".to_string())]),
        ),
        ("other".to_string(), HashMap::from([("OTHER".to_string(), "x".to_string())])),
    ]));
    let team = org.secrets_snapshot_for("team.ingest");
    assert_eq!(team.get("SHARED").map(String::as_str), Some("org"));
    assert_eq!(team.get("TEAM_ONLY").map(String::as_str), Some("t"));
    assert!(!team.contains_key("OTHER"));
    // A stream id without a scope, or of an unknown scope, sees the organization's alone.
    assert_eq!(org.secrets_snapshot_for("plain"), org.secrets_snapshot());
    assert_eq!(org.secrets_snapshot_for("nobody.s"), org.secrets_snapshot());
    assert_eq!(org.resolve_scoped_secret("team", "SHARED").as_deref(), Some("team"));
    assert_eq!(org.resolve_scoped_secret("team", "ORG_ONLY"), None);
    assert_eq!(org.resolve_scoped_secret("ghost", "SHARED"), None);
    // Replacing the scopes drops a scope left out.
    org.replace_scoped_secrets(HashMap::new());
    assert_eq!(org.resolve_scoped_secret("team", "TEAM_ONLY"), None);
    assert_eq!(ekiben_core::org::stream_scope("a.b.c"), Some("a"));
    assert_eq!(ekiben_core::org::stream_scope("abc"), None);
}

#[test]
fn an_exact_http_host_allow_list_entry_matches_only_that_host() {
    let org = OrgContext::new(
        OrgId::new("o"),
        OrgQuotas {
            allowed_http_hosts: Some(vec!["api.example.org".into(), "*.data.example".into()]),
            ..OrgQuotas::default()
        },
    );
    assert!(org.check_http_host("API.example.org").is_ok());
    assert!(org.check_http_host("x.data.example").is_ok());
    assert!(org.check_http_host("data.example").is_ok());
    for refused in ["evil.api.example.org", "example.org", "api.example.org.evil"] {
        assert!(org.check_http_host(refused).is_err(), "{refused}");
    }
}

#[tokio::test]
async fn http_requests_are_paced_by_the_org_rate() {
    let org = OrgContext::new(
        OrgId::new("o"),
        OrgQuotas {
            max_http_requests_per_sec: 20,
            ..OrgQuotas::default()
        },
    );
    let started = tokio::time::Instant::now();
    for _ in 0..40 {
        org.admit_http_request().await;
    }
    // 40 requests at 20 a second: the burst passes at once, the rest wait about a second.
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(500), "{took:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
}

// ---------- config ----------

#[test]
fn ambiguous_component_keys_stay_ambiguous_unless_one_is_known() {
    let known = |k: &str| k == "kafka" || k == "amqp";
    let v = json!({"kafka": {}, "amqp": {}});
    // Two known candidates: the type is not guessed.
    assert!(ComponentConfig::from_json_value(&v, "noop", Some(&known)).is_err());
    let one = ComponentConfig::from_json_value(&json!({"kafka": {}, "typo": {}}), "noop", Some(&known)).unwrap();
    assert_eq!(one.component_type, "kafka");
}

#[test]
fn backoff_reads_every_interval_and_refuses_a_bad_duration() {
    let b = BackoffConfig::from_json_value(
        &json!({"initial_interval": "1s", "max_interval": "4s", "max_elapsed_time": "1m"}),
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(
        (b.initial_interval, b.max_interval, b.max_elapsed_time),
        (Duration::from_secs(1), Duration::from_secs(4), Duration::from_secs(60))
    );
    for key in ["initial_interval", "max_interval", "max_elapsed_time"] {
        assert!(BackoffConfig::from_json_value(&json!({ key: "soon" }), Duration::ZERO).is_err(), "{key}");
    }
    let d = BackoffConfig::from_json_value(&json!("not an object"), Duration::from_secs(9)).unwrap();
    assert_eq!(d.max_elapsed_time, Duration::from_secs(9));
}

#[test]
fn a_stream_without_pipeline_processors_and_a_filter_without_prefixes_parse() {
    let s = StreamConfig::from_yaml_or_json("input: {generate: {}}\npipeline: {threads: 2}\noutput: {drop: {}}\n").unwrap();
    assert_eq!(s.pipeline.threads, 2);
    assert!(s.pipeline.processors.is_empty());
    let f = extract_metadata_filter(&json!({"include_patterns": ["^k", "("]}));
    assert!(f.matches("key"));
    assert!(!f.matches("other"));
    // An invalid pattern alone is skipped: the filter then matches nothing it names.
    assert!(!extract_metadata_filter(&json!({"include_patterns": ["("]})).matches("key"));
}

// ---------- interpolation ----------

#[test]
fn interpolation_keeps_bloblang_and_escapes_and_lists_each_missing_name_once() {
    let none = |_: &str| None::<String>;
    assert_eq!(interpolate_env_str("${{!meta(\"k\")}}", none).unwrap(), "${{!meta(\"k\")}}");
    // Unterminated forms pass through unchanged.
    assert_eq!(interpolate_env_str("${{!open", none).unwrap(), "${{!open");
    assert_eq!(interpolate_env_str("${{open", none).unwrap(), "${{open");
    assert_eq!(interpolate_env_str("${open", none).unwrap(), "${open");
    // Quotes, escaped quotes and nested braces inside a Bloblang query.
    let q = r#"${! "a\"}b" + {"k": 1}.k }"#;
    assert_eq!(interpolate_env_str(q, none).unwrap(), q);
    assert_eq!(interpolate_env_str(r#"${! "a\\" }"#, none).unwrap(), r#"${! "a\\" }"#);
    // Names that are not environment variable names are left as text.
    assert_eq!(interpolate_env_str("${} ${1A} ${a-b}", none).unwrap(), "${} ${1A} ${a-b}");
    let err = interpolate_env_str("${MISSING} and ${MISSING} and ${OTHER}", none).unwrap_err();
    assert_eq!(err.missing, ["MISSING", "OTHER"]);
    assert_eq!(err.best_attempt, " and  and ");
    assert_eq!(interpolate_env_str("${X:def}", none).unwrap(), "def");
    assert_eq!(interpolate_env_str("é${X}ü", |_| Some("1".into())).unwrap(), "é1ü");
}

// ---------- redaction ----------

#[test]
fn urls_that_do_not_parse_still_lose_their_password() {
    assert_eq!(redact_url_str("https://host/path"), "https://host/path");
    assert_eq!(redact_url_str("${SCHEME}://user:secret@host/db"), "${SCHEME}://user:REDACTED@host/db");
    // A password holding `@` loses all of it, as Go splits userinfo at the last `@`.
    assert_eq!(redact_url_str("${S}://u:p@ss@host"), "${S}://u:REDACTED@host");
    assert_eq!(redact_url_str("${S}://token@host"), "${S}://token@host");
    assert_eq!(redact_url_str("${S}://host/a@b"), "${S}://host/a@b");
    assert_eq!(redact_url_str("no scheme at all"), "no scheme at all");
}

#[test]
fn secret_fields_redact_strings_keep_empties_and_walk_nested_values() {
    let out = redact_config_value(&json!({
        "password": "",
        "token": {"inner": "https://u:pw@h/"},
        "secret": 42,
        "list": [true, null, 1.5],
    }));
    assert_eq!(out["password"], "");
    assert_eq!(out["token"]["inner"], "https://u:REDACTED@h/");
    assert_eq!(out["secret"], 42);
    assert_eq!(out["list"], json!([true, null, 1.5]));
}

// ---------- lint ----------

#[test]
fn lint_fields_lists_unknown_and_missing_fields() {
    let errs = lint_fields(&json!({"url": "x", "typo": 1}), &["url", "topic"], &["topic"]);
    assert_eq!(errs, ["field typo not recognised", "field topic is required"]);
    assert!(lint_fields(&json!({"url": "x"}), &["url"], &[]).is_empty());
    assert_eq!(lint_fields(&json!("x"), &[], &[]), ["expected object value"]);
}

#[test]
fn stream_lint_reports_unknown_kinds_registered_rules_and_nested_processors() {
    let reg = Registry::new();
    reg.register_input("generate", |_, _| unreachable_input());
    reg.register_output("drop", |_, _| Ok(Arc::new(Collect::default()) as Arc<dyn Output>));
    for name in ["mapping", "branch", "catch", "try", "noop"] {
        reg.register_processor(name, |_, _| Ok(Arc::new(Bang) as Arc<dyn Processor>));
    }
    reg.register_lint("input", "generate", |c| lint_fields(c, &["mapping"], &[]));
    reg.register_lint("output", "drop", |c| lint_fields(c, &[], &[]));
    reg.register_lint("processor", "noop", |c| lint_fields(c, &[], &[]));
    let yaml = r#"
input:
  generate: {typo: 1}
  processors:
    - noop: {extra: 1}
buffer:
  disk: {}
pipeline:
  processors:
    - mapping: "root = "
    - mapping: "root = this"
    - branch: {request_map: ""}
    - branch: "not an object"
    - catch: {not: a list}
    - try: {not: a list}
    - branch:
        request_map: "root = this"
        result_map: "root = ("
        processors:
          - noop: {}
          - 42
    - catch:
        - noop: {}
        - 7
    - try:
        - noop: {}
        - "bad"
    - mapping: {not: a string}
output:
  drop: {bad: 1}
  processors:
    - noop: {also: 1}
"#;
    let conf = StreamConfig::from_yaml_or_json(yaml).unwrap();
    let report = reg.lint_stream_config(&conf, Vec::new());
    let text = format!("{report:?}");
    for want in [
        "input generate: field typo not recognised",
        "processor noop: field extra not recognised",
        "buffer: type 'disk' was not recognised",
        "output drop: field bad not recognised",
        "processor noop: field also not recognised",
        "branch.result_map",
        "requires a string mapping",
    ] {
        assert!(text.contains(want), "{want} missing from {text}");
    }
    let unknown = StreamConfig::from_yaml_or_json("input: {nope: {}}\noutput: {drop: \"x\"}\n").unwrap();
    let text = format!("{:?}", reg.lint_stream_config(&unknown, Vec::new()));
    assert!(text.contains("input: type 'nope' was not recognised"), "{text}");
}

fn unreachable_input() -> Result<Box<dyn Input>, CoreError> {
    Err(CoreError::config("lint never builds an input"))
}

#[tokio::test]
async fn children_built_by_composite_outputs_get_bento_paths() {
    let reg = Arc::new(Registry::new());
    reg.register_output("collect", |_, _| Ok(Arc::new(Collect::default()) as Arc<dyn Output>));
    for kind in ["fallback", "switch", "retry", "broker"] {
        reg.register_output(kind, |c, ctx| {
            let child = conf(json!({"collect": {}}));
            let n = if c.component_type == "retry" { 1 } else { 2 };
            for _ in 0..n {
                ctx.registry.build_output(&child, ctx)?;
            }
            Ok(Arc::new(Collect::default()) as Arc<dyn Output>)
        });
    }
    reg.register_input("collect_in", |_, _| {
        Ok(Box::new(Script {
            answers: VecDeque::new(),
            acks: AckLog::default(),
            closed: Arc::default(),
        }) as Box<dyn Input>)
    });
    for kind in ["read_until", "broker_in"] {
        reg.register_input(kind, |_, ctx| {
            let child = conf(json!({"collect_in": {}}));
            ctx.registry.build_input(&child, ctx)?;
            ctx.registry.build_input(&child, ctx)
        });
    }
    let (_stop, ctx, exporter) = observed_ctx(reg.clone());
    for kind in ["fallback", "switch", "retry", "broker"] {
        reg.build_output(&conf(json!({ kind: {} })), &ctx).unwrap();
    }
    for kind in ["read_until", "broker_in"] {
        reg.build_input(&conf(json!({ kind: {} })), &ctx).unwrap();
    }
    // Each observed child registers its counters under its own Bento path.
    let page = exporter.render(&|_| true);
    for path in [
        "root.output",
        "root.output.fallback.0",
        "root.output.fallback.1",
        "root.output.switch.cases.0.output",
        "root.output.switch.cases.1.output",
        "root.output.retry.output",
        "root.output.broker.outputs.1",
        "root.input.read_until.input",
        "root.input.broker_in.inputs.0",
        "root.input.broker_in.inputs.1",
    ] {
        assert!(page.contains(&format!("path=\"{path}\"")), "{path} missing");
    }
}

// ---------- messages ----------

#[test]
fn a_part_moves_between_bytes_and_structured_values() {
    let mut p = Part::from_structured(Value::String("txt".into()));
    assert!(p.holds_structured());
    assert_eq!(&p.as_bytes()[..], b"\"txt\"");
    p.set_structured(Value::Bytes(b"raw".to_vec()));
    assert!(!p.holds_structured());
    assert_eq!(&p.as_bytes_ref()[..], b"raw");
    p.set_structured(Value::String("s".into()));
    assert!(p.holds_structured());
    assert_eq!(p.as_structured().unwrap(), Value::String("s".into()));
    // A structured value set to bytes through `as_structured_mut` reads as those bytes; once
    // empty, the cached empty bytes do not hide the structured value.
    let mut q = Part::from_str("{}");
    *q.as_structured_mut().unwrap() = Value::Bytes(b"xy".to_vec());
    assert_eq!(&q.as_bytes()[..], b"xy");
    let mut e = Part::from_str("{}");
    *e.as_structured_mut().unwrap() = Value::Bytes(Vec::new());
    assert!(e.as_bytes().is_empty());
    assert!(e.as_bytes().is_empty());
    assert!(e.as_bytes_ref().is_empty());
    assert_eq!(e.size_estimate(), 2);
    assert!(PartsView(std::slice::from_ref(&e)).part_bytes(0).is_empty());
    assert_eq!(Part::from_str("[1]").as_structured().unwrap(), Part::from_str("[1]").as_structured_ref().unwrap());
    let mut twice = Part::from_str("5");
    twice.as_structured().unwrap();
    assert_eq!(twice.as_structured().unwrap(), Value::Int(5));
    let mut mapped = Part::from_str("old");
    mapped.apply_mapped_value(Value::Nothing);
    assert_eq!(&mapped.as_bytes()[..], b"old");
    mapped.apply_mapped_value(Value::String("new".into()));
    assert_eq!(&mapped.as_bytes()[..], b"new");
    mapped.apply_mapped_value(Value::Bytes(b"b".to_vec()));
    assert_eq!(&mapped.as_bytes()[..], b"b");
    assert!(!mapped.holds_structured());
    let mut v = Part::from_str("{\"a\":1}");
    v.as_structured_mut().unwrap();
    v.as_structured_mut().unwrap();
    assert!(v.holds_structured());
}

#[test]
fn structured_reads_refuse_empty_missing_and_multi_document_parts() {
    assert_eq!(Part::new(Vec::<u8>::new()).as_structured_ref().unwrap_err(), "target message part does not exist");
    assert_eq!(Part::from_str("   ").as_structured_ref().unwrap_err(), "EOF");
    assert_eq!(Part::from_str("1 2").as_structured_ref().unwrap_err(), "message contains multiple valid documents");
    assert!(Part::from_str("1 }").as_structured_ref().is_err());
    assert!(Part::from_str("{").as_structured_ref().is_err());
    assert_eq!(Part::from_str("[1]").as_structured_ref().unwrap(), Value::Array(vec![Value::Int(1)]));
}

#[test]
fn size_estimates_cover_every_value_kind() {
    let size = |v: Value| Part::from_structured(v).size_estimate();
    assert_eq!(size(Value::Uint(0)), 1);
    assert_eq!(size(Value::Uint(12345)), 5);
    assert_eq!(size(Value::Number("1.50".into())), 4);
    assert_eq!(size(Value::Bytes(b"abc".to_vec())), 5);
    assert_eq!(size(Value::String("abc".into())), 5);
    assert_eq!(size(Value::Float(1.0)), 24);
}

#[test]
fn a_parts_view_reads_structured_parts_as_bytes_and_out_of_range_as_empty() {
    let batch = vec![Part::from_structured(Value::Int(3)), Part::from_str("x")];
    let view = PartsView(&batch);
    assert_eq!(&view.part_bytes(0)[..], b"3");
    assert_eq!(&view.part_bytes(1)[..], b"x");
    assert!(view.part_bytes(9).is_empty());
}

#[test]
fn malformed_wire_batches_are_refused() {
    let good = ekiben_core::serialize_bytes(&[b"ab", b""]);
    assert_eq!(deserialize_bytes(&good).unwrap(), [b"ab".to_vec(), Vec::new()]);
    for bad in [
        &[0u8, 0, 1][..],
        &[0, 0, 0, 9, 0][..],
        &[0, 0, 0, 2, 0, 0, 0, 1, b'a', 0][..],
        &[0, 0, 0, 1, 0, 0, 0, 5, b'a'][..],
        &[0xff, 0xff, 0xff, 0xff][..],
    ] {
        let err = deserialize_bytes(bad).unwrap_err();
        assert!(err.to_string().contains("unexpected format"), "{bad:?}");
    }
}

// ---------- errors and transactions ----------

#[test]
fn walking_a_batch_error_skips_foreign_parts_and_stops_when_asked() {
    let (group, source) = SortGroup::new(parts(&["a", "b", "c"]));
    let mut err = BatchError::new(vec![source[0].clone(), Part::from_str("foreign"), source[2].clone()], "all failed");
    let mut seen = Vec::new();
    err.walk_parts_by_source(&group, &source, |i, _, e| {
        seen.push((i, e.map(String::from)));
        true
    });
    // No per-part errors: each part carries the batch message.
    assert_eq!(seen, [(0, Some("all failed".to_string())), (2, Some("all failed".into()))]);
    err.failed(0, "first").failed(99, "out of range");
    assert_eq!(err.indexed_errors(), 1);
    let mut first_only = Vec::new();
    err.walk_parts_by_source(&group, &source, |i, _, e| {
        first_only.push((i, e.map(String::from)));
        false
    });
    assert_eq!(first_only, [(0, Some("first".to_string()))]);
    // A source batch shorter than the group's indexes skips the parts beyond it.
    let mut short = Vec::new();
    err.walk_parts_by_source(&group, &source[..1], |i, _, _| {
        short.push(i);
        true
    });
    assert_eq!(short, [0]);
}

#[test]
fn a_tracked_transaction_resolves_by_its_error_kind() {
    let log = AckLog::default();
    TrackedTransaction::new(parts(&["a"]), logged(&log)).resolve(None);
    TrackedTransaction::new(parts(&["a"]), logged(&log)).resolve(Some(BatchError::new(parts(&["a"]), "whole")));
    // Per-part errors naming no message of this transaction nack it with the batch error.
    let mut foreign = BatchError::new(parts(&["z", "y"]), "foreign");
    foreign.failed(1, "y failed");
    TrackedTransaction::new(parts(&["a"]), logged(&log)).resolve(Some(foreign));
    assert_eq!(*log.lock().unwrap(), [None, Some("whole".to_string()), Some("foreign".into())]);
}

#[test]
fn a_combined_acker_keeps_the_first_error_and_answers_once() {
    let log = AckLog::default();
    let acker = CombinedAcker::new(logged(&log));
    let (a, b, c) = (acker.derive(), acker.derive(), acker.derive());
    a.nack(BatchError::new(parts(&["1"]), "first"));
    b.nack(BatchError::new(parts(&["2"]), "second"));
    assert!(log.lock().unwrap().is_empty());
    c.ack();
    assert_eq!(*log.lock().unwrap(), [Some("first".to_string())]);
    // A responder answered twice is answered once.
    let once = AckLog::default();
    let r = logged(&once);
    r.respond(None);
    assert_eq!(once.lock().unwrap().len(), 1);
}

// ---------- metrics ----------

#[test]
fn stream_metrics_charge_time_and_count_errors_per_processor_path() {
    let exporter = Arc::new(PromExporter::with_defaults());
    let m = StreamMetrics::new("s", "o");
    m.charge_processing(5);
    m.charge_processing(7);
    assert_eq!(m.processing_ns(), 12);
    let c = Namespaced::bare(exporter.clone()).counter("processor_error");
    m.register_processor_error("pipeline.processors.0", c.clone());
    m.incr_processor_error_at("", "pipeline.processors.0", 2);
    m.incr_processor_error_at("lbl", "unregistered", 1);
    assert_eq!(c.value(), 2.0);
    assert_eq!(m.get_counter("processor_error"), 2);
    assert_eq!(m.get_counter("lbl.processor_error"), 1);
    for i in 0..5000 {
        m.incr_counter(&format!("c{i}"), 1);
        m.record_timing(&format!("t{i}"), 1);
    }
    // The local series are capped; a name past the cap is not kept.
    assert_eq!(m.get_counter("c4999"), 0);
    assert_eq!(m.get_counter("c0"), 1);
    let snap = m.snapshot_json(1);
    assert!(snap.to_string().contains("t0"));
}

#[test]
fn timings_keep_a_bounded_window_of_samples() {
    let m = StreamMetrics::new("s", "o");
    for i in 0..3000u64 {
        m.record_timing("lat", i);
    }
    // 2048 samples are kept: the oldest are overwritten by the newest.
    let snap = m.snapshot_json(1);
    let p50 = snap["lat"]["p50"].as_f64().unwrap();
    assert!(p50 > 1500.0, "{snap}");
}

#[test]
fn a_metrics_registry_finds_removes_and_exports_streams() {
    let reg = MetricsRegistry::with_config(MetricsConfig::default()).unwrap();
    let m = StreamMetrics::new("s1", "o1");
    reg.register_stream(m.clone());
    assert!(Arc::ptr_eq(&reg.stream("o1", "s1").unwrap(), &m));
    assert!(reg.stream("o2", "s1").is_none());
    reg.remove_stream("s1");
    assert!(reg.stream("o1", "s1").is_none());
    assert_eq!(reg.exporter().config(), &MetricsConfig::default());
    let bad = MetricsConfig {
        mapping: "root = (".into(),
        ..MetricsConfig::default()
    };
    assert!(MetricsRegistry::with_config(bad).is_err());
}

#[test]
fn metrics_config_refuses_wrong_types_and_reads_string_forms() {
    let ok = MetricsConfig::from_json(&json!({
        "type": "prometheus",
        "max_series": "10",
        "prometheus": {
            "use_histogram_timing": "true",
            "add_process_metrics": "false",
            "histogram_buckets": [" 0.5 ", 0.1],
            "summary_quantiles_objectives": [{"quantile": 0.9}, {"quantile": 0.5}, {"quantile": 0.9}],
            "push_url": null,
            "push_basic_auth": {"username": "", "password": ""}
        }
    }))
    .unwrap();
    assert!(ok.use_histogram_timing && !ok.add_process_metrics);
    assert_eq!(ok.max_series, 10);
    assert_eq!(ok.histogram_buckets, [0.1, 0.5]);
    assert_eq!(ok.summary_quantiles, [0.5, 0.9]);
    assert_eq!(MetricsConfig::from_json(&json!(null)).unwrap(), MetricsConfig::default());
    assert_eq!(MetricsConfig::from_json(&json!({"prometheus": null})).unwrap(), MetricsConfig::default());
    let empty_q = MetricsConfig::from_json(&json!({"prometheus": {"summary_quantiles_objectives": []}})).unwrap();
    assert_eq!(empty_q.summary_quantiles, MetricsConfig::default().summary_quantiles);
    for (bad, want) in [
        (json!("prometheus"), "expected an object"),
        (json!({"type": "statsd"}), "only the prometheus exporter"),
        (json!({"mapping": 3}), "metrics.mapping: expected a string"),
        (json!({"max_series": "many"}), "max_series: expected a number"),
        (json!({"prometheus": []}), "metrics.prometheus: expected an object"),
        (json!({"prometheus": {"use_histogram_timing": "yes"}}), "expected a boolean"),
        (json!({"prometheus": {"push_interval": 5}}), "not supported by ekiben"),
        (json!({"prometheus": {"push_basic_auth": {"username": "u"}}}), "not supported by ekiben"),
        (json!({"prometheus": {"histogram_buckets": "0.1"}}), "expected a list of numbers"),
        (json!({"prometheus": {"histogram_buckets": ["x"]}}), "is not a finite number"),
        (json!({"prometheus": {"summary_quantiles_objectives": [{"quantile": 1.5}]}}), "outside 0..1"),
    ] {
        let err = MetricsConfig::from_json(&bad).unwrap_err().to_string();
        assert!(err.contains(want), "{bad}: {err}");
    }
}

#[test]
fn custom_buckets_and_overflowing_observations_render_in_a_histogram() {
    let conf = MetricsConfig::from_json(&json!({"prometheus": {"use_histogram_timing": true, "histogram_buckets": [0.001]}})).unwrap();
    let exporter = Arc::new(PromExporter::new(conf).unwrap());
    let t = Namespaced::bare(exporter.clone()).timer("lat");
    t.timing(500);
    t.timing(5_000_000_000);
    let page = exporter.render(&|_| true);
    assert!(page.contains("lat_bucket{le=\"0.001\"} 1"), "{page}");
    assert!(page.contains("lat_bucket{le=\"+Inf\"} 2"), "{page}");
    assert!(page.contains("lat_count 2"), "{page}");
}

#[test]
fn a_summary_keeps_its_count_past_the_reservoir() {
    let exporter = Arc::new(PromExporter::with_defaults());
    let t = Namespaced::bare(exporter.clone()).timer("lat");
    for i in 0..(ekiben_core::prometheus::SUMMARY_RESERVOIR_SIZE as u64 + 10) {
        t.timing(i);
    }
    let page = exporter.render(&|_| true);
    assert!(page.contains(&format!("lat_count {}", ekiben_core::prometheus::SUMMARY_RESERVOIR_SIZE + 10)), "{page}");
}

#[test]
fn gauges_counters_and_vectors_ignore_wrong_kinds_and_invalid_input() {
    let exporter = Arc::new(PromExporter::with_defaults());
    let ns = Namespaced::bare(exporter.clone());
    assert!(Arc::ptr_eq(ns.exporter(), &exporter));
    let g = ns.gauge("level");
    g.set_f64(2.5);
    g.incr(3);
    g.decr(1);
    assert_eq!(g.value(), 4.5);
    let noop = Gauge::noop();
    noop.set_f64(1.0);
    noop.incr(1);
    noop.decr(1);
    assert_eq!(noop.value(), 0.0);
    let c = ns.counter("hits");
    c.incr_f64(-1.0);
    c.incr_f64(f64::NAN);
    c.incr_f64(1.5);
    assert_eq!(c.value(), 1.5);
    assert_eq!(ekiben_core::Counter::noop().value(), 0.0);
    // A name taken by another kind gives a no-op handle, before and after the family exists.
    let wrong = ns.gauge("hits");
    wrong.set(9);
    assert_eq!((wrong.value(), c.value()), (0.0, 1.5));
    let clash = ns.timer("level");
    clash.timing(1);
    assert!(!exporter.render(&|_| true).contains("level_count"));
    // Invalid metric and label names are refused.
    assert!(ns.counter("1bad").is_noop());
    assert!(ns.counter("").is_noop());
    assert!(ns.with_labels(&[("9x", "v")]).counter("ok").is_noop());
    assert!(ns.with_labels(&[("", "v")]).counter("ok2").is_noop());
    // A vector of one kind hands out no-op handles of the others and for a wrong label count.
    let cv = ns.counter_vec("req", &["code"]);
    cv.gauge(&["200"]).set(5);
    cv.timer(&["200"]).timing(5);
    assert!(!exporter.render(&|_| true).contains("req_count"));
    assert!(cv.counter(&["200", "extra"]).is_noop());
    let gv = ns.gauge_vec("temp", &["room"]);
    assert!(gv.counter(&["a"]).is_noop());
    let tv = ns.timer_vec("dur", &["op"]);
    assert!(tv.counter(&["a"]).is_noop());
    assert_eq!(tv.gauge(&["a"]).value(), 0.0);
    // A mapping that deletes the name gives a vector without a family.
    let deleting = Arc::new(ekiben_core::MetricMapping::new("root = deleted()").unwrap());
    let dv = ns.with_mapping(deleting).counter_vec("gone", &["x"]);
    assert!(dv.counter(&["1"]).is_noop());
    assert_eq!(escape_label_value("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
}

#[test]
fn process_metrics_render_from_proc_when_asked() {
    let conf = MetricsConfig::from_json(&json!({"prometheus": {"add_process_metrics": true}})).unwrap();
    let exporter = PromExporter::new(conf).unwrap();
    let page = exporter.render(&|_| true);
    for want in ["process_resident_memory_bytes", "process_virtual_memory_bytes", "process_cpu_seconds_total", "process_open_fds"] {
        assert!(page.contains(want), "{want} missing:\n{page}");
    }
    // An organization's view holds no process-wide series.
    assert!(!exporter.render(&|o| o.is_some()).contains("process_"));
}

#[test]
fn the_series_cap_drops_a_new_family_that_does_not_fit() {
    let conf = MetricsConfig {
        max_series: 1,
        ..MetricsConfig::default()
    };
    let exporter = Arc::new(PromExporter::new(conf).unwrap());
    let ns = Namespaced::bare(exporter.clone());
    ns.counter("first").incr(1);
    assert!(ns.counter("second").is_noop());
    assert_eq!(exporter.series_count(), 1);
    let page = exporter.render(&|_| true);
    assert!(!page.contains("second"));
    // A view that sees no series of a family leaves the family out entirely.
    assert!(!exporter.render(&|_| false).contains("first"));
}
