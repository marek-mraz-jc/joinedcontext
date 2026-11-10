use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ekiben_bloblang::Executor;
use tokio::sync::mpsc;

use crate::config::BatchPolicyConfig;
use crate::error::{BatchError, CoreError};
use crate::message::{Batch, Part, PartsView, SortGroup};
use crate::prometheus::{Counter, Timer};
use crate::traits::{Input, Output, Pages, ProcContext, Processor, StreamContext};
use crate::transaction::{AckResponder, CombinedAcker, Transaction};

/// Executes a slice of processors sequentially over `batches` (`internal/component/processor/execute.go:ExecuteAll`).
pub async fn execute_all(
    ctx: &ProcContext,
    procs: &[Arc<dyn Processor>],
    batches: Vec<Batch>,
) -> Result<Vec<Batch>, CoreError> {
    let mut current = batches;
    for proc in procs {
        if current.is_empty() {
            break;
        }
        if ctx.is_shutting_down() {
            return Err(CoreError::ComponentClosed);
        }
        let mut next_batches = Vec::new();
        for batch in current {
            let res = proc.process_batch(ctx, batch).await?;
            next_batches.extend(res);
        }
        current = next_batches;
    }
    Ok(current)
}

/// Executes a slice of processors sequentially, skipping any batch whose first message (`batch[0]`)
/// already has an error flag (`internal/component/processor/execute.go:ExecuteTryAll`).
pub async fn execute_try_all(
    ctx: &ProcContext,
    procs: &[Arc<dyn Processor>],
    batches: Vec<Batch>,
) -> Result<Vec<Batch>, CoreError> {
    let mut current = batches;
    for proc in procs {
        if current.is_empty() {
            break;
        }
        if ctx.is_shutting_down() {
            return Err(CoreError::ComponentClosed);
        }
        let mut next_batches = Vec::new();
        for batch in current {
            if batch.first().is_some_and(Part::is_errored) {
                next_batches.push(batch);
                continue;
            }
            let res = proc.process_batch(ctx, batch).await?;
            next_batches.extend(res);
        }
        current = next_batches;
    }
    Ok(current)
}

struct CatchSlot {
    batches: Vec<Batch>,
    caught: bool,
}

/// Executes a slice of processors only on batches whose first message had an error flag upon entering
/// (`internal/component/processor/execute.go:ExecuteCatchAll`).
pub async fn execute_catch_all(
    ctx: &ProcContext,
    procs: &[Arc<dyn Processor>],
    batches: Vec<Batch>,
) -> Result<Vec<Batch>, CoreError> {
    let mut slots: Vec<CatchSlot> = batches
        .into_iter()
        .map(|b| {
            let caught = b.first().is_some_and(Part::is_errored);
            CatchSlot {
                batches: vec![b],
                caught,
            }
        })
        .collect();

    for proc in procs {
        if ctx.is_shutting_down() {
            return Err(CoreError::ComponentClosed);
        }
        for slot in &mut slots {
            if !slot.caught || slot.batches.is_empty() {
                continue;
            }
            let mut next_batches = Vec::new();
            for b in std::mem::take(&mut slot.batches) {
                let res = proc.process_batch(ctx, b).await?;
                next_batches.extend(res);
            }
            slot.batches = next_batches;
        }
    }

    let mut out = Vec::new();
    for slot in slots {
        out.extend(slot.batches);
    }
    Ok(out)
}

/// Wraps a `Processor` to automatically record `processor_received`, `processor_sent`, `processor_error`,
/// and `processor_latency_ns` (`internal/component/processor/auto_observed.go`).
pub struct ObservedProcessor {
    inner: Arc<dyn Processor>,
    ctx: ProcContext,
    prom: ProcessorMetrics,
}

/// Bento's processor series for one processor (labels `label` and `path`).
struct ProcessorMetrics {
    received: Counter,
    batch_received: Counter,
    sent: Counter,
    batch_sent: Counter,
    latency: Timer,
}

/// The Bento metric path of a processor context path (`pipeline.processors.0` is
/// `root.pipeline.processors.0`).
pub fn metric_path(ctx_path: &str) -> String {
    if ctx_path == "root" || ctx_path.starts_with("root.") {
        ctx_path.to_string()
    } else if ctx_path.is_empty() {
        "root".to_string()
    } else {
        format!("root.{ctx_path}")
    }
}

impl ObservedProcessor {
    /// Observes `inner` under the label its context carries.
    pub fn new(inner: Arc<dyn Processor>, ctx: ProcContext) -> Self {
        let label = ctx.label.clone();
        Self::with_identity(inner, ctx, &label)
    }

    /// Observes `inner` with Bento's metric identity: the processor's own `label` (empty when it
    /// has none) and its config path.
    pub fn with_identity(inner: Arc<dyn Processor>, ctx: ProcContext, label: &str) -> Self {
        let path = metric_path(&ctx.path);
        let prom = match ctx.metrics.component(label, &path) {
            Some(ns) => {
                ctx.metrics
                    .register_processor_error(&ctx.path, ns.counter("processor_error"));
                ProcessorMetrics {
                    received: ns.counter("processor_received"),
                    batch_received: ns.counter("processor_batch_received"),
                    sent: ns.counter("processor_sent"),
                    batch_sent: ns.counter("processor_batch_sent"),
                    latency: ns.timer("processor_latency_ns"),
                }
            }
            None => ProcessorMetrics {
                received: Counter::noop(),
                batch_received: Counter::noop(),
                sent: Counter::noop(),
                batch_sent: Counter::noop(),
                latency: Timer::noop(),
            },
        };
        Self { inner, ctx, prom }
    }
}

#[async_trait]
impl Processor for ObservedProcessor {
    fn processor_type(&self) -> &'static str {
        self.inner.processor_type()
    }

    fn label(&self) -> &str {
        &self.ctx.label
    }

    async fn process_batch(
        &self,
        _caller_ctx: &ProcContext,
        batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let in_len = batch.len();
        let bytes: usize = batch.iter().map(Part::size_estimate).sum();
        self.ctx.org.check_batch_memory(bytes)?;

        self.ctx
            .metrics
            .incr_processor_received(&self.ctx.label, in_len as u64);
        self.prom.received.incr(in_len as u64);
        self.prom.batch_received.incr(1);
        let started = Instant::now();

        let (input, kept) = if self.inner.may_fail_batch() {
            (batch.clone(), Some(batch))
        } else {
            (batch, None)
        };
        match self.inner.process_batch(&self.ctx, input).await {
            Ok(out_batches) => {
                let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
                self.ctx
                    .metrics
                    .record_processor_latency(&self.ctx.label, elapsed);
                self.prom.latency.timing(elapsed);
                if out_batches.is_empty() {
                    return Ok(Vec::new());
                }
                let sent_msgs: u64 = out_batches.iter().map(|b| b.len() as u64).sum();
                self.ctx.metrics.incr_processor_sent(
                    &self.ctx.label,
                    sent_msgs,
                    out_batches.len() as u64,
                );
                self.prom.sent.incr(sent_msgs);
                self.prom.batch_sent.incr(out_batches.len() as u64);
                Ok(out_batches)
            }
            Err(CoreError::QuotaExceeded { org, reason }) => {
                Err(CoreError::QuotaExceeded { org, reason })
            }
            Err(CoreError::ComponentClosed) => Err(CoreError::ComponentClosed),
            Err(err) => {
                // In Bento, if a processor returns a batch-wide error, all parts in the batch are marked
                // with `DetailedError` and continue through the pipeline!
                // A processor promising no batch-wide error broke it: fail the run, never drop.
                let Some(mut errored_batch) = kept else {
                    return Err(err);
                };
                let err_msg = err.to_string();
                for part in &mut errored_batch {
                    self.ctx.on_error(part, err_msg.clone());
                }
                let elapsed = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
                self.ctx
                    .metrics
                    .record_processor_latency(&self.ctx.label, elapsed);
                self.prom.latency.timing(elapsed);
                self.ctx.metrics.incr_processor_sent(
                    &self.ctx.label,
                    errored_batch.len() as u64,
                    1,
                );
                self.prom.sent.incr(errored_batch.len() as u64);
                self.prom.batch_sent.incr(1);
                Ok(vec![errored_batch])
            }
        }
    }

    fn pages(
        &self,
        _caller_ctx: &ProcContext,
        batch: Batch,
        rows: usize,
    ) -> Result<Box<Pages>, Batch> {
        // Over the memory quota: `process_batch` refuses it as it refuses any other batch.
        let bytes: usize = batch.iter().map(Part::size_estimate).sum();
        if self.ctx.org.check_batch_memory(bytes).is_err() {
            return Err(batch);
        }
        let in_len = batch.len() as u64;
        let pages = self.inner.pages(&self.ctx, batch, rows)?;
        self.ctx
            .metrics
            .incr_processor_received(&self.ctx.label, in_len);
        self.prom.received.incr(in_len);
        self.prom.batch_received.incr(1);
        let (ctx, sent, batch_sent) = (
            self.ctx.clone(),
            self.prom.sent.clone(),
            self.prom.batch_sent.clone(),
        );
        Ok(Box::new(pages.inspect(move |page| {
            ctx.metrics
                .incr_processor_sent(&ctx.label, page.len() as u64, 1);
            sent.incr(page.len() as u64);
            batch_sent.incr(1);
        })))
    }

    async fn close(&self) -> Result<(), CoreError> {
        self.inner.close().await
    }
}

/// Wraps an `Input` with input-level `processors` (`internal/component/input/wrap_with_pipeline.go`).
pub struct InputWithProcessors {
    inner: Box<dyn Input>,
    procs: Vec<Arc<dyn Processor>>,
    ctx: ProcContext,
    pending_batches: Vec<(Batch, AckResponder)>,
}

impl InputWithProcessors {
    pub fn new(inner: Box<dyn Input>, procs: Vec<Arc<dyn Processor>>, ctx: ProcContext) -> Self {
        Self {
            inner,
            procs,
            ctx,
            pending_batches: Vec::new(),
        }
    }
}

#[async_trait]
impl Input for InputWithProcessors {
    fn input_type(&self) -> &'static str {
        self.inner.input_type()
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    async fn connect(&mut self, ctx: &StreamContext) -> Result<(), CoreError> {
        self.inner.connect(ctx).await
    }

    async fn read_batch(
        &mut self,
        ctx: &StreamContext,
    ) -> Result<(Batch, AckResponder), CoreError> {
        loop {
            if !self.pending_batches.is_empty() {
                return Ok(self.pending_batches.remove(0));
            }
            let (batch, ack) = self.inner.read_batch(ctx).await?;
            let res_batches = execute_all(&self.ctx, &self.procs, vec![batch]).await?;
            if res_batches.is_empty() {
                ack.ack();
                continue;
            }
            if res_batches.len() == 1 {
                return Ok((res_batches.into_iter().next().unwrap(), ack));
            }
            let combined = CombinedAcker::new(ack);
            for b in res_batches {
                self.pending_batches.push((b, combined.derive()));
            }
        }
    }

    fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    async fn close(&mut self) -> Result<(), CoreError> {
        for p in &self.procs {
            let _ = p.close().await;
        }
        self.inner.close().await
    }
}

/// An output's `batching` policy (`internal/component/output/batched_send.go` with
/// `internal/batch/policy`): messages gather across writes in Bento's batcher (`count`,
/// `byte_size`, `check`, then the policy's `processors`) and go out when it triggers, when
/// `period` passes since the first one waiting, or when the output closes.
///
/// ponytail: the runner awaits each write before the next, so a write that waited for its batch
/// would stall the stream; a message that joins a batch is acknowledged at once. A batch whose
/// later write fails is logged and counted (`output_error`) and the next write answers that error,
/// where Bento would nack the messages. The Portal's outcome post, the one batched output the seed
/// uses, sits in a `fallback` whose next output is `drop`, which acks them in Bento too. Upgrade
/// path: let the runner hold acks per transaction until its batch is written.
pub struct BatchedOutput {
    inner: Arc<dyn Output>,
    state: Arc<tokio::sync::Mutex<BatchState>>,
}

struct BatchState {
    batcher: Batcher,
    timer: bool,
    failed: Option<CoreError>,
    /// The stream the writes came from: `close` writes what still waits with it.
    ctx: Option<StreamContext>,
}

impl BatchedOutput {
    pub fn new(inner: Arc<dyn Output>, batcher: Batcher) -> Self {
        Self {
            inner,
            state: Arc::new(tokio::sync::Mutex::new(BatchState {
                batcher,
                timer: false,
                failed: None,
                ctx: None,
            })),
        }
    }
}

/// Flushes what waits and writes it.
async fn flush_batched(
    inner: &Arc<dyn Output>,
    state: &mut BatchState,
    ctx: &StreamContext,
) -> Result<(), CoreError> {
    match state.batcher.flush().await? {
        Some(b) if !b.is_empty() => inner.write_batch(ctx, b).await,
        _ => Ok(()),
    }
}

/// A flush nobody awaits: its failure is logged, counted and answered by the next write.
fn deferred_failure(state: &mut BatchState, ctx: &StreamContext, e: CoreError) {
    tracing::warn!(stream = %ctx.stream_id, error = %e, "a batched output's write failed");
    ctx.metrics
        .output_error
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    state.failed.get_or_insert(e);
}

#[async_trait]
impl Output for BatchedOutput {
    fn output_type(&self) -> &'static str {
        self.inner.output_type()
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    async fn connect(&self, ctx: &StreamContext) -> Result<(), CoreError> {
        self.inner.connect(ctx).await
    }

    async fn write_batch(&self, ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        let mut state = self.state.lock().await;
        if let Some(e) = state.failed.take() {
            return Err(e);
        }
        if state.ctx.is_none() {
            state.ctx = Some(ctx.clone());
        }
        for part in batch {
            state.batcher.skip_idle_periods();
            if state.batcher.add(part) {
                flush_batched(&self.inner, &mut state, ctx).await?;
            }
        }
        if state.batcher.count() > 0 && !state.timer {
            if let Some(wait) = state.batcher.until_next() {
                state.timer = true;
                let (inner, shared, ctx) = (self.inner.clone(), self.state.clone(), ctx.clone());
                tokio::spawn(async move {
                    tokio::time::sleep(wait).await;
                    let mut state = shared.lock().await;
                    state.timer = false;
                    if let Err(e) = flush_batched(&inner, &mut state, &ctx).await {
                        deferred_failure(&mut state, &ctx, e);
                    }
                });
            }
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    async fn close(&self) -> Result<(), CoreError> {
        let mut state = self.state.lock().await;
        let flushed = match state.ctx.clone() {
            Some(ctx) => flush_batched(&self.inner, &mut state, &ctx).await,
            None => Ok(()),
        };
        let closed = self.inner.close().await;
        flushed
            .and(closed)
            .and(state.failed.take().map_or(Ok(()), Err))
    }
}

/// Wraps an `Output` with output-level `processors` (`internal/component/output/wrap_with_pipeline.go`).
pub struct OutputWithProcessors {
    inner: Arc<dyn Output>,
    procs: Vec<Arc<dyn Processor>>,
    ctx: ProcContext,
}

impl OutputWithProcessors {
    pub fn new(inner: Arc<dyn Output>, procs: Vec<Arc<dyn Processor>>, ctx: ProcContext) -> Self {
        Self { inner, procs, ctx }
    }
}

#[async_trait]
impl Output for OutputWithProcessors {
    fn output_type(&self) -> &'static str {
        self.inner.output_type()
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    async fn connect(&self, ctx: &StreamContext) -> Result<(), CoreError> {
        self.inner.connect(ctx).await
    }

    async fn write_batch(&self, ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        let res_batches = execute_all(&self.ctx, &self.procs, vec![batch]).await?;
        for b in res_batches {
            if !b.is_empty() {
                self.inner.write_batch(ctx, b).await?;
            }
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.inner.is_connected()
    }

    async fn close(&self) -> Result<(), CoreError> {
        for p in &self.procs {
            let _ = p.close().await;
        }
        self.inner.close().await
    }
}

/// Batcher implementing Bento's `batching` policy (`internal/batch/policy/policy.go`).
pub struct Batcher {
    count: usize,
    byte_size: usize,
    period: Option<Duration>,
    check: Option<Executor>,
    procs: Vec<Arc<dyn Processor>>,
    proc_ctx: ProcContext,
    parts: Batch,
    size_tally: usize,
    triggered: bool,
    last_batch: Instant,
}

impl Batcher {
    pub fn new(conf: &BatchPolicyConfig, proc_ctx: ProcContext) -> Result<Self, CoreError> {
        let period = if conf.period.is_empty() {
            None
        } else {
            let d = crate::config::parse_duration_str(&conf.period)?;
            (!d.is_zero()).then_some(d)
        };
        let check = if conf.check.is_empty() {
            None
        } else {
            Some(proc_ctx.blobl_env.compile_mapping(&conf.check)?)
        };
        let procs = proc_ctx
            .registry
            .build_processor_list(&conf.processors, &proc_ctx)?;

        Ok(Self {
            count: conf.count,
            byte_size: conf.byte_size,
            period,
            check,
            procs,
            proc_ctx,
            parts: Vec::new(),
            size_tally: 0,
            triggered: false,
            last_batch: Instant::now(),
        })
    }

    pub fn add(&mut self, part: Part) -> bool {
        if self.byte_size > 0 {
            self.size_tally += part.as_bytes_ref().len();
        }
        self.parts.push(part);

        if !self.triggered && self.count > 0 && self.parts.len() >= self.count {
            self.triggered = true;
        }
        if !self.triggered && self.byte_size > 0 && self.size_tally >= self.byte_size {
            self.triggered = true;
        }
        if !self.triggered {
            if let Some(ref check) = self.check {
                let idx = self.parts.len() - 1;
                if check
                    .query_part(idx, &PartsView(&self.parts))
                    .unwrap_or(false)
                {
                    self.triggered = true;
                }
            }
        }
        self.triggered || self.period.is_some_and(|p| self.last_batch.elapsed() >= p)
    }

    pub fn count(&self) -> usize {
        self.parts.len()
    }

    /// Bento's output batcher flushes on every period tick, an empty batch too
    /// (`internal/component/output/batcher`): while nothing waits, its period runs on from the
    /// last flush in whole periods, so a message joining an idle batch waits for the next tick.
    pub fn skip_idle_periods(&mut self) {
        let Some(p) = self.period.filter(|_| self.parts.is_empty()) else {
            return;
        };
        let ticks = self.last_batch.elapsed().as_nanos() / p.as_nanos();
        let step = u32::try_from(ticks).ok().and_then(|n| p.checked_mul(n));
        if let Some(at) = step.and_then(|s| self.last_batch.checked_add(s)) {
            self.last_batch = at;
        }
    }

    pub fn until_next(&self) -> Option<Duration> {
        self.period
            .map(|p| p.saturating_sub(self.last_batch.elapsed()))
    }

    pub async fn flush(&mut self) -> Result<Option<Batch>, CoreError> {
        if self.parts.is_empty() {
            self.last_batch = Instant::now();
            self.triggered = false;
            return Ok(None);
        }
        let batch = std::mem::take(&mut self.parts);
        self.size_tally = 0;
        self.last_batch = Instant::now();
        self.triggered = false;

        if self.procs.is_empty() {
            return Ok(Some(batch));
        }

        let res = execute_all(&self.proc_ctx, &self.procs, vec![batch]).await?;
        if res.is_empty() {
            return Ok(None);
        }
        let mut combined = Vec::new();
        for b in res {
            combined.extend(b);
        }
        Ok(Some(combined))
    }
}

/// Multi-worker pipeline pool (`internal/pipeline/pool.go` & `internal/pipeline/processor.go`).
/// Reads `Transaction`s from `in_rx`, processes them across `threads` workers, and sends resulting
/// `Transaction`s to `out_tx` with `SortGroup` and `CombinedAcker` tracking when a processor splits batches.
pub struct PipelineWorkerPool {
    threads: usize,
    procs: Vec<Arc<dyn Processor>>,
    ctx: ProcContext,
}

impl PipelineWorkerPool {
    pub fn new(threads: isize, procs: Vec<Arc<dyn Processor>>, ctx: ProcContext) -> Self {
        let num_threads = if threads <= 0 {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        } else {
            threads as usize
        };
        Self {
            threads: num_threads.max(1),
            procs,
            ctx,
        }
    }

    pub async fn run(
        self,
        in_rx: mpsc::Receiver<Transaction>,
        out_tx: mpsc::Sender<Transaction>,
    ) -> Result<(), CoreError> {
        if self.threads == 1 || self.procs.is_empty() {
            return run_single_worker(self.procs, self.ctx, in_rx, out_tx).await;
        }

        let shared_rx = Arc::new(tokio::sync::Mutex::new(in_rx));
        let mut handles = Vec::with_capacity(self.threads);

        for _ in 0..self.threads {
            let rx = shared_rx.clone();
            let tx = out_tx.clone();
            let procs = self.procs.clone();
            let ctx = self.ctx.clone();
            handles.push(tokio::spawn(async move {
                loop {
                    let tran_opt = {
                        let mut guard = rx.lock().await;
                        guard.recv().await
                    };
                    let Some(tran) = tran_opt else {
                        break;
                    };
                    process_one_transaction(&procs, &ctx, tran, &tx).await?;
                }
                Ok::<(), CoreError>(())
            }));
        }
        drop(out_tx);

        for h in handles {
            if let Ok(Err(e)) = h.await {
                return Err(e);
            }
        }
        for p in &self.procs {
            let _ = p.close().await;
        }
        Ok(())
    }
}

async fn run_single_worker(
    procs: Vec<Arc<dyn Processor>>,
    ctx: ProcContext,
    mut in_rx: mpsc::Receiver<Transaction>,
    out_tx: mpsc::Sender<Transaction>,
) -> Result<(), CoreError> {
    while let Some(tran) = in_rx.recv().await {
        process_one_transaction(&procs, &ctx, tran, &out_tx).await?;
    }
    for p in &procs {
        let _ = p.close().await;
    }
    Ok(())
}

async fn process_one_transaction(
    procs: &[Arc<dyn Processor>],
    ctx: &ProcContext,
    tran: Transaction,
    out_tx: &mpsc::Sender<Transaction>,
) -> Result<(), CoreError> {
    if procs.is_empty() {
        let _ = out_tx.send(tran).await;
        return Ok(());
    }
    let (sorter, sort_batch) = SortGroup::new(tran.payload);
    match execute_all(ctx, procs, vec![sort_batch.clone()]).await {
        Ok(mut result_batches) => {
            if result_batches.is_empty() {
                tran.ack.ack();
                return Ok(());
            }
            if result_batches.len() == 1 {
                let single = result_batches.pop().unwrap();
                let _ = out_tx.send(Transaction::new(single, tran.ack)).await;
                return Ok(());
            }
            // Split into multiple batches: use CombinedAcker and map errors back to `sorter`
            let ack = tran.ack;
            let state = Arc::new(std::sync::Mutex::new((
                result_batches.len(),
                None::<BatchError>,
                Some(ack),
            )));
            for b in result_batches {
                let tmp_batch = b.clone();
                let st = state.clone();
                let srt = sorter.clone();
                let sb = sort_batch.clone();
                let derived = AckResponder::from_fn(move |err_opt| {
                    let mut to_resolve = None;
                    let mut final_err = None;
                    {
                        let mut guard = st.lock().unwrap();
                        if let Some(err) = err_opt {
                            let b_err = guard.1.get_or_insert_with(|| {
                                BatchError::new(sb.clone(), err.message.clone())
                            });
                            for m in &tmp_batch {
                                if let Some(b_idx) = srt.get_index(m) {
                                    b_err.failed(b_idx, err.message.clone());
                                }
                            }
                        }
                        guard.0 = guard.0.saturating_sub(1);
                        if guard.0 == 0 {
                            to_resolve = guard.2.take();
                            final_err = guard.1.clone();
                        }
                    }
                    if let Some(root_ack) = to_resolve {
                        root_ack.respond(final_err);
                    }
                });
                let _ = out_tx.send(Transaction::new(b, derived)).await;
            }
            Ok(())
        }
        Err(e) => {
            tran.ack.nack(BatchError::new(sort_batch, e.to_string()));
            Err(e)
        }
    }
}
