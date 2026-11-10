use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ekiben_core::config::build_blobl_env_for_stream;
use ekiben_core::{
    execute_paged, AckResponder, Batch, BatchError, CoreError, EnvAccess, MetricsRegistry, OrgId,
    Paging, Part, ProcContext, Registry, ResourceManager, RunSummary, SortGroup, StreamConfig,
    StreamContext, StreamMetrics, DEFAULT_PAGE_ROWS,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// A processed result up to this size (estimated JSON bytes) is encoded on its worker; a larger one
/// is encoded batch by batch as the writer writes it.
const ENCODE_AHEAD_BYTES: usize = 8 << 20;

/// Pages a worker may finish ahead of the writer: past them it waits, so a run holds a few pages
/// of results, never its whole source.
const PAGES_AHEAD: usize = 2;

/// Handle to a running resident stream (`internal/stream/type.go` & `internal/stream/manager/type.go`).
pub struct RunningStreamHandle {
    pub stream_id: String,
    pub org_id: OrgId,
    pub created_at: Instant,
    stopped_after_ns: Arc<AtomicU64>,
    input_connected: Arc<AtomicBool>,
    output_connected: Arc<AtomicBool>,
    pub metrics: Arc<StreamMetrics>,
    shutdown_tx: watch::Sender<bool>,
    join_handle: tokio::sync::Mutex<Option<JoinHandle<RunSummary>>>,
    last_summary: Arc<std::sync::Mutex<Option<RunSummary>>>,
}

impl RunningStreamHandle {
    /// Returns true if the stream is currently running (`StreamStatus.IsRunning`).
    pub fn is_running(&self) -> bool {
        self.stopped_after_ns.load(Ordering::Acquire) == 0
    }

    /// Returns true if both input and output are connected (`StreamStatus.IsReady`).
    pub fn is_ready(&self) -> bool {
        self.input_connected.load(Ordering::Acquire)
            && self.output_connected.load(Ordering::Acquire)
    }

    /// Returns the stream's uptime (`StreamStatus.Uptime`).
    /// When a finite stream finishes, `stopped_after_ns` is recorded so `uptime` freezes
    /// exactly as Bento 1.22 does (`PART B trap [T-6140]`).
    pub fn uptime(&self) -> Duration {
        let frozen = self.stopped_after_ns.load(Ordering::Acquire);
        if frozen > 0 {
            Duration::from_nanos(frozen)
        } else {
            self.created_at.elapsed()
        }
    }

    pub fn last_summary(&self) -> Option<RunSummary> {
        self.last_summary.lock().unwrap().clone()
    }

    pub async fn wait_for_completion(&self) -> Option<RunSummary> {
        let handle_opt = self.join_handle.lock().await.take();
        if let Some(h) = handle_opt {
            if let Ok(summary) = h.await {
                *self.last_summary.lock().unwrap() = Some(summary.clone());
                return Some(summary);
            }
        }
        self.last_summary()
    }

    /// Gracefully stops the stream within `timeout`.
    pub async fn stop(&self, timeout: Duration) -> Result<Option<RunSummary>, CoreError> {
        let _ = self.shutdown_tx.send(true);
        let handle_opt = self.join_handle.lock().await.take();
        if let Some(h) = handle_opt {
            match tokio::time::timeout(timeout, h).await {
                Ok(Ok(summary)) => {
                    *self.last_summary.lock().unwrap() = Some(summary.clone());
                    return Ok(Some(summary));
                }
                Ok(Err(e)) => return Err(CoreError::other(e.to_string())),
                Err(_) => {
                    return Err(CoreError::Timeout);
                }
            }
        }
        Ok(self.last_summary())
    }
}

/// Spawns a stream from `conf` and returns its `RunningStreamHandle`.
pub fn spawn_stream(
    stream_id: String,
    conf: StreamConfig,
    registry: Arc<Registry>,
    resources: ResourceManager,
    metrics_reg: Option<&MetricsRegistry>,
    file_root: Option<std::path::PathBuf>,
    env_access: impl Into<EnvAccess>,
) -> Result<Arc<RunningStreamHandle>, CoreError> {
    let org_ctx = resources.get_or_create_org(&conf.org);
    let stream_slot_guard = org_ctx.acquire_stream_slot()?;

    // The stream's variables: its organization's map (`--org-vars`, read at start), its
    // `secret_refs`, then the process environment only as `env_access` allows (E1).
    let mut env_vars: HashMap<String, String> = org_ctx.secrets_snapshot();
    for secret_name in &conf.secret_refs {
        if let Some(val) = org_ctx.resolve_secret(secret_name) {
            env_vars.insert(secret_name.clone(), val);
        }
    }
    let blobl_env = build_blobl_env_for_stream(&env_vars, env_access);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let metrics = StreamMetrics::new(stream_id.clone(), conf.org.0.clone());
    if let Some(mr) = metrics_reg {
        mr.register_stream(metrics.clone());
    }

    let sctx = StreamContext {
        stream_id: stream_id.clone(),
        org: org_ctx.clone(),
        resources: resources.clone(),
        registry: registry.clone(),
        blobl_env: blobl_env.clone(),
        metrics: metrics.clone(),
        label: conf.input.label.clone(),
        path: "stream".to_string(),
        file_root: file_root.clone(),
        env_vars: Arc::new(env_vars),
        shutdown_rx: shutdown_rx.clone(),
    };

    // Register inline resources into the organization's isolated resource namespace
    register_inline_resources(&conf, &registry, &sctx)?;

    let pctx = ProcContext {
        stream_id: stream_id.clone(),
        org: org_ctx.clone(),
        resources: resources.clone(),
        registry: registry.clone(),
        blobl_env,
        metrics: metrics.clone(),
        label: "pipeline".to_string(),
        component_type: "pipeline".to_string(),
        path: "pipeline.processors".to_string(),
        file_root,
        shutdown_rx: shutdown_rx.clone(),
    };

    let input = registry.build_input(&conf.input, &sctx)?;
    let procs = registry.build_processor_list(&conf.pipeline.processors, &pctx)?;
    let output = registry.build_output(&conf.output, &sctx)?;
    let threads = conf.pipeline.threads;
    let paging = Paging::new(
        &conf.pipeline.processors,
        conf.pipeline.page_rows.unwrap_or(DEFAULT_PAGE_ROWS),
    );

    let created_at = Instant::now();
    let stopped_after_ns = Arc::new(AtomicU64::new(0));
    let input_connected = Arc::new(AtomicBool::new(false));
    let output_connected = Arc::new(AtomicBool::new(false));
    let last_summary = Arc::new(std::sync::Mutex::new(None));

    let stopped_clone = stopped_after_ns.clone();
    let in_conn_clone = input_connected.clone();
    let out_conn_clone = output_connected.clone();
    let last_sum_clone = last_summary.clone();
    let sid_clone = stream_id.clone();
    let oid_str = conf.org.0.clone();

    let join_handle = tokio::spawn(async move {
        let _slot_guard = stream_slot_guard;
        let summary = execute_stream_loop(
            sid_clone,
            oid_str,
            created_at,
            input,
            procs,
            output,
            sctx,
            pctx,
            threads,
            paging,
            in_conn_clone,
            out_conn_clone,
        )
        .await;

        let elapsed_ns = created_at.elapsed().as_nanos().max(1) as u64;
        stopped_clone.store(elapsed_ns, Ordering::Release);
        *last_sum_clone.lock().unwrap() = Some(summary.clone());
        summary
    });

    Ok(Arc::new(RunningStreamHandle {
        stream_id,
        org_id: conf.org,
        created_at,
        stopped_after_ns,
        input_connected,
        output_connected,
        metrics,
        shutdown_tx,
        join_handle: tokio::sync::Mutex::new(Some(join_handle)),
        last_summary,
    }))
}

/// Runs a stream once until its input ends, drains all outputs, frees all state, and returns `RunSummary`
/// (`Section 6: ekiben run <config.yaml>`).
pub async fn run_stream_once(
    stream_id: impl Into<String>,
    conf: StreamConfig,
    registry: Arc<Registry>,
    resources: ResourceManager,
    file_root: Option<std::path::PathBuf>,
    env_access: impl Into<EnvAccess>,
) -> Result<RunSummary, CoreError> {
    let handle = spawn_stream(
        stream_id.into(),
        conf,
        registry,
        resources,
        None,
        file_root,
        env_access,
    )?;
    let summary = handle
        .wait_for_completion()
        .await
        .ok_or_else(|| CoreError::other("stream task terminated without summary"))?;
    Ok(summary)
}

pub(crate) fn register_inline_resources(
    conf: &StreamConfig,
    registry: &Registry,
    sctx: &StreamContext,
) -> Result<(), CoreError> {
    for c in &conf.resources.cache_resources {
        if !c.label.is_empty() {
            let cache = registry.build_cache(c, sctx)?;
            sctx.resources
                .insert_cache(&conf.org, c.label.clone(), cache);
        }
    }
    for rl in &conf.resources.rate_limit_resources {
        if !rl.label.is_empty() {
            let rate_limit = registry.build_rate_limit(rl, sctx)?;
            sctx.resources
                .insert_rate_limit(&conf.org, rl.label.clone(), rate_limit);
        }
    }
    for p in &conf.resources.processor_resources {
        if !p.label.is_empty() {
            let pctx = sctx.to_proc_context(
                p.component_type.clone(),
                p.label.clone(),
                "processor_resources",
            );
            let proc = registry.build_processor(p, &pctx, &p.label)?;
            sctx.resources
                .insert_processor(&conf.org, p.label.clone(), proc);
        }
    }
    for i in &conf.resources.input_resources {
        if !i.label.is_empty() {
            let path = format!("root.input_resources.{}", i.label);
            let inp = registry.build_input_at(i, sctx, &path)?;
            sctx.resources.insert_input(&conf.org, i.label.clone(), inp);
        }
    }
    for o in &conf.resources.output_resources {
        if !o.label.is_empty() {
            let path = format!("root.output_resources.{}", o.label);
            let out = registry.build_output_at(o, sctx, &path)?;
            sctx.resources
                .insert_output(&conf.org, o.label.clone(), out);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn execute_stream_loop(
    stream_id: String,
    org_id: String,
    started_at: Instant,
    mut input: Box<dyn ekiben_core::Input>,
    procs: Vec<Arc<dyn ekiben_core::Processor>>,
    output: Arc<dyn ekiben_core::Output>,
    sctx: StreamContext,
    pctx: ProcContext,
    threads: isize,
    paging: Paging,
    input_connected: Arc<AtomicBool>,
    output_connected: Arc<AtomicBool>,
) -> RunSummary {
    let mut summary = RunSummary {
        stream_id,
        org_id,
        success: true,
        ..Default::default()
    };

    if let Err(e) = input.connect(&sctx).await {
        summary.success = false;
        summary.error = Some(format!("input connect failed: {e}"));
        summary.duration_ms = started_at.elapsed().as_millis() as u64;
        return summary;
    }
    input_connected.store(true, Ordering::Release);

    if let Err(e) = output.connect(&sctx).await {
        let _ = input.close().await;
        input_connected.store(false, Ordering::Release);
        summary.success = false;
        summary.error = Some(format!("output connect failed: {e}"));
        summary.duration_ms = started_at.elapsed().as_millis() as u64;
        return summary;
    }
    output_connected.store(true, Ordering::Release);

    let cpu_limit_ms = sctx.org.get_quotas().max_cpu_time_ms_per_run;
    let workers = worker_count(threads);
    let (tx_chan, rx_chan) = mpsc::channel::<(Batch, AckResponder)>(64.max(workers * 2));

    let sctx_in = sctx.clone();
    let in_conn_flag = input_connected.clone();
    let input_task = tokio::spawn(async move {
        let mut inp = input;
        let mut read_err: Option<String> = None;
        loop {
            if sctx_in.is_shutting_down() {
                break;
            }
            match inp.read_batch(&sctx_in).await {
                Ok((batch, ack)) => {
                    if batch.is_empty() {
                        ack.ack();
                        continue;
                    }
                    let msg_count = batch.len() as u64;
                    // The organization's message rate paces the input (backpressure); it never fails it.
                    let waiting = sctx_in.clone();
                    if !sctx_in
                        .org
                        .admit_messages(msg_count, || waiting.is_shutting_down())
                        .await
                    {
                        ack.nack(BatchError::new(batch, "stream stopped".to_string()));
                        break;
                    }
                    let byte_count: u64 = batch.iter().map(|p| p.as_bytes_ref().len() as u64).sum();
                    sctx_in
                        .metrics
                        .input_received
                        .fetch_add(msg_count, Ordering::Relaxed);
                    sctx_in
                        .metrics
                        .input_bytes
                        .fetch_add(byte_count, Ordering::Relaxed);
                    if tx_chan.send((batch, ack)).await.is_err() {
                        break;
                    }
                }
                Err(CoreError::EndOfInput) => break,
                Err(CoreError::Timeout) => continue,
                Err(e) => {
                    read_err = Some(e.to_string());
                    break;
                }
            }
        }
        let _ = inp.close().await;
        in_conn_flag.store(false, Ordering::Release);
        read_err
    });

    let pool = Arc::new(WorkerPool {
        rx: tokio::sync::Mutex::new(rx_chan),
        procs: procs.clone(),
        output: output.clone(),
        sctx: sctx.clone(),
        pctx,
        paging,
        cpu_limit_ns: cpu_limit_ms.saturating_mul(1_000_000),
        busy_ns: AtomicU64::new(0),
        stop: AtomicBool::new(false),
        taken: AtomicU64::new(0),
        in_flight: Arc::new(tokio::sync::Semaphore::new(workers * 2)),
        failure: std::sync::Mutex::new(None),
    });
    let (done_tx, done_rx) = mpsc::channel::<Processed>(workers * 2);
    let writer = tokio::spawn(pool.clone().write_in_order(done_rx));
    let handles: Vec<_> = (0..workers)
        .map(|_| pipeline_runtime().spawn(pool.clone().work(done_tx.clone())))
        .collect();
    drop(done_tx);
    for h in handles {
        if let Err(e) = h.await {
            pool.fail(format!("pipeline worker ended abnormally: {e}"));
        }
    }
    if let Err(e) = writer.await {
        pool.fail(format!("pipeline writer ended abnormally: {e}"));
    }
    let write_failed = pool.failure.lock().unwrap().take();
    // Dropping the pool drops the receiver, which ends an input still waiting to send.
    drop(pool);
    let input_err = input_task.await.ok().flatten();

    for p in &procs {
        let _ = p.close().await;
    }
    let _ = output.close().await;
    output_connected.store(false, Ordering::Release);

    summary.messages_in = sctx.metrics.input_received.load(Ordering::Relaxed);
    summary.bytes_in = sctx.metrics.input_bytes.load(Ordering::Relaxed);
    summary.messages_out = sctx.metrics.output_sent.load(Ordering::Relaxed);
    summary.bytes_out = sctx.metrics.output_bytes.load(Ordering::Relaxed);
    summary.messages_failed = sctx.metrics.processor_errors_total.load(Ordering::Relaxed)
        + sctx.metrics.output_error.load(Ordering::Relaxed);
    summary.duration_ms = started_at.elapsed().as_millis() as u64;
    summary.peak_rss_bytes = read_peak_rss_bytes();

    if let Some(err) = write_failed.or(input_err) {
        summary.success = false;
        summary.error = Some(err);
    }

    summary
}

/// The runtime every stream's pipeline workers run on, one thread per core, apart from the runtime
/// of inputs, outputs and the API. tokio does not preempt a task, so CPU-bound batches on the main
/// runtime held the input and the writer back: woken into a busy thread's LIFO slot, which no other
/// thread can steal, they waited out each batch (threads -1 ran no faster than threads 1).
fn pipeline_runtime() -> tokio::runtime::Handle {
    static RUNTIME: std::sync::OnceLock<Option<tokio::runtime::Runtime>> =
        std::sync::OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(worker_count(-1))
                .thread_name("ekiben-pipeline")
                .enable_all()
                .build()
                .ok()
        })
        .as_ref()
        .map_or_else(tokio::runtime::Handle::current, |rt| rt.handle().clone())
}

/// The number of pipeline workers for `pipeline.threads`: zero or less means one per core (Bento).
fn worker_count(threads: isize) -> usize {
    if threads > 0 {
        return threads as usize;
    }
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

/// The workers of one stream: each takes the next input batch and runs the pipeline over it, so
/// batches run in parallel; one writer writes the results in input order, so a run's output does
/// not depend on the thread count. A worker hands its results over page by page
/// ([`execute_paged`]) and waits once [`PAGES_AHEAD`] wait unwritten.
struct WorkerPool {
    rx: tokio::sync::Mutex<mpsc::Receiver<(Batch, AckResponder)>>,
    procs: Vec<Arc<dyn ekiben_core::Processor>>,
    output: Arc<dyn ekiben_core::Output>,
    sctx: StreamContext,
    pctx: ProcContext,
    paging: Paging,
    /// `max_cpu_time_ms_per_run`, charged with the time workers spend inside the processors.
    cpu_limit_ns: u64,
    busy_ns: AtomicU64,
    stop: AtomicBool,
    failure: std::sync::Mutex<Option<String>>,
    /// Sequence numbers handed out as batches are taken.
    taken: AtomicU64,
    /// Bounds the batches taken but not yet written, so a slow batch cannot let the writer's
    /// reorder buffer grow without limit.
    in_flight: Arc<tokio::sync::Semaphore>,
}

/// A batch the pipeline runs over, its results waiting for their turn to be written.
struct Processed {
    seq: u64,
    sorter: SortGroup,
    tagged_batch: Batch,
    ack: AckResponder,
    /// The results as each is done; an `Err` ends them with the reason the run failed.
    results: mpsc::Receiver<Result<Batch, String>>,
    slot: tokio::sync::OwnedSemaphorePermit,
}

impl WorkerPool {
    /// Records the run's first failure.
    fn fail(&self, reason: String) {
        self.failure.lock().unwrap().get_or_insert(reason);
    }

    async fn work(self: Arc<Self>, done: mpsc::Sender<Processed>) {
        while !self.stop.load(Ordering::Acquire) {
            let Ok(slot) = self.in_flight.clone().acquire_owned().await else {
                return;
            };
            let next = {
                let mut rx = self.rx.lock().await;
                rx.recv()
                    .await
                    .map(|b| (self.taken.fetch_add(1, Ordering::Relaxed), b))
            };
            let Some((seq, (batch, ack))) = next else {
                return;
            };
            // Releasing the receiver's lock woke the next worker into this thread's LIFO slot, which
            // no other thread can steal: yield so it takes the next batch now, instead of waiting
            // out this CPU-bound one (without this the workers ran one after another, 12-14 s).
            tokio::task::yield_now().await;
            let (sorter, tagged_batch) = SortGroup::new(batch);
            let (results, rx) = mpsc::channel(PAGES_AHEAD);
            let processed = Processed {
                seq,
                sorter,
                tagged_batch: tagged_batch.clone(),
                ack,
                results: rx,
                slot,
            };
            if done.send(processed).await.is_err() {
                return;
            }
            let failed = if self.stop.load(Ordering::Acquire) {
                Some("stream stopped".to_string())
            } else if self.busy_ns.load(Ordering::Relaxed) > self.cpu_limit_ns {
                Some(format!(
                    "stream run exceeded per-run processing time budget ({} ms)",
                    self.cpu_limit_ns / 1_000_000
                ))
            } else {
                self.run(tagged_batch, &results).await.err()
            };
            if let Some(reason) = failed {
                let _ = results.send(Err(reason)).await;
            }
        }
    }

    /// Runs the pipeline over one input batch, sending each result to the writer as it is done.
    /// Only the time inside the processors counts against the run's budget, not the waits for
    /// the writer.
    async fn run(
        &self,
        batch: Batch,
        results: &mpsc::Sender<Result<Batch, String>>,
    ) -> Result<(), String> {
        let started = Instant::now();
        let waited = AtomicU64::new(0);
        let res = execute_paged(&self.pctx, &self.procs, batch, &self.paging, |mut page| {
            // Encode a small result here, in parallel, rather than in the one writer; a larger
            // one is encoded as it is written, so its bytes never all sit beside its values.
            if page.iter().map(Part::size_estimate).sum::<usize>() <= ENCODE_AHEAD_BYTES {
                for part in &mut page {
                    let _ = part.as_bytes();
                }
            }
            let waited = &waited;
            async move {
                if self.stop.load(Ordering::Acquire) {
                    return Err(CoreError::other("stream stopped"));
                }
                let at = Instant::now();
                let sent = results.send(Ok(page)).await;
                waited.fetch_add(at.elapsed().as_nanos() as u64, Ordering::Relaxed);
                sent.map_err(|_| CoreError::other("stream stopped"))
            }
        })
        .await;
        let busy = (started.elapsed().as_nanos() as u64).saturating_sub(waited.into_inner());
        self.busy_ns.fetch_add(busy, Ordering::Relaxed);
        res.map_err(|e| e.to_string())
    }

    /// Writes processed batches in sequence order as they arrive, holding early ones back.
    async fn write_in_order(self: Arc<Self>, mut done: mpsc::Receiver<Processed>) {
        let mut held = std::collections::BTreeMap::new();
        let mut next = 0u64;
        while let Some(p) = done.recv().await {
            held.insert(p.seq, p);
            while let Some(p) = held.remove(&next) {
                next += 1;
                self.write_all(p).await;
            }
        }
    }

    /// Writes a processed batch's results as they come, then acks it, or nacks the messages a
    /// write failed, or the whole batch when the pipeline failed part way: the results written
    /// before stay written.
    async fn write_all(&self, mut p: Processed) {
        let mut batch_err: Option<BatchError> = None;
        while let Some(result) = p.results.recv().await {
            let out_batch = match result {
                Ok(out_batch) => out_batch,
                Err(reason) => {
                    p.ack.nack(BatchError::new(p.tagged_batch, reason.clone()));
                    self.fail(reason);
                    self.stop.store(true, Ordering::Release);
                    return;
                }
            };
            if !out_batch.is_empty() {
                if let Err(e) = self.write(&out_batch).await {
                    let reason = e.to_string();
                    self.fail(reason.clone());
                    let b_err = batch_err.get_or_insert_with(|| {
                        BatchError::new(p.tagged_batch.clone(), reason.clone())
                    });
                    for part in &out_batch {
                        if let Some(orig_idx) = p.sorter.get_index(part) {
                            b_err.failed(orig_idx, reason.clone());
                        }
                    }
                }
            }
            // Freeing a batch's messages costs more than writing them (glibc frees memory
            // other threads allocated slowly): free on the blocking pool, in parallel.
            tokio::task::spawn_blocking(move || drop(out_batch));
        }
        match batch_err {
            Some(b_err) => p.ack.nack(b_err),
            None => p.ack.ack(),
        }
        // The in-flight slot goes back once its batch is freed.
        let leftovers = (p.tagged_batch, p.slot);
        tokio::task::spawn_blocking(move || drop(leftovers));
    }

    /// Writes one batch and counts the messages written (`messages_out`).
    async fn write(&self, out_batch: &Batch) -> Result<(), CoreError> {
        // The output's copy holds the encoded bytes, freed with it once written.
        let mut out_batch = out_batch.clone();
        let out_bytes: u64 = out_batch
            .iter_mut()
            .map(|p| p.as_bytes().len() as u64)
            .sum();
        let out_len = out_batch.len() as u64;
        let metrics = &self.sctx.metrics;
        match self.output.write_batch(&self.sctx, out_batch).await {
            Ok(()) => {
                metrics.output_sent.fetch_add(out_len, Ordering::Relaxed);
                metrics.output_batch_sent.fetch_add(1, Ordering::Relaxed);
                metrics.output_bytes.fetch_add(out_bytes, Ordering::Relaxed);
                Ok(())
            }
            Err(e) => {
                metrics.output_error.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }
}

fn read_peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line
            .strip_prefix("VmHWM:")
            .or_else(|| line.strip_prefix("VmRSS:"))
        {
            let kb_str = rest.split_whitespace().next()?;
            let kb: u64 = kb_str.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}
