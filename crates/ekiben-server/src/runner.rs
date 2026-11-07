use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ekiben_core::config::build_blobl_env_for_stream;
use ekiben_core::{
    execute_all, AckResponder, Batch, BatchError, CoreError, MetricsRegistry, OrgId, ProcContext,
    Registry, ResourceManager, RunSummary, SortGroup, StreamConfig, StreamContext, StreamMetrics,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

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
    allow_host_env: bool,
) -> Result<Arc<RunningStreamHandle>, CoreError> {
    let org_ctx = resources.get_or_create_org(&conf.org);
    let stream_slot_guard = org_ctx.acquire_stream_slot()?;

    // Build environment variables for this stream from its org's `secret_refs` + process env if allowed
    let mut env_vars = HashMap::new();
    for secret_name in &conf.secret_refs {
        if let Some(val) = org_ctx.resolve_secret(secret_name) {
            env_vars.insert(secret_name.clone(), val);
        }
    }
    let blobl_env = build_blobl_env_for_stream(&env_vars, allow_host_env);

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
    allow_host_env: bool,
) -> Result<RunSummary, CoreError> {
    let handle = spawn_stream(
        stream_id.into(),
        conf,
        registry,
        resources,
        None,
        file_root,
        allow_host_env,
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
            let inp = registry.build_input(i, sctx)?;
            sctx.resources.insert_input(&conf.org, i.label.clone(), inp);
        }
    }
    for o in &conf.resources.output_resources {
        if !o.label.is_empty() {
            let out = registry.build_output(o, sctx)?;
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
    let (tx_chan, mut rx_chan) = mpsc::channel::<(Batch, AckResponder)>(64);

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
                    let byte_count: u64 = batch.iter().map(|p| p.as_bytes_ref().len() as u64).sum();
                    sctx_in
                        .metrics
                        .input_received
                        .fetch_add(msg_count, Ordering::Relaxed);
                    sctx_in
                        .metrics
                        .input_bytes
                        .fetch_add(byte_count, Ordering::Relaxed);
                    if !sctx_in.label.is_empty() {
                        sctx_in
                            .metrics
                            .incr_counter(&format!("{}.input_received", sctx_in.label), msg_count);
                    }
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

    let mut write_failed: Option<String> = None;

    while let Some((batch, ack)) = rx_chan.recv().await {
        if started_at.elapsed().as_millis() as u64 > cpu_limit_ms {
            let err_msg = format!("stream run exceeded per-run time budget ({cpu_limit_ms} ms)");
            ack.nack(BatchError::new(batch, err_msg.clone()));
            write_failed = Some(err_msg);
            break;
        }

        let (sorter, tagged_batch) = SortGroup::new(batch);
        match execute_all(&pctx, &procs, vec![tagged_batch.clone()]).await {
            Ok(out_batches) => {
                if out_batches.is_empty() {
                    ack.ack();
                    continue;
                }
                let mut batch_err: Option<BatchError> = None;
                for out_batch in out_batches {
                    if out_batch.is_empty() {
                        continue;
                    }
                    let collapsed: u64 = out_batch.iter().map(|p| p.collapsed_count() as u64).sum();
                    let out_bytes: u64 = out_batch
                        .iter()
                        .map(|p| p.as_bytes_ref().len() as u64)
                        .sum();

                    match output.write_batch(&sctx, out_batch.clone()).await {
                        Ok(()) => {
                            sctx.metrics
                                .output_sent
                                .fetch_add(collapsed, Ordering::Relaxed);
                            sctx.metrics
                                .output_batch_sent
                                .fetch_add(1, Ordering::Relaxed);
                            sctx.metrics
                                .output_bytes
                                .fetch_add(out_bytes, Ordering::Relaxed);
                        }
                        Err(e) => {
                            sctx.metrics.output_error.fetch_add(1, Ordering::Relaxed);
                            let err_str = e.to_string();
                            write_failed = Some(err_str.clone());
                            let b_err = batch_err.get_or_insert_with(|| {
                                BatchError::new(tagged_batch.clone(), err_str.clone())
                            });
                            for p in &out_batch {
                                if let Some(orig_idx) = sorter.get_index(p) {
                                    b_err.failed(orig_idx, err_str.clone());
                                }
                            }
                        }
                    }
                }
                if let Some(b_err) = batch_err {
                    ack.nack(b_err);
                } else {
                    ack.ack();
                }
            }
            Err(e) => {
                let err_str = e.to_string();
                ack.nack(BatchError::new(tagged_batch, err_str.clone()));
                write_failed = Some(err_str);
                break;
            }
        }
    }

    drop(rx_chan);
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
