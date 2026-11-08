use std::collections::HashSet;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ekiben_bloblang::{Executor, FieldExpression};
use ekiben_core::{
    BackoffConfig, Batch, BatchError, ComponentConfig, CoreError, Output, Part, PartsView,
    Registry, SortGroup, StreamContext,
};
use serde_json::Value as JsonValue;

use crate::path_guard::sanitize_file_path;

pub fn register(registry: &Registry) {
    registry.register_output("drop", |_conf, _ctx| Ok(Arc::new(DropOutput)));

    registry.register_output("stdout", |conf, _ctx| {
        let codec = conf
            .config
            .get("codec")
            .and_then(JsonValue::as_str)
            .unwrap_or("lines")
            .to_string();
        Ok(Arc::new(StdoutOutput { codec }))
    });

    registry.register_output("file", |conf, ctx| {
        Ok(Arc::new(FileOutput::from_config(conf, ctx)?))
    });

    registry.register_output("reject", |conf, ctx| {
        let msg_str = conf
            .config
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                CoreError::config(
                    "an error message must be provided in order to provide context for the rejection",
                )
            })?;
        let expr = ctx.blobl_env.compile_field(msg_str)?;
        Ok(Arc::new(RejectOutput { expr }))
    });

    registry.register_output("reject_errored", |conf, ctx| {
        let child_conf = ComponentConfig::from_json_value(&conf.config, "drop", None)?;
        let child = ctx.registry.build_output(&child_conf, ctx)?;
        Ok(Arc::new(RejectErroredOutput { child }))
    });

    registry.register_output("drop_on", |conf, ctx| {
        Ok(Arc::new(DropOnOutput::from_config(conf, ctx)?))
    });

    registry.register_output("retry", |conf, ctx| {
        Ok(Arc::new(RetryOutput::from_config(conf, ctx)?))
    });

    registry.register_output("fallback", |conf, ctx| {
        Ok(Arc::new(FallbackOutput::from_config(conf, ctx)?))
    });

    registry.register_output("switch", |conf, ctx| {
        Ok(Arc::new(SwitchOutput::from_config(conf, ctx)?))
    });

    registry.register_output("broker", |conf, ctx| {
        Ok(Arc::new(BrokerOutput::from_config(conf, ctx)?))
    });

    registry.register_output("resource", |conf, _ctx| {
        let name = conf
            .config
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CoreError::config("resource output requires a resource name"))?
            .to_string();
        Ok(Arc::new(ResourceOutput { name }))
    });

    registry.register_output("cache", |conf, ctx| {
        Ok(Arc::new(CacheOutput::from_config(conf, ctx)?))
    });

    registry.register_output("sync_response", |_conf, _ctx| {
        Ok(Arc::new(SyncResponseOutput))
    });

    registry.register_output("inproc", |_conf, _ctx| Ok(Arc::new(DropOutput)));
}

/// Implements `drop` output (`internal/impl/pure/output_drop.go`).
pub struct DropOutput;

#[async_trait]
impl Output for DropOutput {
    fn output_type(&self) -> &'static str {
        "drop"
    }

    async fn write_batch(&self, _ctx: &StreamContext, _batch: Batch) -> Result<(), CoreError> {
        Ok(())
    }
}

/// Implements `stdout` output (`internal/impl/io/output_stdout.go`).
pub struct StdoutOutput {
    codec: String,
}

#[async_trait]
impl Output for StdoutOutput {
    fn output_type(&self) -> &'static str {
        "stdout"
    }

    async fn write_batch(&self, _ctx: &StreamContext, mut batch: Batch) -> Result<(), CoreError> {
        let mut stdout = std::io::stdout().lock();
        for part in &mut batch {
            let bytes = part.as_bytes();
            stdout
                .write_all(&bytes)
                .map_err(|e| CoreError::other(e.to_string()))?;
            if self.codec == "lines" && !bytes.ends_with(b"\n") {
                stdout
                    .write_all(b"\n")
                    .map_err(|e| CoreError::other(e.to_string()))?;
            }
        }
        stdout.flush().map_err(|e| CoreError::other(e.to_string()))
    }
}

/// Implements `file` output (`internal/impl/io/output_file.go`).
pub struct FileOutput {
    path: FieldExpression,
    codec: String,
    /// Pipeline workers write batches in parallel: one batch at a time keeps every batch's lines
    /// together and in order. Holds the open file of the last appended path, which Bento's file
    /// output also keeps open until the path changes.
    write_lock: Arc<std::sync::Mutex<Option<(std::path::PathBuf, std::fs::File)>>>,
}

impl FileOutput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("file output config must be an object"))?;
        let path_str = obj
            .get("path")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("file output requires a 'path' field"))?;
        let path = ctx.blobl_env.compile_field(path_str)?;
        let codec = obj
            .get("codec")
            .and_then(JsonValue::as_str)
            .unwrap_or("lines")
            .to_string();
        Ok(Self {
            path,
            codec,
            write_lock: Arc::default(),
        })
    }
}

#[async_trait]
impl Output for FileOutput {
    fn output_type(&self) -> &'static str {
        "file"
    }

    async fn write_batch(&self, ctx: &StreamContext, mut batch: Batch) -> Result<(), CoreError> {
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        let view = PartsView(&batch);
        let append_mode = self.codec == "lines" || self.codec == "append";
        let lines = self.codec == "lines";
        // One write per run of messages to the same file: appended messages join one buffer, while
        // a truncating codec writes each message (the last one per file stays, as in Bento).
        let mut writes: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();
        for (i, part) in batch.iter().enumerate() {
            let resolved = self
                .path
                .resolve_string(i, &view)
                .map_err(|e| CoreError::other(format!("path interpolation error: {e}")))?;
            let safe_path = sanitize_file_path(&resolved, ctx.file_root.as_deref())?;
            let bytes = part.as_bytes_ref();
            if !(append_mode && writes.last().is_some_and(|(p, _)| *p == safe_path)) {
                writes.push((safe_path, Vec::with_capacity(bytes.len() + 1)));
            }
            if let Some((_, buf)) = writes.last_mut() {
                buf.extend_from_slice(&bytes);
                if lines && !bytes.ends_with(b"\n") {
                    buf.push(b'\n');
                }
            }
        }

        let write_lock = self.write_lock.clone();
        tokio::task::spawn_blocking(move || -> Result<(), CoreError> {
            let mut held = write_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (path, bytes) in writes {
                if append_mode {
                    if let Some((_, file)) = held.as_mut().filter(|(p, _)| *p == path) {
                        if let Err(e) = file.write_all(&bytes) {
                            *held = None;
                            return Err(CoreError::other(format!(
                                "failed to write '{}': {e}",
                                path.display()
                            )));
                        }
                        continue;
                    }
                }
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| CoreError::other(e.to_string()))?;
                    }
                }
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .append(append_mode)
                    .truncate(!append_mode)
                    .open(&path)
                    .map_err(|e| {
                        CoreError::other(format!("failed to open '{}': {e}", path.display()))
                    })?;
                file.write_all(&bytes)
                    .map_err(|e| CoreError::other(e.to_string()))?;
                if append_mode {
                    *held = Some((path, file));
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| CoreError::other(e.to_string()))?
    }
}

/// Implements `reject` output (`internal/impl/pure/output_reject.go`).
pub struct RejectOutput {
    expr: FieldExpression,
}

#[async_trait]
impl Output for RejectOutput {
    fn output_type(&self) -> &'static str {
        "reject"
    }

    async fn write_batch(&self, _ctx: &StreamContext, mut batch: Batch) -> Result<(), CoreError> {
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        let view = PartsView(&batch);
        let msg = self
            .expr
            .resolve_string(0, &view)
            .map_err(|e| CoreError::other(format!("reject message interpolation error: {e}")))?;
        Err(CoreError::Batch(BatchError::new(batch, msg)))
    }
}

/// Implements `reject_errored` output (`docs/configuration/error_handling.md`):
/// nacks any message part that has an error flag (`p.is_errored()`), sending only non-errored parts to `child`.
pub struct RejectErroredOutput {
    child: Arc<dyn Output>,
}

#[async_trait]
impl Output for RejectErroredOutput {
    fn output_type(&self) -> &'static str {
        "reject_errored"
    }

    async fn write_batch(&self, ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        let mut clean = Vec::new();
        let mut errored_indices = Vec::new();
        let mut first_err_msg = String::new();

        for (i, part) in batch.iter().enumerate() {
            if let Some(err) = part.error_get() {
                if first_err_msg.is_empty() {
                    first_err_msg = err.message.clone();
                }
                errored_indices.push((i, err.message.clone()));
            } else {
                clean.push(part.clone());
            }
        }

        if !clean.is_empty() {
            self.child.write_batch(ctx, clean).await?;
        }

        if !errored_indices.is_empty() {
            let mut batch_err = BatchError::new(batch, first_err_msg);
            for (idx, msg) in errored_indices {
                batch_err.failed(idx, msg);
            }
            return Err(CoreError::Batch(batch_err));
        }

        Ok(())
    }
}

pub struct DropOnOutput {
    error: bool,
    error_patterns: Vec<String>,
    child: Arc<dyn Output>,
}

impl DropOnOutput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let error = conf
            .config
            .get("error")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let error_patterns = conf
            .config
            .get("error_patterns")
            .and_then(JsonValue::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(JsonValue::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let out_val = conf
            .config
            .get("output")
            .ok_or_else(|| CoreError::config("drop_on requires an 'output' field"))?;
        let out_conf = ComponentConfig::from_json_value(out_val, "drop", None)?;
        let child = ctx.registry.build_output(&out_conf, ctx)?;
        Ok(Self {
            error,
            error_patterns,
            child,
        })
    }
}

#[async_trait]
impl Output for DropOnOutput {
    fn output_type(&self) -> &'static str {
        "drop_on"
    }

    async fn write_batch(&self, ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        match self.child.write_batch(ctx, batch).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if self.error || self.error_patterns.iter().any(|pat| msg.contains(pat)) {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }
}

/// Implements `retry` output (`internal/impl/pure/output_retry.go`).
pub struct RetryOutput {
    backoff: BackoffConfig,
    max_retries: usize,
    child: Arc<dyn Output>,
}

impl RetryOutput {
    pub fn new_indefinite(child: Arc<dyn Output>) -> Self {
        Self {
            backoff: BackoffConfig::default(),
            max_retries: 0,
            child,
        }
    }

    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let backoff = match conf.config.get("backoff") {
            Some(v) => BackoffConfig::from_json_value(v, Duration::ZERO)?,
            None => BackoffConfig::default(),
        };
        let max_retries = conf
            .config
            .get("max_retries")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0) as usize;
        let out_val = conf
            .config
            .get("output")
            .ok_or_else(|| CoreError::config("retry output requires an 'output' field"))?;
        let out_conf = ComponentConfig::from_json_value(out_val, "drop", None)?;
        let child = ctx.registry.build_output(&out_conf, ctx)?;
        Ok(Self {
            backoff,
            max_retries,
            child,
        })
    }
}

#[async_trait]
impl Output for RetryOutput {
    fn output_type(&self) -> &'static str {
        "retry"
    }

    async fn write_batch(&self, ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        let mut attempt = 0usize;
        let mut elapsed = Duration::ZERO;
        loop {
            match self.child.write_batch(ctx, batch.clone()).await {
                Ok(()) => return Ok(()),
                Err(err) => {
                    attempt += 1;
                    if self.max_retries > 0 && attempt > self.max_retries {
                        return Err(err);
                    }
                    let Some(wait) = self.backoff.next_backoff((attempt - 1) as u32, elapsed)
                    else {
                        return Err(err);
                    };
                    elapsed += wait;
                    tokio::time::sleep(wait).await;
                    if ctx.is_shutting_down() {
                        return Err(CoreError::ComponentClosed);
                    }
                }
            }
        }
    }
}

/// Implements `fallback` output (`internal/impl/pure/output_fallback.go`).
/// On failure of tier $i$, sets `fallback_error` metadata and routes only the failed parts
/// (narrowed via `SortGroup` when `BatchError` has indexed errors) to tier $i+1$.
pub struct FallbackOutput {
    outputs: Vec<Arc<dyn Output>>,
}

impl FallbackOutput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let arr = conf
            .config
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| {
                CoreError::config("attempting to create fallback output type with no outputs")
            })?;
        let mut outputs = Vec::with_capacity(arr.len());
        for item in arr {
            let c = ComponentConfig::from_json_value(item, "drop", None)?;
            outputs.push(ctx.registry.build_output(&c, ctx)?);
        }
        Ok(Self { outputs })
    }
}

#[async_trait]
impl Output for FallbackOutput {
    fn output_type(&self) -> &'static str {
        "fallback"
    }

    async fn write_batch(&self, ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        let (mut out_sorter, mut current_batch) = SortGroup::new(batch);
        let mut last_err = None;

        for out in &self.outputs {
            match out.write_batch(ctx, current_batch.clone()).await {
                Ok(()) => return Ok(()),
                Err(CoreError::Batch(b_err)) => {
                    if current_batch.len() > 1 && b_err.indexed_errors() > 0 {
                        let mut only_errs = Vec::new();
                        let mut seen = HashSet::new();
                        b_err.walk_parts_by_source(
                            &out_sorter,
                            &current_batch,
                            |idx, part, err_opt| {
                                if let Some(err_str) = err_opt {
                                    if seen.insert(idx) {
                                        let mut tmp = part.shallow_copy();
                                        tmp.meta_set_str("fallback_error", err_str);
                                        only_errs.push(tmp);
                                    }
                                }
                                true
                            },
                        );
                        if !only_errs.is_empty() {
                            let (ns, nb) = SortGroup::new(only_errs);
                            out_sorter = ns;
                            current_batch = nb;
                            last_err = Some(CoreError::Batch(b_err));
                            continue;
                        }
                    }
                    let err_msg = b_err.message.clone();
                    for m in &mut current_batch {
                        m.meta_set_str("fallback_error", err_msg.clone());
                    }
                    last_err = Some(CoreError::Batch(b_err));
                }
                Err(other_err) => {
                    let err_msg = other_err.to_string();
                    for m in &mut current_batch {
                        m.meta_set_str("fallback_error", err_msg.clone());
                    }
                    last_err = Some(other_err);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| CoreError::other("all fallback outputs failed")))
    }
}

struct SwitchOutputCase {
    check: Option<Executor>,
    output: Arc<dyn Output>,
    cont: bool,
}

/// Implements `switch` output (`internal/impl/pure/output_switch.go`).
pub struct SwitchOutput {
    strict_mode: bool,
    cases: Vec<SwitchOutputCase>,
}

impl SwitchOutput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("switch output config must be an object"))?;
        let strict_mode = obj
            .get("strict_mode")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let retry_until_success = obj
            .get("retry_until_success")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);

        let cases_arr = obj
            .get("cases")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| CoreError::config("switch output requires a 'cases' array"))?;
        if cases_arr.len() < 2 {
            return Err(CoreError::config(
                "attempting to create switch with fewer than 2 cases",
            ));
        }

        let mut cases = Vec::with_capacity(cases_arr.len());
        for (i, c_val) in cases_arr.iter().enumerate() {
            let c_obj = c_val
                .as_object()
                .ok_or_else(|| CoreError::config(format!("switch case {i} must be an object")))?;
            let check = match c_obj.get("check").and_then(JsonValue::as_str) {
                Some(s) if !s.trim().is_empty() => Some(ctx.blobl_env.compile_mapping(s)?),
                _ => None,
            };
            let cont = c_obj
                .get("continue")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            let out_val = c_obj.get("output").ok_or_else(|| {
                CoreError::config(format!("switch case {i} requires an 'output' field"))
            })?;
            let out_conf = ComponentConfig::from_json_value(out_val, "drop", None)?;
            let mut out = ctx.registry.build_output(&out_conf, ctx)?;
            if retry_until_success {
                out = Arc::new(RetryOutput::new_indefinite(out));
            }
            cases.push(SwitchOutputCase {
                check,
                output: out,
                cont,
            });
        }

        Ok(Self { strict_mode, cases })
    }
}

#[async_trait]
impl Output for SwitchOutput {
    fn output_type(&self) -> &'static str {
        "switch"
    }

    async fn write_batch(&self, ctx: &StreamContext, mut batch: Batch) -> Result<(), CoreError> {
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        let view = PartsView(&batch);
        let mut targets: Vec<Batch> = vec![Vec::new(); self.cases.len()];

        for (i, part) in batch.iter().enumerate() {
            let mut routed = false;
            for (j, c) in self.cases.iter().enumerate() {
                let matched = match &c.check {
                    None => true,
                    Some(chk) => chk.query_part(i, &view).unwrap_or(false),
                };
                if matched {
                    routed = true;
                    targets[j].push(part.shallow_copy());
                    if !c.cont {
                        break;
                    }
                }
            }
            if !routed && self.strict_mode {
                return Err(CoreError::other(
                    "no switch output conditions were met by message",
                ));
            }
        }

        for (j, target_batch) in targets.into_iter().enumerate() {
            if !target_batch.is_empty() {
                self.cases[j].output.write_batch(ctx, target_batch).await?;
            }
        }
        Ok(())
    }
}

/// Implements `broker` output (`internal/impl/pure/output_broker.go`)
/// supporting `fan_out`, `fan_out_fail_fast`, `fan_out_sequential`, `fan_out_sequential_fail_fast`,
/// `round_robin`, and `greedy`.
pub struct BrokerOutput {
    pattern: String,
    outputs: Vec<Arc<dyn Output>>,
    rr_cursor: AtomicUsize,
}

impl BrokerOutput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("broker output config must be an object"))?;

        let copies = obj
            .get("copies")
            .and_then(JsonValue::as_u64)
            .unwrap_or(1)
            .max(1) as usize;
        let pattern = obj
            .get("pattern")
            .and_then(JsonValue::as_str)
            .unwrap_or("fan_out")
            .to_string();
        if !matches!(
            pattern.as_str(),
            "fan_out"
                | "fan_out_fail_fast"
                | "fan_out_sequential"
                | "fan_out_sequential_fail_fast"
                | "round_robin"
                | "greedy"
        ) {
            return Err(CoreError::config(format!(
                "broker pattern was not recognised: {pattern}"
            )));
        }

        let is_retry_wrapped = matches!(pattern.as_str(), "fan_out" | "fan_out_sequential");

        let out_arr = obj
            .get("outputs")
            .and_then(JsonValue::as_array)
            .filter(|a| !a.is_empty())
            .ok_or_else(|| {
                CoreError::config("attempting to create broker output type with no outputs")
            })?;

        let mut outputs = Vec::with_capacity(out_arr.len() * copies);
        for _ in 0..copies {
            for item in out_arr {
                let c = ComponentConfig::from_json_value(item, "drop", None)?;
                let mut o = ctx.registry.build_output(&c, ctx)?;
                if is_retry_wrapped {
                    o = Arc::new(RetryOutput::new_indefinite(o));
                }
                outputs.push(o);
            }
        }

        Ok(Self {
            pattern,
            outputs,
            rr_cursor: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl Output for BrokerOutput {
    fn output_type(&self) -> &'static str {
        "broker"
    }

    async fn write_batch(&self, ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        match self.pattern.as_str() {
            "fan_out" | "fan_out_fail_fast" => {
                let mut futs = Vec::with_capacity(self.outputs.len());
                for out in &self.outputs {
                    let b = batch.iter().map(Part::shallow_copy).collect();
                    futs.push(out.write_batch(ctx, b));
                }
                for res in futures::future::join_all(futs).await {
                    res?;
                }
                Ok(())
            }
            "fan_out_sequential" | "fan_out_sequential_fail_fast" => {
                for out in &self.outputs {
                    let b = batch.iter().map(Part::shallow_copy).collect();
                    out.write_batch(ctx, b).await?;
                }
                Ok(())
            }
            "round_robin" | "greedy" => {
                let start = self.rr_cursor.fetch_add(1, Ordering::Relaxed);
                let mut last_err = None;
                for offset in 0..self.outputs.len() {
                    let idx = (start + offset) % self.outputs.len();
                    match self.outputs[idx].write_batch(ctx, batch.clone()).await {
                        Ok(()) => return Ok(()),
                        Err(e) => last_err = Some(e),
                    }
                }
                Err(last_err.unwrap_or_else(|| CoreError::other("no broker outputs available")))
            }
            _ => unreachable!(),
        }
    }
}

pub struct ResourceOutput {
    name: String,
}

#[async_trait]
impl Output for ResourceOutput {
    fn output_type(&self) -> &'static str {
        "resource"
    }

    async fn write_batch(&self, ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        let out = ctx.resources.get_output(ctx.org_id(), &self.name)?;
        out.write_batch(ctx, batch).await
    }
}

pub struct CacheOutput {
    target: String,
    key: FieldExpression,
}

impl CacheOutput {
    pub fn from_config(conf: &ComponentConfig, ctx: &StreamContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("cache output config must be an object"))?;
        let target = obj
            .get("target")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| CoreError::config("cache output requires a 'target' field"))?
            .to_string();
        let key_str = obj
            .get("key")
            .and_then(JsonValue::as_str)
            .unwrap_or("${! count(\"items\") }");
        let key = ctx.blobl_env.compile_field(key_str)?;
        Ok(Self { target, key })
    }
}

#[async_trait]
impl Output for CacheOutput {
    fn output_type(&self) -> &'static str {
        "cache"
    }

    async fn write_batch(&self, ctx: &StreamContext, mut batch: Batch) -> Result<(), CoreError> {
        let cache = ctx.resources.get_cache(ctx.org_id(), &self.target)?;
        for p in &mut batch {
            let _ = p.as_bytes();
        }
        let view = PartsView(&batch);
        for (i, part) in batch.iter().enumerate() {
            let k = self
                .key
                .resolve_string(i, &view)
                .map_err(|e| CoreError::other(e.to_string()))?;
            cache.set(&k, part.as_bytes_ref(), None).await?;
        }
        Ok(())
    }
}

pub struct SyncResponseOutput;

#[async_trait]
impl Output for SyncResponseOutput {
    fn output_type(&self) -> &'static str {
        "sync_response"
    }

    async fn write_batch(&self, _ctx: &StreamContext, batch: Batch) -> Result<(), CoreError> {
        if let Some(first) = batch.first() {
            if let Some(store) = first.sync_response_store() {
                store.add(&batch);
            }
        }
        Ok(())
    }
}
